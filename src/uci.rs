//! UCI protocol handling.
//!
//! Implements `uci`, `isready`, `setoption`, `position`, `go` and `quit`
//! plus the test helpers `d`, `eval`, `perft` and `divide`. The search runs in
//! [`crate::search`]; this module only parses commands, manages the board /
//! TT / thread state and prints `bestmove`.
//!
//! [`run_with_io`] runs the same loop over an arbitrary `BufRead`/`Write`
//! pair, which is what servers embedding the engine (rather than spawning a
//! process) use. The binary calls [`run_with_config`] with its CLI options;
//! [`run`] is the same loop on stdin/stdout with the defaults.

use crate::board::Board;
use crate::eval::{evaluate_with, EvalParams};
use crate::search::{SearchInfo, SearchLimits, SearchParams, Searcher};
use crate::tt::{default_hash_mb, TranspositionTable, MAX_HASH_MB};
use crate::types::Color;
use std::io::{BufRead, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

#[cfg(feature = "gamma")]
pub const ENGINE_NAME: &str = "Ferrum Gamma";
#[cfg(not(feature = "gamma"))]
pub const ENGINE_NAME: &str = "Ferrum Alpha";
pub const ENGINE_AUTHOR: &str = "Ferrum";

// Shared with the C ABI, which advertises the same limit.
#[cfg(feature = "threads")]
pub(crate) const MAX_THREADS: usize = 8;
#[cfg(not(feature = "threads"))]
pub(crate) const MAX_THREADS: usize = 1;

/// Thread-safe line writer. A server may attach the engine to a pipe/socket
/// (or several engines to one log), so every write is short and locks only
/// for the duration of one line — never while joining a search thread.
pub struct Output<W: Write> {
    inner: Arc<Mutex<W>>,
}

impl<W: Write> Clone for Output<W> {
    fn clone(&self) -> Self {
        Output { inner: Arc::clone(&self.inner) }
    }
}

impl<W: Write> Output<W> {
    pub fn new(writer: W) -> Output<W> {
        Output { inner: Arc::new(Mutex::new(writer)) }
    }

    pub fn line(&self, text: &str) {
        if let Ok(mut w) = self.inner.lock() {
            let _ = writeln!(w, "{text}");
            let _ = w.flush();
        }
    }

    pub fn flush(&self) {
        if let Ok(mut w) = self.inner.lock() {
            let _ = w.flush();
        }
    }
}

/// Runtime configuration for [`run_with_config`]. The binary's CLI flags
/// map onto these fields; embedders can too.
#[derive(Clone, Debug)]
pub struct Config {
    pub hash_mb: usize,
    pub threads: usize,
    pub eval: EvalParams,
    pub search: SearchParams,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            hash_mb: default_hash_mb(),
            threads: 1,
            eval: EvalParams::DEFAULT,
            search: SearchParams::DEFAULT,
        }
    }
}

/// Run the UCI loop on stdin/stdout (the normal binary entry point).
pub fn run() {
    run_with_config(std::io::stdin().lock(), std::io::stdout(), Config::default());
}

/// Run the UCI loop over any reader/writer pair with default options. The
/// writer is shared with the background search thread, so it must be
/// `Send + 'static` (use a `Vec<u8>`/`Cursor` pair in tests, a pipe or a
/// socket in servers).
pub fn run_with_io<R: BufRead, W: Write + Send + 'static>(reader: R, writer: W) {
    run_with_config(reader, writer, Config::default());
}

