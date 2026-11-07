//! Tactical probes: mate-in-2 and win-of-queen. Run with `cargo run --release --example tactics`.

use ferrum::board::Board;
use ferrum::search::{search_position, SearchLimits, MATE};
use ferrum::tt::TranspositionTable;

fn check(name: &str, fen: &str, depth: i32, expect: &str, expect_mate: bool) {
    let mut b = Board::from_fen(fen).unwrap();
    let tt = TranspositionTable::new(32);
    let info = search_position(&mut b, &[], &SearchLimits::fixed_depth(depth), &tt);
    let bm = info.best_move.map(|m| m.to_uci()).unwrap_or("(none)".into());
    println!("{name}: FEN={fen}\n  depth={depth} best={bm} score={} nodes={} expect={} mate={}", info.score, info.nodes, expect, expect_mate);
    let ok_move = bm == expect;
    let ok_mate = if expect_mate { info.score > MATE - 100 } else { true };
    println!("  -> {}", if ok_move && ok_mate { "PASS" } else { "FAIL" });
}

fn main() {
    // Mate in 2 (actually mate in 1: Kc7# is instant mate because Qa1 covers
    // the a-file and Kc7 covers b8 — engine must return a mate score).
    check("mate-in-2", "k7/8/2K5/8/8/8/8/Q7 w - - 0 1", 5, "c6c7", true);
    // Win the queen with a knight fork/capture.
    check("win-queen", "q7/2N5/8/8/8/8/8/K6k w - - 0 1", 3, "c7a8", false);
    // Scholar's mate in 1.
    check(
        "mate-in-1",
        "r1bqkbnr/pppp1ppp/2n5/4p2Q/2B1P3/8/PPPP1PPP/RNB1K1NR w KQkq - 0 1",
        3,
        "h5f7",
        true,
    );
    // Center tension: after 1.e4 e5, the engine should develop / hold the center,
    // not hang a pawn. Just report the move.
    let mut b = Board::from_fen("rnbqkbnr/pppp1ppp/8/4p3/4P3/8/PPPP1PPP/RNBQKBNR w KQkq - 0 1").unwrap();
    let tt = TranspositionTable::new(32);
    let info = search_position(&mut b, &[], &SearchLimits::fixed_depth(5), &tt);
    let bm = info.best_move.map(|m| m.to_uci()).unwrap_or("(none)".into());
    println!("center: best={bm} score={} nodes={}", info.score, info.nodes);
}
