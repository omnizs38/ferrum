//! Evaluate-throughput microbenchmark.
//!
//! Times `eval::evaluate` on a fixed set of middlegame/endgame positions so
//! evaluation cost can be tracked separately from search throughput. Run with
//! `cargo run --release --example evalbench`.

use ferrum::board::Board;
use ferrum::eval::evaluate;
use std::time::Instant;

fn main() {
    let fens = [
        "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1",
        "r1bq1rk1/pp2ppbp/2np1np1/8/2PPP3/2N2N2/PP2BPPP/R1BQ1RK1 w - - 0 9",
        "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1",
        "8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1",
        "r2q1rk1/1b1nbppp/p2ppn2/1p6/3NPP2/PBN1B3/1PP2PPP/R2Q1RK1 w - - 0 12",
        "8/5pk1/6p1/8/3P4/5K2/6P1/8 w - - 0 40",
    ];
    let boards: Vec<Board> = fens
        .iter()
        .map(|f| Board::from_fen_strict(f).expect("valid bench FEN"))
        .collect();

    // Warm up.
    let mut sink = 0i64;
    for b in &boards {
        sink += evaluate(b) as i64;
    }

    let budget = std::time::Duration::from_millis(2000);
    let start = Instant::now();
    let mut n = 0u64;
    while start.elapsed() < budget {
        for b in &boards {
            sink += evaluate(b) as i64;
            n += 1;
        }
    }
    let elapsed = start.elapsed();
    let ns = elapsed.as_nanos() as f64 / n as f64;
    println!("{n} evals in {elapsed:?}: {ns:.1} ns/eval (sink {sink})");
}
