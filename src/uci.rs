//! UCI protocol handling.
//!
//! Implements `uci`, `isready`, `setoption`, `position`, `go` and `quit`
//! plus the test helpers `d`, `eval` and `perft`. The search itself runs in
//! [`crate::search`]; this module only parses commands, manages the board /
//! TT / thread state and prints `bestmove`.

use crate::board::Board;
use crate::search::{Searcher, SearchInfo, SearchLimits};
use crate::tt::{TranspositionTable, DEFAULT_HASH_MB, MAX_HASH_MB};
use crate::types::Color;
use std::io::{BufRead, BufReader, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[cfg(feature = "gamma")]
const ENGINE_NAME: &str = "Ferrum Gamma";
#[cfg(not(feature = "gamma"))]
const ENGINE_NAME: &str = "Ferrum Alpha";
const ENGINE_AUTHOR: &str = "Ferrum";

#[cfg(feature = "threads")]
const MAX_THREADS: usize = 8;
#[cfg(not(feature = "threads"))]
const MAX_THREADS: usize = 1;

pub fn run() {
    let stdin = std::io::stdin();
    let mut reader = BufReader::new(stdin.lock());
    // `Stdout` (not `StdoutLock`) so the search thread can print while the
    // main thread blocks on `read_line`. A held `StdoutLock` would deadlock `go`.
    let mut out = std::io::stdout();

    let mut board = Board::startpos();
    let mut tt = TranspositionTable::new(DEFAULT_HASH_MB);
    let mut threads: usize = 1;
    let mut hash_mb = DEFAULT_HASH_MB;
    let mut game_hashes: Vec<u64> = Vec::new();
    let mut stop_flag = Arc::new(AtomicBool::new(false));
    let mut search_handle: Option<std::thread::JoinHandle<()>> = None;

    // History of the position before the current one, for repetition.
    game_hashes.push(board.hash);

    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => break,
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let mut parts = trimmed.split_whitespace();
        let cmd = parts.next().unwrap_or("");

        match cmd {
            "uci" => {
                let name = ENGINE_NAME;
                let _ = writeln!(out, "id name {name}");
                let _ = writeln!(out, "id author {ENGINE_AUTHOR}");
                let _ = writeln!(out, "option name Hash type spin default {} min 1 max {}", hash_mb, MAX_HASH_MB);
                let _ = writeln!(out, "option name Threads type spin default {} min 1 max {}", threads, MAX_THREADS);
                let _ = writeln!(out, "uciok");
                let _ = out.flush();
            }
            "isready" => {
                let _ = writeln!(out, "readyok");
                let _ = out.flush();
            }
            "ucinewgame" => {
                // A search may still be running from the previous game;
                // stop and join it before swapping board/TT so its result
                // never lands in the new game.
                stop_and_join(&mut search_handle, &mut stop_flag);
                board = Board::startpos();
                tt = TranspositionTable::new(hash_mb);
                game_hashes.clear();
                game_hashes.push(board.hash);
            }
            "setoption" => {
                // `setoption name Hash value N` / `setoption name Threads value N`
                let rest = trimmed["setoption".len()..].trim();
                let mut name = String::new();
                let mut value: Option<String> = None;
                // Parse loosely: `name <name> value <value>`.
                let lower = rest.to_lowercase();
                if let Some(pos) = lower.find("name") {
                    let after = &rest[pos + 4..].trim();
                    if let Some(vpos) = after.to_lowercase().find(" value ") {
                        name = after[..vpos].trim().to_string();
                        value = Some(after[vpos + 7..].trim().to_string());
                    } else {
                        name = after.to_string();
                    }
                }
                if name.eq_ignore_ascii_case("hash") {
                    if let Some(v) = value.and_then(|s| s.parse::<usize>().ok()) {
                        hash_mb = v.clamp(1, MAX_HASH_MB);
                        tt.resize(hash_mb);
                    }
                } else if name.eq_ignore_ascii_case("threads") {
                    if let Some(v) = value.and_then(|s| s.parse::<usize>().ok()) {
                        threads = v.clamp(1, MAX_THREADS);
                    }
                }
            }
            "position" => {
                // Same rule as ucinewgame: once the position changes, any
                // in-flight search is on a stale position. Stop, join and
                // then reset, so no stale bestmove lands in the new game.
                stop_and_join(&mut search_handle, &mut stop_flag);
                board = parse_position(trimmed, &mut game_hashes);
            }
            "go" => {
                // Stop any previous search before starting a new one.
                stop_and_join(&mut search_handle, &mut stop_flag);
                let limits = parse_go(trimmed, &board);
                let tt_clone = tt.clone();
                let board_clone = board.clone();
                let history_clone = game_hashes.clone();
                let flag_clone = stop_flag.clone();
                let handle = std::thread::spawn(move || {
                    let info = search_parallel(&board_clone, &history_clone, &limits, &tt_clone, threads, flag_clone);
                    let stdout = std::io::stdout();
                    let mut out = stdout.lock();
                    match info.best_move {
                        Some(m) => {
                            let _ = writeln!(out, "bestmove {}", m.to_uci());
                        }
                        None => {
                            let _ = writeln!(out, "bestmove (none)");
                        }
                    }
                    let _ = out.flush();
                });
                search_handle = Some(handle);
            }
            "stop" => {
                stop_flag.store(true, Ordering::Relaxed);
            }
            "quit" => {
                stop_and_join(&mut search_handle, &mut stop_flag);
                break;
            }
            // ---- debug / test commands (not part of the UCI spec) ----
            "d" => {
                let _ = writeln!(out, "{}", board.to_fen());
                let _ = out.flush();
            }
            "eval" => {
                let s = crate::eval::evaluate(&board);
                let _ = writeln!(out, "eval {}", s);
                let _ = out.flush();
            }
            "perft" => {
                // `perft <depth>`
                let depth: u32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(3);
                let mut b = board.clone();
                let n = b.perft(depth);
                let _ = writeln!(out, "perft {} nodes {}", depth, n);
                let _ = out.flush();
            }
            "divide" => {
                // `divide <depth>`: per-move counts (textbook divide).
                let depth: u32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(1);
                let mut b = board.clone();
                let moves = b.generate_legal_moves();
                let mut total = 0u64;
                for m in moves.iter() {
                    b.make_move(m);
                    let n = b.perft(depth - 1);
                    b.unmake_move();
                    total += n;
                    let _ = writeln!(out, "{} {}", m.to_uci(), n);
                }
                let _ = writeln!(out, "total {}", total);
                let _ = out.flush();
            }
            _ => {
                // Ignore unknown commands (e.g. `debug`, `ponderhit`).
            }
        }
    }
    // Ensure the search thread finished.
    stop_and_join(&mut search_handle, &mut stop_flag);
}

