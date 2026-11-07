//! End-to-end UCI smoke test: spawn the built binary, handshake, and search.
//!
//! The binary path is discovered via `CARGO_BIN_EXE_ferrum` (available in
//! integration tests) so `cargo test` builds it first.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

#[test]
fn uci_handshake_and_search() {
    let exe = env!("CARGO_BIN_EXE_ferrum");
    let mut child = Command::new(exe)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn ferrum");

    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            let line = line.unwrap_or_default();
            if tx.send(line).is_err() {
                break;
            }
        }
    });

    let mut send = |s: &str| {
        stdin.write_all(s.as_bytes()).unwrap();
        stdin.write_all(b"\n").unwrap();
        stdin.flush().unwrap();
    };
    let recv_until = |prefix: &str| -> String {
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while std::time::Instant::now() < deadline {
            match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(l) if l.starts_with(prefix) => return l,
                Ok(_) => continue,
                Err(_) => continue,
            }
        }
        panic!("timeout waiting for {prefix}");
    };

    send("uci");
    let l = recv_until("uciok");
    assert!(l.contains("uciok"));
    send("isready");
    let l = recv_until("readyok");
    assert!(l.contains("readyok"));

    send("position startpos moves e2e4 e7e5");
    send("go depth 4");
    let best = recv_until("bestmove");
    eprintln!("bestmove line: {best}");
    assert!(best.starts_with("bestmove "));
    let mv = best.split_whitespace().nth(1).unwrap_or("");
    assert!(mv.len() == 4 || mv.len() == 5, "unexpected bestmove '{mv}'");

    // Mate-in-one via FEN: engine must find Qxf7#.
    send("position fen r1bqkbnr/pppp1ppp/2n5/4p2Q/2B1P3/8/PPPP1PPP/RNB1K1NR w KQkq - 0 1");
    send("go depth 4");
    let best = recv_until("bestmove");
    eprintln!("mate bestmove line: {best}");
    assert_eq!(best.split_whitespace().nth(1).unwrap_or(""), "h5f7");

    // Clock search with stop.
    send("position startpos");
    send("go wtime 10000 btime 10000 winc 100 binc 100");
    std::thread::sleep(Duration::from_millis(100));
    send("stop");
    let best = recv_until("bestmove");
    assert!(best.starts_with("bestmove "));

    // Multithreaded search (Gamma only; Alpha clamps to 1 thread but must not hang).
    send("setoption name Threads value 2");
    send("setoption name Hash value 16");
    send("position startpos");
    send("go depth 4");
    let best = recv_until("bestmove");
    eprintln!("threads bestmove line: {best}");
    assert!(best.starts_with("bestmove "));
    let mv = best.split_whitespace().nth(1).unwrap_or("");
    assert!(mv.len() == 4 || mv.len() == 5, "unexpected threads bestmove '{mv}'");

    // ucinewgame must fully reset state: the board returns to the start
    // position and the next search is a legal, ordinary search of it.
    send("position fen rnbqkbnr/pppp1ppp/8/4p3/4P3/5N2/PPPP1PPP/RNBQKB1R b KQkq - 0 2");
    send("ucinewgame");
    send("d");
    let fen = recv_until("rnbqkbnr/pppppppp");
    assert!(fen.starts_with("rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1"));
    send("go depth 2");
    let best = recv_until("bestmove");
    let mv = best.split_whitespace().nth(1).unwrap_or("");
    assert_eq!(mv.len(), 4, "bestmove after ucinewgame should be a plain startpos move, got '{mv}'");

    send("quit");
    let _ = child.wait_timeout(Duration::from_secs(5));
    // `wait_timeout` needs `wait` fallback for portability.
    match child.try_wait() {
        Ok(Some(_)) => {}
        Ok(None) => {
            let _ = child.kill();
            let _ = child.wait();
        }
        Err(_) => {}
    }
}

// Tiny helper to avoid an extra dependency.
trait WaitTimeout {
    fn wait_timeout(&mut self, dur: Duration) -> std::io::Result<Option<std::process::ExitStatus>>;
}
impl WaitTimeout for std::process::Child {
    fn wait_timeout(&mut self, dur: Duration) -> std::io::Result<Option<std::process::ExitStatus>> {
        let start = std::time::Instant::now();
        loop {
            match self.try_wait()? {
                Some(s) => return Ok(Some(s)),
                None => {
                    if start.elapsed() >= dur {
                        return Ok(None);
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        }
    }
}