/// Run the UCI loop with explicit initial options.
pub fn run_with_config<R: BufRead, W: Write + Send + 'static>(
    mut reader: R,
    writer: W,
    config: Config,
) {
    let out = Output::new(writer);

    let mut board = Board::startpos();
    let mut hash_mb = config.hash_mb.clamp(1, MAX_HASH_MB);
    let mut tt = TranspositionTable::new(hash_mb);
    let mut threads: usize = config.threads.clamp(1, MAX_THREADS);
    let mut eval_params = config.eval;
    let mut search_params = config.search;
    // History of the position before the current one, for repetition.
    let mut game_hashes: Vec<u64> = vec![board.hash];
    let mut stop_flag = Arc::new(AtomicBool::new(false));
    let mut search_handle: Option<std::thread::JoinHandle<()>> = None;

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
                out.line(&format!("id name {name}"));
                out.line(&format!("id author {ENGINE_AUTHOR}"));
                out.line(&format!(
                    "option name Hash type spin default {hash_mb} min 1 max {MAX_HASH_MB}"
                ));
                out.line(&format!(
                    "option name Threads type spin default {threads} min 1 max {MAX_THREADS}"
                ));
                // Training hooks: `key=value,...` specs (see `EvalParams` and
                // `SearchParams`). Empty / "default" = trained baseline.
                out.line("option name EvalParams type string default default");
                out.line("option name SearchParams type string default default");
                out.line("uciok");
            }
            "isready" => {
                out.line("readyok");
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
                // Parse loosely: `name <name> value <value>`. The search is
                // ASCII case-insensitive and indexes the original string, so
                // multi-byte option text cannot shift the slice boundaries.
                if let Some(pos) = find_ci(rest, "name") {
                    let after = rest[pos + 4..].trim();
                    if let Some(vpos) = find_ci(after, " value ") {
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
                } else if name.eq_ignore_ascii_case("evalparams") {
                    if let Some(spec) = value {
                        match EvalParams::from_spec_or_default(&spec) {
                            Ok(p) => eval_params = p,
                            Err(e) => eprintln!("EvalParams rejected: {e}"),
                        }
                    }
                } else if name.eq_ignore_ascii_case("searchparams") {
                    if let Some(spec) = value {
                        match SearchParams::from_spec_or_default(&spec) {
                            Ok(p) => search_params = p,
                            Err(e) => eprintln!("SearchParams rejected: {e}"),
                        }
                    }
                }
            }
            "position" => {
                // Same rule as ucinewgame: once the position changes, any
                // in-flight search is on a stale position. Stop, join and
                // then reset, so no stale bestmove lands in the new game.
                stop_and_join(&mut search_handle, &mut stop_flag);
                match parse_position(trimmed) {
                    Ok((b, h)) => {
                        board = b;
                        game_hashes = h;
                    }
                    Err(e) => {
                        // Keep the previous position; a server must not start
                        // searching a bogus board because one request was bad.
                        out.line(&format!("info string invalid position: {e}"));
                        eprintln!("position rejected: {e}");
                    }
                }
            }
            "go" => {
                // Stop any previous search before starting a new one.
                stop_and_join(&mut search_handle, &mut stop_flag);
                let limits = parse_go(trimmed, &board);
                let tt_clone = tt.clone();
                let board_clone = board.clone();
                let history_clone = game_hashes.clone();
                let flag_clone = stop_flag.clone();
                let params = eval_params;
                let sparams = search_params;
                let out_thread = out.clone();
                let handle = std::thread::spawn(move || {
                    let info = search_parallel(
                        &board_clone,
                        &history_clone,
                        &limits,
                        &tt_clone,
                        threads,
                        flag_clone,
                        params,
                        sparams,
                        out_thread.clone(),
                    );
                    match info.best_move {
                        Some(m) => out_thread.line(&format!("bestmove {}", m.to_uci())),
                        None => out_thread.line("bestmove (none)"),
                    }
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
                out.line(&board.to_fen());
            }
            "eval" => {
                let s = evaluate_with(&board, &eval_params);
                out.line(&format!("eval {s}"));
            }
            "perft" => {
                // `perft <depth>`
                let depth: u32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(3).min(8);
                let mut b = board.clone();
                let n = b.perft(depth);
                out.line(&format!("perft {depth} nodes {n}"));
            }
            "divide" => {
                // `divide <depth>`: per-move counts (textbook divide).
                let depth: u32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(1).clamp(1, 8);
                let mut b = board.clone();
                let moves = b.generate_legal_moves();
                let mut total = 0u64;
                for m in moves.iter() {
                    b.make_move(m);
                    let n = b.perft(depth - 1);
                    b.unmake_move();
                    total += n;
                    out.line(&format!("{} {}", m.to_uci(), n));
                }
                out.line(&format!("total {total}"));
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
        // A panicking search must not take the server down: the thread
        // unwinds, the join result is ignored and the loop continues.
        let _ = h.join();
    }
    *stop_flag = Arc::new(AtomicBool::new(false));
}

/// ASCII case-insensitive substring search, returning a byte offset into
/// `haystack`. ASCII folding preserves byte lengths, so slicing the original
/// string at the returned offset is safe even for non-ASCII option text.
fn find_ci(haystack: &str, needle: &str) -> Option<usize> {
    let n = needle.as_bytes();
    if n.is_empty() {
        return Some(0);
    }
    haystack
        .as_bytes()
        .windows(n.len())
        .position(|w| w.eq_ignore_ascii_case(n))
}

/// A start position plus its repetition history.
fn startpos_position() -> (Board, Vec<u64>) {
    let board = Board::startpos();
    let hash = board.hash;
    (board, vec![hash])
}

fn parse_position(cmd: &str) -> Result<(Board, Vec<u64>), String> {
    let mut game_hashes: Vec<u64> = Vec::new();
    // Forms:
    //   position startpos [moves ...]
    //   position fen <fen fields...> [moves ...]
    let parts = cmd.splitn(3, ' ').collect::<Vec<_>>();
    if parts.len() < 2 {
        return Ok(startpos_position());
    }
    let mut board: Board;
    if parts.get(1).copied() == Some("startpos") {
        board = Board::startpos();
    } else if parts.get(1).copied() == Some("fen") {
        let rest = parts.get(2).copied().unwrap_or("");
        let (fen, moves_part) = match rest.rfind(" moves") {
            Some(i) => (&rest[..i], Some(&rest[i + 6..])),
            None => (rest, None),
        };
        board = Board::from_fen_strict(fen)?;
        game_hashes.push(board.hash);
        if let Some(moves_str) = moves_part {
            for tok in moves_str.split_whitespace() {
                apply_uci_move(&mut board, tok, &mut game_hashes)?;
            }
        }
        return Ok((board, game_hashes));
    } else {
        return Ok(startpos_position());
    }
    // startpos + moves
    let tokens: Vec<&str> = cmd.split_whitespace().collect();
    for tok in tokens.iter().skip(2) {
        // Skip the literal "moves".
        if *tok == "moves" {
            continue;
        }
        if tok.len() < 4 {
            continue;
        }
        apply_uci_move(&mut board, tok, &mut game_hashes)?;
    }
    game_hashes.push(board.hash);
    Ok((board, game_hashes))
}

fn apply_uci_move(board: &mut Board, tok: &str, game_hashes: &mut Vec<u64>) -> Result<(), String> {
    match board.find_legal(tok) {
        Some(m) => {
            board.make_move(&m);
            game_hashes.push(board.hash);
            Ok(())
        }
        None => Err(format!("illegal move '{tok}'")),
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
#[allow(clippy::too_many_arguments)]
fn search_parallel<W: Write + Send + 'static>(
    board: &Board,
    game_history: &[u64],
    limits: &SearchLimits,
    tt: &TranspositionTable,
    threads: usize,
    stop: Arc<AtomicBool>,
    params: EvalParams,
    sparams: SearchParams,
    out: Output<W>,
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
                let mut s = Searcher::with_settings(tt_main, params, sparams);
                s.search(&mut b, &history_main, &limits_main, Some(flag_main), &|msg: String| {
                    out.line(&msg);
                })
            });
            // Helpers: same budget as the main thread, slightly shallower.
            for h in 0..(threads - 1) {
                let b = board.clone();
                let tt_h = tt.clone();
                let flag_h = stop.clone();
                let hist_h = game_history.to_vec();
                let lim = limits.helper(h);
                // Detached: `JoinHandle` dropped immediately; the thread
                // exits at the next `should_stop` check once `stop` fires.
                let _ = std::thread::spawn(move || {
                    let mut bb = b;
                    let mut s = Searcher::with_settings(tt_h, params, sparams);
                    let _ = s.search(&mut bb, &hist_h, &lim, Some(flag_h), &|_| {});
                });
            }
            let result = main_handle.join().unwrap_or_else(|_| {
                // The main search panicked: return a safe empty result
                // instead of propagating the panic into the server thread.
                SearchInfo {
                    best_move: None,
                    score: 0,
                    depth: 0,
                    nodes: 0,
                    time_ms: 0,
                    pv: Vec::new(),
                }
            });
            // Wind helpers down promptly; they share `stop` (checked every
            // 512 nodes) and the shared TT, so abandoning them is safe.
            stop.store(true, Ordering::Relaxed);
            return result;
        }
    }
    let mut b = board.clone();
    let mut s = Searcher::with_settings(tt.clone(), params, sparams);
    s.search(&mut b, game_history, limits, Some(stop), &|msg: String| {
        out.line(&msg);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_ci_offsets_the_original_text() {
        assert_eq!(find_ci("name Hash value 32", "name"), Some(0));
        assert_eq!(find_ci("NAME Hash VALUE 32", " value "), Some(9));
        assert_eq!(find_ci("näme name x", "name"), Some(6));
        assert_eq!(find_ci("näme", "value"), None);
    }

    #[test]
    fn parse_position_accepts_startpos_and_fen() {
        let (b, h) = parse_position("position startpos moves e2e4 e7e5").unwrap();
        assert_eq!(
            b.to_fen(),
            "rnbqkbnr/pppp1ppp/8/4p3/4P3/8/PPPP1PPP/RNBQKBNR w KQkq e6 0 2"
        );
        assert_eq!(h.len(), 3); // start position + two moves
        let (b, _) = parse_position("position fen 8/8/8/4k3/8/8/8/4K3 w - - 0 1").unwrap();
        assert!(b.to_fen().starts_with("8/8/8/4k3/"));
        assert!(parse_position("position fen bad").is_err());
    }

    #[test]
    fn go_limits_cover_depth_nodes_and_clock() {
        let b = Board::startpos();
        let limits = parse_go("go depth 7", &b);
        assert_eq!(limits.depth, 7);
        assert!(limits.time_ms.is_none());

        let limits = parse_go("go nodes 5000", &b);
        assert_eq!(limits.nodes, Some(5000));

        let limits = parse_go("go movetime 250", &b);
        assert_eq!(limits.time_ms, Some(250));

        let limits = parse_go("go wtime 1000 btime 1000 winc 10 binc 10", &b);
        let ms = limits.time_ms.unwrap();
        assert!((1..=1000).contains(&ms));
    }
}