/// Stop any running search, wait for it, and reset the shared flag so the
/// next search starts unblocked. The joined thread prints its `bestmove`
/// (possibly `bestmove (none)` if stopped before finishing) before any
/// caller resets board/TT, so ordering on stdout is preserved.
fn stop_and_join(search_handle: &mut Option<std::thread::JoinHandle<()>>, stop_flag: &mut Arc<AtomicBool>) {
    stop_flag.store(true, Ordering::Relaxed);
    if let Some(h) = search_handle.take() {
        let _ = h.join();
    }
    *stop_flag = Arc::new(AtomicBool::new(false));
}

fn parse_position(cmd: &str, game_hashes: &mut Vec<u64>) -> Board {
    game_hashes.clear();
    // Forms:
    //   position startpos [moves ...]
    //   position fen <fen fields...> [moves ...]
    let parts = cmd.splitn(3, ' ').collect::<Vec<_>>();
    if parts.len() < 2 {
        return Board::startpos();
    }
    let mut board: Board;
    let moves_start;
    if parts.get(1).map(|s| *s) == Some("startpos") {
        board = Board::startpos();
        moves_start = 2;
    } else if parts.get(1).map(|s| *s) == Some("fen") {
        // FEN is everything until ` moves`.
        let rest = parts.get(2).map(|s| *s).unwrap_or("");
        let (fen, moves_part) = match rest.rfind(" moves") {
            Some(i) => (&rest[..i], Some(&rest[i + 6..])),
            None => (rest, None),
        };
        board = Board::from_fen(fen).unwrap_or_else(|_| Board::startpos());
        game_hashes.push(board.hash);
        // Apply moves (if any) by looping over tokens after `moves`.
        if let Some(moves_str) = moves_part {
            for tok in moves_str.split_whitespace() {
                apply_uci_move(&mut board, tok, game_hashes);
            }
        }
        return board;
    } else {
        return Board::startpos();
    }
    // startpos + moves
    let tokens: Vec<&str> = cmd.split_whitespace().collect();
    for tok in tokens.iter().skip(moves_start) {
        // Skip the literal "moves".
        if *tok == "moves" {
            continue;
        }
        if tok.len() < 4 {
            continue;
        }
        apply_uci_move(&mut board, tok, game_hashes);
    }
    game_hashes.push(board.hash);
    board
}

fn apply_uci_move(board: &mut Board, tok: &str, game_hashes: &mut Vec<u64>) {
    if let Some(m) = board.find_legal(tok) {
        board.make_move(&m);
        game_hashes.push(board.hash);
    }
}

