//! Server-side embedding test: run the UCI loop over in-memory I/O.
//!
//! This is the code path a bot uses when it links `ferrum` as a library
//! instead of spawning `ferrum.exe`. It also checks that a malformed FEN is
//! rejected without destroying the current position.

use ferrum::uci;
use std::io::{Cursor, Write};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
struct SharedBuf(Arc<Mutex<Vec<u8>>>);

impl Write for SharedBuf {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn embedded_uci_loop_search_and_bad_fen() {
    let input = b"uci\nisready\n\
        position startpos moves e2e4 e7e5\n\
        go depth 4\n\
        position fen 8/8/8/8/8/8/8/8 w - - 0 1\n\
        d\n\
        quit\n";
    let buf = SharedBuf(Arc::new(Mutex::new(Vec::new())));
    uci::run_with_config(Cursor::new(input), buf.clone(), uci::Config::default());
    let text = String::from_utf8(buf.0.lock().unwrap().clone()).unwrap();

    assert!(text.contains("uciok"), "missing handshake: {text}");
    assert!(text.contains("readyok"), "missing readyok: {text}");
    assert!(text.contains("bestmove "), "missing bestmove: {text}");
    // The bad FEN must be reported and must not replace the position: `d`
    // still prints the last valid FEN (startpos + e2e4 e7e5).
    assert!(
        text.contains("info string invalid position:"),
        "bad FEN was not reported: {text}"
    );
    assert!(
        text.contains("rnbqkbnr/pppp1ppp/8/4p3/4P3/8/PPPP1PPP/RNBQKBNR w KQkq"),
        "position was clobbered by the bad FEN: {text}"
    );
}

#[test]
fn embedded_loop_honours_initial_options() {
    // The CLI builds this same Config for --hash/--threads/--eval-params.
    let eval = ferrum::eval::EvalParams::from_spec("tempo=100").unwrap();
    let cfg = uci::Config {
        hash_mb: 4,
        threads: 2,
        eval,
        ..uci::Config::default()
    };
    let buf = SharedBuf(Arc::new(Mutex::new(Vec::new())));
    uci::run_with_config(Cursor::new(b"position startpos\neval\nquit\n".to_vec()), buf.clone(), cfg);
    let text = String::from_utf8(buf.0.lock().unwrap().clone()).unwrap();
    assert!(text.contains("eval 100"), "EvalParams config not applied: {text}");
}
