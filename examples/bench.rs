//! Quick strength / speed probe: search the start position at fixed depths
//! and report nodes + nps. Run with `cargo run --release --example bench`.

use ferrum::board::Board;
use ferrum::search::{search_position, SearchLimits};
use ferrum::tt::TranspositionTable;

fn main() {
    let tt = TranspositionTable::new(64);
    for depth in [4, 6, 7, 8] {
        let mut b = Board::startpos();
        let t0 = std::time::Instant::now();
        let info = search_position(&mut b, &[], &SearchLimits::fixed_depth(depth), &tt);
        let elapsed = t0.elapsed();
        let bm = info.best_move.map(|m| m.to_uci()).unwrap_or_else(|| "(none)".into());
        let pv: Vec<String> = info.pv.iter().map(|m| m.to_uci()).collect();
        println!(
            "depth {} best {} score {} nodes {} elapsed {:?} pv_len {} pv={}",
            info.depth,
            bm,
            info.score,
            info.nodes,
            elapsed,
            info.pv.len(),
            pv.join(" ")
        );
    }
}