fn parse_go(cmd: &str, board: &Board) -> SearchLimits {
    let mut wtime: Option<u64> = None;
    let mut btime: Option<u64> = None;
    let mut winc: u64 = 0;
    let mut binc: u64 = 0;
    let mut movetime: Option<u64> = None;
    let mut depth: Option<i32> = None;
    let mut nodes: Option<u64> = None;
    let mut movestogo: Option<u64> = None;
    let mut infinite = false;

    let toks: Vec<&str> = cmd.split_whitespace().collect();
    let mut i = 1;
    while i < toks.len() {
        let t = toks[i];
        let next = toks.get(i + 1);
        match t {
            "wtime" => wtime = next.and_then(|s| s.parse().ok()),
            "btime" => btime = next.and_then(|s| s.parse().ok()),
            "winc" => winc = next.and_then(|s| s.parse().ok()).unwrap_or(0),
            "binc" => binc = next.and_then(|s| s.parse().ok()).unwrap_or(0),
            "movetime" => movetime = next.and_then(|s| s.parse().ok()),
            "depth" => depth = next.and_then(|s| s.parse().ok()),
            "nodes" => nodes = next.and_then(|s| s.parse().ok()),
            "movestogo" => movestogo = next.and_then(|s| s.parse().ok()),
            "infinite" => infinite = true,
            _ => {}
        }
        i += 2;
    }

    if let Some(n) = nodes {
        return SearchLimits { time_ms: None, nodes: Some(n), depth: depth.unwrap_or(128), infinite };
    }
    if let Some(d) = depth {
        // Fixed depth: give a generous time budget so deep searches finish.
        return SearchLimits { time_ms: None, nodes: None, depth: d, infinite };
    }
    if let Some(mt) = movetime {
        return SearchLimits { time_ms: Some(mt), nodes: None, depth: 128, infinite: false };
    }
    // Clock-based allocation.
    let remaining = match board.side {
        Color::White => wtime.unwrap_or(1000),
        Color::Black => btime.unwrap_or(1000),
    };
    let inc = match board.side {
        Color::White => winc,
        Color::Black => binc,
    };
    if infinite {
        return SearchLimits { time_ms: Some(remaining), nodes: None, depth: 128, infinite: true };
    }
    let mst = movestogo.unwrap_or(20).max(1);
    let mut alloc = remaining / mst + inc * 3 / 4;
    // Keep a safety margin.
    alloc = alloc.min(remaining.saturating_sub(50).max(1));
    alloc = alloc.max(1);
    SearchLimits { time_ms: Some(alloc), nodes: None, depth: 128, infinite: false }
}

/// Run the search, using multiple threads on Gamma (`threads` feature).
fn search_parallel(
    board: &Board,
    game_history: &[u64],
    limits: &SearchLimits,
    tt: &TranspositionTable,
    threads: usize,
    stop: Arc<AtomicBool>,
) -> SearchInfo {
    #[cfg(not(feature = "threads"))]
    let _ = threads;
    #[cfg(feature = "threads")]
    {
        if threads > 1 {
            // Lazy SMP: the main thread searches normally; helpers only warm
            // the shared TT. Helpers are detached (not scope-joined) so we
            // return the main result the moment it finishes instead of
            // idling until helpers expire (+100 ms latency per move before).
            let board_main = board.clone();
            let tt_main = tt.clone();
            let flag_main = stop.clone();
            let history_main = game_history.to_vec();
            let limits_main = limits.clone();
            let main_handle = std::thread::spawn(move || {
                let mut b = board_main;
                let mut s = Searcher::new(tt_main);
                let printer = |msg: String| {
                    let stdout = std::io::stdout();
                    let mut out = stdout.lock();
                    let _ = writeln!(out, "{msg}");
                    let _ = out.flush();
                };
                // Need to share info output from the main thread only.
                s.search(&mut b, &history_main, &limits_main, Some(flag_main), &printer)
            });
            // Helpers: same budget as the main thread, slightly shallower.
            for h in 0..(threads - 1) {
                let b = board.clone();
                let tt_h = tt.clone();
                let flag_h = stop.clone();
                let hist_h = game_history.to_vec();
                let lim = SearchLimits {
                    time_ms: limits.time_ms,
                    nodes: None,
                    depth: (limits.depth - 1 - h as i32).max(1),
                    infinite: limits.infinite,
                };
                // Detached: `JoinHandle` dropped immediately; the thread
                // exits at the next `should_stop` check once `stop` fires.
                let _ = std::thread::spawn(move || {
                    let mut bb = b;
                    let mut s = Searcher::new(tt_h);
                    let _ = s.search(&mut bb, &hist_h, &lim, Some(flag_h), &|_| {});
                });
            }
            let result = main_handle.join().expect("main search thread panicked");
            // Wind helpers down promptly; they share `stop` (checked every
            // 2048 nodes) and the shared TT, so abandoning them is safe.
            stop.store(true, Ordering::Relaxed);
            return result;
        }
    }
    let mut b = board.clone();
    let mut s = Searcher::new(tt.clone());
    s.search(&mut b, game_history, limits, Some(stop), &|msg: String| {
        let stdout = std::io::stdout();
        let mut out = stdout.lock();
        let _ = writeln!(out, "{msg}");
        let _ = out.flush();
    })
}
