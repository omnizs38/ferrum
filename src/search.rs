//! Search: negamax alpha-beta with iterative deepening.
//!
//! Quiescence search is part of the **shared core** now: both variants
//! resolve captures at the horizon (Alpha's tactical blind spot was the
//! single largest gap to its rating target). The Gamma build adds the extra
//! aggressive set — null-move pruning, late-move reduction, razoring and
//! check extensions — behind the `qsearch` feature, plus advanced evaluation
//! and optional multithreading (`threads`). What remains shared —
//! transposition table, move ordering, killers/history, delta/SEE pruning,
//! reverse futility, late-move pruning, futility and qsearch itself — is the
//! common core, because Alpha *is* Gamma with features removed.
//!
//! Every `evaluate` call goes through `evaluate_with(&self.params)` so the
//! training harness (`examples/train.rs`) can search with trained weights.
//!
//! Scores are centipawns from the side-to-move's perspective.
//! `MATE` = 32000; mate in `n` is `MATE - n`.

use crate::bitboard::{
    bishop_attacks, bit, king_attacks, knight_attacks, pawn_attackers_to, pop_lsb, popcount,
    rook_attacks,
};
use crate::board::Board;
use crate::eval::{evaluate_with, EvalParams};
use crate::params::param_accessors;
use crate::tt::{Bound, TranspositionTable};
use crate::types::{Color, Move, MoveList, PieceType, MAX_MOVES};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub const MATE: i32 = 32000;
pub const MAX_PLY: usize = 128;

pub const INF: i32 = 1_000_000;

#[derive(Clone, Debug)]
pub struct SearchLimits {
    pub time_ms: Option<u64>,
    pub nodes: Option<u64>,
    pub depth: i32,
    pub infinite: bool,
}

impl SearchLimits {
    pub fn fixed_depth(depth: i32) -> SearchLimits {
        SearchLimits { time_ms: None, nodes: None, depth, infinite: false }
    }
}

/// Trainable search prunings. `DEFAULT` is the trained baseline; a match of
/// "default" vs a candidate isolates the change being tested. The trainer
/// passes these as `key=value,...` specs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SearchParams {
    /// Reverse futility: at depth <= `rfp_max_depth`, a non-PV node whose
    /// static eval beats beta by `rfp_margin * depth` is cut immediately.
    /// 0 = off.
    pub rfp_margin: i32,
    pub rfp_max_depth: i32,
    /// Futility: at depth 1, skip quiet moves when `eval + margin <= alpha`.
    /// 0 = off.
    pub futility_margin: i32,
    /// Late-move pruning: at depth <= `lmp_max_depth`, skip quiet moves at
    /// index >= `lmp_base + depth*depth`. 0 = off.
    pub lmp_base: i32,
    pub lmp_max_depth: i32,
    /// LMR divisor, in hundredths of a ply: 0 keeps the historical fixed
    /// one-ply reduction; a positive value enables the log formula
    /// `r = lmr_base/100 + ln(new_depth)*ln(index) / (lmr_div/100)` plies.
    /// The formula path clamps `r` to `1..=new_depth-1`, so it never drops
    /// below depth 1. `lmr_div=400` is the classic divisor 4.0 (the old
    /// unscaled knob's `4`); 225 matches the traditional 2.25. Swept
    /// 150/225/300/400 against the fixed reduction: see `ELO.md`.
    pub lmr_div: i32,
    /// LMR log-formula base, in hundredths of a ply (75 = 0.75 plies).
    /// Only used when `lmr_div > 0`.
    pub lmr_base: i32,
    /// Minimum remaining depth (`new_depth`, in plies) at which LMR runs.
    /// 1 is the historical gate (reduce whenever `new_depth > 0`); 2 stops
    /// the fixed one-ply reduction from pushing a depth-1 child into
    /// qsearch. The log formula already skips `new_depth < 2`, because
    /// reducing a depth-1 child would leave nothing but qsearch.
    pub lmr_min_depth: i32,
    /// 1 = allow quiet reductions at PV nodes too (still from the third
    /// move on). 0 keeps PV nodes at full depth. Measured in the
    /// 2026-11-17 sweep; the default is off.
    pub lmr_pv: i32,
    /// Fixed reduction in plies for losing captures (`SEE < 0`), from the
    /// third move on and only at non-PV nodes unless `lmr_pv` is on.
    /// 0 = off. Measured in the 2026-11-17 sweep; the default is off.
    pub lmr_captures: i32,
    /// After this many quiet moves have failed to raise alpha, add one
    /// extra ply of reduction (the extra ply is clamped so the probe
    /// stays at depth 1 or deeper). 0 = off.
    pub lmr_fail_threshold: i32,
    /// 1 = re-search a reduced move at full depth whenever its probe
    /// returns `score > alpha`, including fail-highs. This is the standard
    /// LMR rule and the correctness fix for zero-window cut nodes, where
    /// `score < beta` can never hold. 0 keeps the pre-2026-11-17 in-window
    /// re-search only, which never re-searches a reduced cut-node move.
    /// Unreduced zero-window fail-highs are not re-searched twice: the
    /// repeat at the same depth and window would return the same score.
    /// Measured 2026-11-18: +20 Elo over 1200 games and +8 over another
    /// 1600 at 20k nodes (pooled +13, 95% CI ~[0,+26]), and +6 at a fixed
    /// 50 ms clock over 800 games. Adopted as the default; 0 reproduces the
    /// pre-2026-11-18 tree.
    pub lmr_research: i32,
    /// 1 = resolve captures at the horizon with quiescence; 0 = static eval
    /// (the pre-training Alpha behaviour). Exposed so training can A/B the
    /// depth-vs-tactics trade-off at a fixed clock.
    pub qsearch: i32,
    /// Maximum capture plies inside quiescence (0 = unlimited). A low cap
    /// keeps qsearch cheap on engines without null-move/LMR pruning.
    pub qs_max_plies: i32,
    /// Quiescence SEE pruning threshold: captures with `SEE < -margin` are
    /// skipped. 0 prunes every losing capture (the trained baseline).
    pub qs_see_margin: i32,
    /// Quiescence delta-pruning margin in centipawns (baseline 200).
    pub qs_delta_margin: i32,
    /// Null-move reduction: `R = null_r_base + depth / null_r_div`.
    pub null_r_base: i32,
    pub null_r_div: i32,
    /// Aspiration window half-width in centipawns at the root.
    pub asp_delta: i32,
    /// Internal iterative deepening: at PV nodes with no TT move, do a
    /// reduced-depth search for a move to order first. 0 = off, otherwise
    /// the minimum depth at which it runs.
    pub iid_min_depth: i32,
    /// ProbCut: at non-PV nodes, prove a cutoff above `beta + margin` with
    /// a reduced search instead of the full move loop, using captures only.
    /// 0 = off.
    pub probcut_margin: i32,
    /// Minimum depth at which ProbCut runs; it pays off at high depth, where
    /// the reduced verifier is much cheaper than the full-width search.
    pub probcut_min_depth: i32,
    /// Plies subtracted from the depth for ProbCut's verifier.
    pub probcut_r: i32,
}

impl Default for SearchParams {
    fn default() -> Self {
        SearchParams::DEFAULT
    }
}

impl SearchParams {
    /// Trained baseline (2026-11-15): late-move pruning, reverse futility
    /// and depth-1 futility, plus a resolved horizon (quiescence).
    ///
    /// Gamma: +160 Elo (CI +131..+193) at 20k nodes/move over 600 paired
    /// games vs all-off, and +77 Elo (CI +43..+113) at 80k nodes/move over
    /// 400 games.
    ///
    /// 2026-11-18: standard LMR re-search at cut nodes is on
    /// (`lmr_research=1`), measured +13 Elo pooled over 2800 games at 20k
    /// nodes and +6 at a fixed 50 ms clock. It costs nodes (the bench tree
    /// changes) but wins at both fixed nodes and fixed time.
    ///
    /// Alpha: +160 Elo at a fixed 50 ms clock over 200 games vs all-off, and
    /// qsearch adds another +98 Elo (CI +50..+151) once the prunings pay for
    /// its depth cost. Without the prunings, qsearch alone measured -262 Elo
    /// (it cost more depth than the tactics were worth), which is why the
    /// two are trained and kept together.
    pub const DEFAULT: SearchParams = SearchParams {
        rfp_margin: 120,
        rfp_max_depth: 6,
        futility_margin: 100,
        lmp_base: 4,
        lmp_max_depth: 4,
        lmr_div: 0,
        lmr_base: 75,
        lmr_min_depth: 1,
        lmr_pv: 0,
        lmr_captures: 0,
        lmr_fail_threshold: 0,
        lmr_research: 1,
        qsearch: 1,
        qs_max_plies: 0,
        qs_see_margin: 0,
        qs_delta_margin: 200,
        null_r_base: 2,
        null_r_div: 4,
        asp_delta: 50,
        iid_min_depth: 0,
        probcut_margin: 0,
        probcut_min_depth: 5,
        probcut_r: 4,
    };

    // Keys, `set` and `get` are generated from this one field list.
    param_accessors!("search";
        rfp_margin, rfp_max_depth, futility_margin,
        lmp_base, lmp_max_depth, lmr_div,
        lmr_base, lmr_min_depth, lmr_pv, lmr_captures,
        lmr_fail_threshold, lmr_research,
        qsearch, qs_max_plies, qs_see_margin, qs_delta_margin,
        null_r_base, null_r_div, asp_delta, iid_min_depth,
        probcut_margin, probcut_min_depth, probcut_r
    );

    pub fn from_spec(spec: &str) -> Result<SearchParams, String> {
        let mut p = SearchParams::DEFAULT;
        crate::params::parse_spec(spec, |k, v| p.set(k, v))?;
        Ok(p)
    }

    pub fn to_spec(&self) -> String {
        let mut out = String::new();
        for &key in Self::KEYS {
            if !out.is_empty() {
                out.push(',');
            }
            out.push_str(key);
            out.push('=');
            out.push_str(&self.get(key).to_string());
        }
        out
    }
}

#[derive(Clone, Debug)]
pub struct SearchInfo {
    pub best_move: Option<Move>,
    pub score: i32,
    pub depth: i32,
    pub nodes: u64,
    pub time_ms: u64,
    pub pv: Vec<Move>,
}

pub struct Searcher {
    tt: TranspositionTable,
    pub nodes: u64,
    /// Evaluation weights used by this searcher (training hook).
    pub params: EvalParams,
    /// Search-pruning settings used by this searcher (training hook).
    pub sparams: SearchParams,
    killers: [Option<Move>; MAX_PLY * 2],
    history: [[i32; 64]; 12],
    pv_len: [usize; MAX_PLY],
    pv_table: [[Option<Move>; MAX_PLY]; MAX_PLY],
    path_hashes: Vec<u64>,
    game_hashes: Vec<u64>,
    stop: Option<std::sync::Arc<AtomicBool>>,
    hard_deadline: Option<Instant>,
    node_limit: Option<u64>,
}

impl Searcher {
    pub fn new(tt: TranspositionTable) -> Searcher {
        Searcher::with_params(tt, EvalParams::DEFAULT)
    }

    /// A searcher that evaluates with explicit (e.g. trained) weights.
    pub fn with_params(tt: TranspositionTable, params: EvalParams) -> Searcher {
        Searcher::with_settings(tt, params, SearchParams::DEFAULT)
    }

    /// Full training hook: evaluation weights plus search prunings.
    pub fn with_settings(tt: TranspositionTable, params: EvalParams, sparams: SearchParams) -> Searcher {
        Searcher {
            tt,
            nodes: 0,
            params,
            sparams,
            killers: [None; MAX_PLY * 2],
            history: [[0; 64]; 12],
            pv_len: [0; MAX_PLY],
            pv_table: [[None; MAX_PLY]; MAX_PLY],
            path_hashes: Vec::with_capacity(MAX_PLY),
            game_hashes: Vec::new(),
            stop: None,
            hard_deadline: None,
            node_limit: None,
        }
    }

    fn slot(&self, board: &Board, from: u8) -> usize {
        match board.piece_on(from) {
            Some(p) => p.color.idx() * 6 + p.kind.idx(),
            None => 0,
        }
    }

    /// Iterative-deepening root search. `on_info` is called with UCI info
    /// strings after every completed iteration.
    pub fn search(
        &mut self,
        board: &mut Board,
        game_history: &[u64],
        limits: &SearchLimits,
        stop_flag: Option<std::sync::Arc<AtomicBool>>,
        on_info: &dyn Fn(String),
    ) -> SearchInfo {
        self.nodes = 0;
        self.killers = [None; MAX_PLY * 2];
        self.history = [[0; 64]; 12];
        self.game_hashes = game_history.to_vec();
        self.stop = stop_flag.clone();
        self.node_limit = limits.nodes;
        self.hard_deadline = limits.time_ms.map(|t| Instant::now() + Duration::from_millis(t));
        self.tt.new_search();

        let start = Instant::now();
        let max_depth = limits.depth.max(1);
        let mut best_move: Option<Move> = self.tt.probe_move(board.hash);
        let mut best_score = 0i32;
        let mut completed_depth = 0i32;
        // Last *completed* iteration's PV. A partial iteration can reset the
        // PV table (and then be discarded), which used to leave `SearchInfo`
        // with an empty PV even though a full line had been found — bad for
        // library/FFI callers that report the PV to a bot.
        let mut best_pv: Vec<Move> = Vec::new();

        // Order root moves by TT move first so the first iteration is sensible.
        for depth in 1..=max_depth {
            if self.should_stop() && completed_depth > 0 {
                break;
            }
            let (score, mv) = self.root_iter(board, depth, best_score);
            if self.should_stop() && completed_depth > 0 {
                // Discard the partial iteration; keep the previous best.
                break;
            }
            best_score = score;
            if mv.is_some() {
                best_move = mv;
            }
            completed_depth = depth;
            // Emit a UCI info line after every completed iteration.
            {
                let elapsed = start.elapsed().as_millis().max(1) as u64;
                let nps = (self.nodes * 1000) / elapsed;
                let pv_moves = self.extract_pv();
                best_pv = pv_moves.clone();
                let pv_str = pv_moves
                    .iter()
                    .map(Move::to_uci)
                    .collect::<Vec<_>>()
                    .join(" ");
                let score_str = match mate_in(best_score) {
                    Some(n) => format!("mate {n}"),
                    None => format!("cp {best_score}"),
                };
                let pv_suffix = if pv_str.is_empty() {
                    String::new()
                } else {
                    format!(" pv {pv_str}")
                };
                on_info(format!(
                    "info depth {} score {} nodes {} nps {} time {}{}",
                    completed_depth, score_str, self.nodes, nps, elapsed, pv_suffix
                ));
            }
            if limits.infinite {
                continue;
            }
        }

        let time_ms = start.elapsed().as_millis() as u64;
        let pv = if best_pv.is_empty() { self.extract_pv() } else { best_pv };
        // A search stopped before its first iteration completes (immediate
        // `quit`, a tiny clock, a stopped server request) must still hand the
        // caller a legal move when one exists — bots rely on that.
        if best_move.is_none() {
            let legal = board.generate_legal_moves();
            best_move = legal.as_slice().first().copied();
        }
        SearchInfo { best_move, score: best_score, depth: completed_depth, nodes: self.nodes, time_ms, pv }
    }

    fn extract_pv(&self) -> Vec<Move> {
        let mut pv = Vec::new();
        let n = self.pv_len[0];
        for i in 0..n {
            if let Some(m) = self.pv_table[0][i] {
                pv.push(m);
            }
        }
        pv
    }

    fn root_iter(&mut self, board: &mut Board, depth: i32, prev_score: i32) -> (i32, Option<Move>) {
        // Aspiration windows around the previous iteration's score.
        // On a miss we re-center on the returned score (not just widen one
        // side) so the score lies strictly inside the next window — with
        // one-sided widening a score exactly on the edge (e.g. 30 in [30,90])
        // oscillates forever. A hard cap falls back to full window.
        // Delta 50 (was 30): fewer fail-high/low re-searches at short TC.
        // Trainable via `SearchParams::asp_delta`.
        let mut delta = self.sparams.asp_delta.max(1);
        let (mut alpha, mut beta) = if depth == 1 {
            (-INF, INF)
        } else {
            (prev_score - delta, prev_score + delta)
        };
        for attempt in 0..5 {
            let (score, mv) = self.negamax(board, depth, alpha, beta, 0, true, true);
            if self.should_stop() {
                return (prev_score, mv);
            }
            if score > alpha && score < beta {
                return (score, mv);
            }
            if attempt == 4 {
                // The fifth attempt used the widest window; fall through to
                // one final full-window search rather than widening forever.
                break;
            }
            // Missed the window: re-center on the score with doubled radius.
            delta *= 2;
            alpha = score - delta;
            beta = score + delta;
        }
        // Should not reach here (attempt 4 uses a wide window), but return
        // the last search result rather than looping forever.
        self.negamax(board, depth, -INF, INF, 0, true, true)
    }

    /// Cheap check: the clock and node limit are only consulted every
    /// 512 nodes, so most calls short-circuit for free. 512 (not 2048) so
    /// the resolution stays well below the shortest clock budgets — a
    /// `go movetime 1` gets checked ~every 100 µs instead of every ~400 µs.
    fn should_stop(&self) -> bool {
        if self.nodes.is_multiple_of(512) {
            if let Some(d) = self.hard_deadline {
                if Instant::now() >= d {
                    return true;
                }
            }
            if let Some(n) = self.node_limit {
                if self.nodes >= n {
                    return true;
                }
            }
            if let Some(f) = &self.stop {
                if f.load(Ordering::Relaxed) {
                    return true;
                }
            }
        }
        false
    }

    /// Negamax with alpha-beta, PVS, null-move pruning and LMR (the last
    /// two are Gamma-only; Alpha searches every move in full).
    /// `pv_node` marks nodes on the principal variation.
    /// `allow_null` is false for the node right after a null move.
    // The flat parameter list mirrors the standard negamax signature; a
    // context struct would only add indirection in the hottest function.
    #[allow(clippy::too_many_arguments)]
    fn negamax(&mut self, board: &mut Board, depth: i32, mut alpha: i32, beta: i32, ply: usize, pv_node: bool, allow_null: bool) -> (i32, Option<Move>) {
        // Alpha has no null-move, so the flag is unused there.
        #[cfg(not(feature = "qsearch"))]
        let _ = allow_null;
        // Repetition / draw shortcuts (not at the root).
        if ply > 0 {
            if board.halfmove >= 100 {
                return (0, None);
            }
            if is_insufficient_material(board) {
                return (0, None);
            }
            if self.is_repetition(board) {
                return (0, None);
            }
        }
        // Hard ply cap, just past the draw shortcuts (a horizon, not a
        // cutoff): check extensions can keep `depth` high in long checking
        // sequences, so bound `ply` explicitly. Without this guard an
        // extreme line would index past the killer/PV arrays. Like
        // qsearch, fall back to the static evaluation — mate scores are
        // therefore approximate at the very end of the line, which is
        // unreachable in real games.
        if ply >= MAX_PLY - 1 {
            return (evaluate_with(board, &self.params), None);
        }
        if self.should_stop() {
            return (0, None);
        }
        self.nodes += 1;

        let in_check = board.is_in_check(board.side);
        // Static evaluation is lazy and cached for the whole node: razoring,
        // reverse futility, null-move and per-move futility all ask for the
        // same position, and depth-1 nodes used to re-evaluate once per
        // quiet move. The board is untouched until a move is made, so one
        // value is valid throughout. This is node-identical, only faster.
        let mut static_eval: Option<i32> = None;
        // Razoring (Gamma only): at very low depth, a position whose static
        // evaluation is hopeless against beta has no future in the full
        // search — drop straight into qsearch. `pv_node` is excluded so the
        // principal variation is never truncated.
        #[cfg(feature = "qsearch")]
        if depth <= 2 && !pv_node && !in_check {
            let sp = *static_eval.get_or_insert_with(|| evaluate_with(board, &self.params));
            if sp + 250 < beta {
                return (self.qsearch(board, alpha, beta, ply, 0), None);
            }
        }
        // Check extension (Gamma-only via the `qsearch` feature): checks
        // deserve extra depth because tactics hide there, but an uncapped
        // extension explodes in checking sequences (10 checks = +10 depth).
        // Cap at 12 so the search stays focused without magic depth
        // blow-ups.
        #[cfg(feature = "qsearch")]
        let depth = if in_check && depth < 12 { depth + 1 } else { depth };

        if depth == 0 {
            // Quiescence is shared core by default (trainable off for A/Bs):
            // both variants must resolve captures/evasions at the horizon or
            // they blunder tactically.
            if self.sparams.qsearch != 0 {
                let s = self.qsearch(board, alpha, beta, ply, 0);
                return (s, None);
            }
            return (evaluate_with(board, &self.params), None);
        }

        // Reverse futility (trainable): a non-PV node whose static eval
        // beats beta by a depth-scaled margin cannot realistically fall back
        // below beta, so cut on the static score.
        if !pv_node && !in_check && self.sparams.rfp_margin > 0
            && depth <= self.sparams.rfp_max_depth
        {
            let sp = *static_eval.get_or_insert_with(|| evaluate_with(board, &self.params));
            if sp - self.sparams.rfp_margin * depth >= beta {
                return (sp, None);
            }
        }

        // One lock covers both the ordering move and the cutoff entry.
        let (mut tt_move, tt_entry) = self.tt.probe_both(board.hash, depth);
        if !pv_node {
            if let Some(e) = tt_entry {
                let s = adjust_from_tt(e.score, ply);
                match e.flag {
                    Bound::Exact => return (s, e.best),
                    Bound::Lower if s >= beta => return (s, e.best),
                    Bound::Upper if s <= alpha => return (s, e.best),
                    _ => {}
                }
            }
        }

        // Null-move pruning (Gamma only, gated by the `qsearch` feature —
        // Gamma's feature set): skip a whole move and see if we still
        // fail low. Alpha has no null-move — it searches every move in
        // full, which is both slower and weaker (by design).
        #[cfg(feature = "qsearch")]
        if allow_null && !pv_node && !in_check && depth >= 3 && has_non_pawn_material(board) {
            let eval = *static_eval.get_or_insert_with(|| evaluate_with(board, &self.params));
            if eval >= beta {
                let saved_side = board.side;
                let saved_ep = board.ep_square;
                let saved_hash = board.hash;
                let saved_half = board.halfmove;
                board.side = board.side.other();
                board.ep_square = None;
                board.halfmove += 1;
                board.hash = board.null_move_hash();
                self.path_hashes.push(board.hash);
                let r = self.sparams.null_r_base
                    + depth / self.sparams.null_r_div.max(1);
                let (score, _) = self.negamax(board, depth - 1 - r, -beta, -beta + 1, ply + 1, false, false);
                self.path_hashes.pop();
                board.side = saved_side;
                board.ep_square = saved_ep;
                board.hash = saved_hash;
                board.halfmove = saved_half;
                if !self.should_stop() && -score >= beta {
                    return (-score.max(beta), None);
                }
            }
        }

        // Internal iterative deepening (trainable, off by default): at a PV
        // node without a TT move, a shallow search finds a move worth trying
        // first, which usually pays for itself in the full-width search.
        if self.sparams.iid_min_depth > 0
            && tt_move.is_none()
            && pv_node
            && depth > 2
            && depth >= self.sparams.iid_min_depth
        {
            let (_, m) = self.negamax(board, (depth - 2).max(1), alpha, beta, ply, pv_node, allow_null);
            if m.is_some() {
                tt_move = m;
            }
        }

        // Move generation + ordering (reuse the check test above).
        let mut moves = board.generate_moves_with_check(false, in_check);
        self.order_moves(board, &mut moves, tt_move, ply);
        let us = board.side;
        let mut best = -INF;
        let mut best_move: Option<Move> = None;
        let mut legal_moves = 0i32;
        let alpha_orig = alpha;
        self.pv_len[ply] = ply;
        // Quiet moves that failed to raise alpha, for the quiet-failure
        // reduction knob. Gamma-only, like LMR itself.
        #[cfg(feature = "qsearch")]
        let mut quiet_fails = 0i32;

        // ProbCut gate (trainable, off by default). It is a selective
        // cutoff, so it only runs away from PV nodes, checks and mate
        // windows, and not in the first few plies where a shallow proof
        // would shape the whole line.
        let probcut_ok = self.sparams.probcut_margin > 0
            && !pv_node
            && !in_check
            && ply >= 3
            && depth >= self.sparams.probcut_min_depth
            && beta < MATE - 1000;

        self.path_hashes.push(board.hash);
        for i in 0..moves.len() {
            let m = moves[i];
            let is_capture = m.is_capture || m.promo.is_some();
            // Killer test is used by the pruning and LMR below.
            let is_killer = self.is_killer(m, ply) && !is_capture;

            // Late-move pruning and futility (trainable). Both only skip
            // quiet moves after at least one legal move has been searched,
            // so a node can never be mistaken for stalemate.
            if !pv_node && !in_check && !is_capture && !is_killer && i > 0 && legal_moves > 0 {
                if self.sparams.lmp_base > 0
                    && depth <= self.sparams.lmp_max_depth
                    && i as i32 >= self.sparams.lmp_base + depth * depth
                {
                    continue;
                }
                if self.sparams.futility_margin > 0 && depth == 1 {
                    let se = *static_eval.get_or_insert_with(|| evaluate_with(board, &self.params));
                    if se + self.sparams.futility_margin <= alpha {
                        continue;
                    }
                }
            }

            // SEE reads the mover on `from`, so it must be computed before
            // the move is made. Only the optional ProbCut/capture-LMR paths
            // need it; both are off by default, so the baseline pays nothing.
            let m_see = if is_capture && (probcut_ok || self.sparams.lmr_captures > 0) {
                see(board, &m)
            } else {
                0
            };

            board.make_move(&m);
            if board.is_in_check(us) {
                board.unmake_move();
                continue; // illegal: leaves own king in check
            }
            legal_moves += 1;

            // ProbCut: a capture that wins material statically is a good
            // candidate to prove a cutoff above `beta + margin` without
            // searching the full move loop. A failed probe costs one
            // reduced search; a successful one prunes the whole subtree.
            // The TT move is always searched normally.
            if probcut_ok && is_capture && Some(m) != tt_move && m_see >= 0 {
                let r = self.sparams.probcut_r.max(1);
                let beta_cut = beta + self.sparams.probcut_margin;
                if depth > r && beta_cut < MATE - 1000 {
                    let (s, _) = self.negamax(
                        board,
                        depth - r,
                        -beta_cut,
                        -beta_cut + 1,
                        ply + 1,
                        false,
                        true,
                    );
                    let score = -s;
                    if !self.should_stop() && score >= beta_cut {
                        board.unmake_move();
                        self.path_hashes.pop();
                        return (score, Some(m));
                    }
                }
            }

            let new_depth = depth - 1;
            let mut score: i32;
            if i == 0 {
                let (s, _) = self.negamax(board, new_depth, -beta, -alpha, ply + 1, pv_node, true);
                score = -s;
            } else {
                // Late-move reduction (Gamma only): late quiet moves are
                // probed shallower first and re-searched when they beat
                // alpha. `lmr_div=0` keeps the historical fixed one-ply
                // reduction; a positive divisor selects the log formula.
                // The first two moves and killers are never reduced, and
                // Alpha never reduces at all.
                #[cfg(feature = "qsearch")]
                let reduce = if i > 1
                    && !is_killer
                    && new_depth >= self.sparams.lmr_min_depth.max(1)
                {
                    if is_capture {
                        // Losing captures are tactically suspect but still
                        // worth a reduced probe; the full-depth verification
                        // below confirms any cutoff. 0 = off, and PV nodes
                        // stay full depth unless `lmr_pv` is on.
                        if self.sparams.lmr_captures > 0
                            && (!pv_node || self.sparams.lmr_pv > 0)
                            && m_see < 0
                        {
                            self.sparams.lmr_captures.min(new_depth - 1).max(0)
                        } else {
                            0
                        }
                    } else if pv_node && self.sparams.lmr_pv <= 0 {
                        0
                    } else if self.sparams.lmr_div > 0 {
                        // Log formula, clamped so the probe never drops
                        // below depth 1. At new_depth 1 the child is
                        // already a qsearch, so there is nothing to reduce.
                        if new_depth < 2 {
                            0
                        } else {
                            let base = self.sparams.lmr_base as f64 / 100.0;
                            let div = (self.sparams.lmr_div as f64 / 100.0).max(0.01);
                            let r = base
                                + (new_depth as f64).ln() * (i as f64).ln() / div;
                            let mut r = (r as i32).clamp(1, new_depth - 1);
                            if self.sparams.lmr_fail_threshold > 0
                                && quiet_fails >= self.sparams.lmr_fail_threshold
                            {
                                r = (r + 1).min(new_depth - 1);
                            }
                            r
                        }
                    } else {
                        // Historical fixed one-ply reduction. It may drop a
                        // depth-1 child into qsearch; that is the trained
                        // baseline and stays node-identical.
                        let mut r = 1;
                        if self.sparams.lmr_fail_threshold > 0
                            && quiet_fails >= self.sparams.lmr_fail_threshold
                        {
                            r = (r + 1).min(new_depth - 1).max(r);
                        }
                        r
                    }
                } else {
                    0
                };
                #[cfg(not(feature = "qsearch"))]
                let reduce = 0;
                let reduced = (new_depth - reduce).max(0);
                let (reduced_score, _) =
                    self.negamax(board, reduced, -alpha - 1, -alpha, ply + 1, false, true);
                score = -reduced_score;
                // Standard LMR verification: a reduced probe that beats
                // alpha is re-searched at full depth. At a zero-window cut
                // node (beta = alpha+1) `score < beta` can never hold, so
                // the old in-window test skipped every such re-search;
                // `lmr_research=1` (the default since 2026-11-18) restores
                // it. 0 keeps the older in-window-only rule for A/Bs.
                let research = if self.sparams.lmr_research > 0 {
                    score > alpha && (reduce > 0 || score < beta)
                } else {
                    score > alpha && score < beta
                };
                if research {
                    let (full_score, _) =
                        self.negamax(board, new_depth, -beta, -alpha, ply + 1, pv_node, true);
                    score = -full_score;
                }
            }

            board.unmake_move();
            #[cfg(feature = "qsearch")]
            if !is_capture && legal_moves > 1 && score <= alpha {
                quiet_fails += 1;
            }
            if self.should_stop() {
                break;
            }

            if score > best {
                best = score;
                best_move = Some(m);
                // Update PV table: root PV = best move + child's PV.
                let child_len = self.pv_len[ply + 1];
                self.pv_table[ply][ply] = Some(m);
                // Copy the child's PV line after our move.
                let copy_len = child_len.saturating_sub(ply + 1);
                for k in 0..copy_len {
                    let src = ply + 1 + k;
                    let dst = ply + 1 + k;
                    if dst < MAX_PLY {
                        self.pv_table[ply][dst] = self.pv_table[ply + 1][src];
                    }
                }
                self.pv_len[ply] = child_len.max(ply + 1);
            }

            if score > alpha {
                alpha = score;
                if alpha >= beta {
                    // Beta cutoff: update killers / history for quiet moves.
                    // (History gravity was tried and measured neutral:
                    // 200 ms 20 g 4–12–4 ~1853 vs 7–12–1 ~1911, bench
                    // 60785 unchanged. Plain accumulation is kept.)
                    if !is_capture {
                        self.store_killer(m, ply);
                        let s = self.slot(board, m.from);
                        self.history[s][m.to as usize] += depth * depth;
                    }
                    break;
                }
            }
        }
        self.path_hashes.pop();

        if legal_moves == 0 {
            if in_check {
                return (-MATE + ply as i32, None);
            } else {
                return (0, None); // stalemate
            }
        }

        // TT store. A stopped search must not store: its `best` may be a
        // partial result from an incomplete move list, and an Upper/Lower
        // bound from an aborted node is wrong for the stored depth. Such
        // entries survive into the next move's search and can cut real
        // tactics, so the check is before the store, not after.
        if self.should_stop() {
            return (best, best_move);
        }
        let flag = if best <= alpha_orig {
            Bound::Upper
        } else if best >= beta {
            Bound::Lower
        } else {
            Bound::Exact
        };
        self.tt.store(board.hash, depth, flag, adjust_to_tt(best, ply), best_move);

        (best, best_move)
    }

    /// Quiescence search (shared core). Resolves captures and, when in
    /// check, all evasions. Gamma additionally drops negative-SEE captures
    /// (`qsearch` feature); Alpha keeps the full capture set.
    fn qsearch(&mut self, board: &mut Board, mut alpha: i32, beta: i32, ply: usize, qply: usize) -> i32 {
        self.nodes += 1;
        if ply >= MAX_PLY - 1 {
            return evaluate_with(board, &self.params);
        }
        // Optional cap on capture plies: engines without null-move/LMR can
        // spend their whole clock here, so the training harness can bound it.
        if self.sparams.qs_max_plies > 0 && qply >= self.sparams.qs_max_plies as usize {
            return evaluate_with(board, &self.params);
        }
        if self.should_stop() {
            return evaluate_with(board, &self.params);
        }
        let in_check = board.is_in_check(board.side);
        // Stand-pat is only meaningful outside check: in check the side to
        // move may not "pass", so its value is never read there. Skipping
        // the evaluation on check nodes removes a whole eval per qnode.
        let stand_pat = if in_check {
            -INF
        } else {
            evaluate_with(board, &self.params)
        };
        if !in_check {
            if stand_pat >= beta {
                return beta;
            }
            if stand_pat > alpha {
                alpha = stand_pat;
            }
        }

        let mut moves = board.generate_moves_with_check(true, in_check);
        // Order captures by MVV-LVA (cheap: sorts a small list).
        self.order_captures(board, &mut moves);
        let us = board.side;
        let mut legal = 0;
        self.path_hashes.push(board.hash);
        for i in 0..moves.len() {
            let m = moves.as_slice()[i];
            // Delta pruning (pre-make): skip if even winning the victim
            // cannot reach alpha. Silent promotions are tactical too, and
            // they are worth far more than the pawn push's nominal 0, so
            // count the promotion gain (queen-pawn material swing) or a
            // winning promotion would be pruned at a high alpha.
            let victim = captured_value_from_board(board, &m);
            let promo_gain = if m.promo.is_some() { 800 } else { 0 };
            if !in_check && stand_pat + victim + promo_gain + self.sparams.qs_delta_margin < alpha {
                continue;
            }
            // SEE pruning: skip captures that lose material in a static
            // exchange (SEE < 0) when not in check. In check every evasion
            // must be tried, so no pruning there. This cuts hopeless qnodes
            // without deep search — tactics that win material (SEE >= 0)
            // are always searched. Shared by both variants: it is what keeps
            // quiescence cheap enough to pay for its depth cost.
            if !in_check && see(board, &m) < -self.sparams.qs_see_margin.max(0) {
                continue;
            }
            board.make_move(&m);
            if board.is_in_check(us) {
                board.unmake_move();
                continue;
            }
            legal += 1;
            let score = -self.qsearch(board, -beta, -alpha, ply + 1, qply + 1);
            board.unmake_move();
            if self.should_stop() {
                break;
            }
            if score >= beta {
                self.path_hashes.pop();
                return beta;
            }
            if score > alpha {
                alpha = score;
            }
        }
        self.path_hashes.pop();

        if legal == 0 && in_check {
            return -MATE + ply as i32; // checkmated in qsearch
        }
        alpha
    }

    fn is_repetition(&self, board: &Board) -> bool {
        // Only positions after the last irreversible move (pawn push or
        // capture) can repeat — earlier positions had more material or a
        // pawn further back and can never be reached again. The halfmove
        // clock counts plies since that move, so scan only that suffix.
        // (Before, the full game history — up to ~300 hashes — was scanned
        // at every node; this is one slice instead of three linear scans.)
        let h = board.hash;
        let back = (board.halfmove as usize).min(self.game_hashes.len());
        let start = self.game_hashes.len() - back;
        if self.game_hashes[start..].contains(&h) {
            return true;
        }
        let back = (board.halfmove as usize).min(self.path_hashes.len());
        let start = self.path_hashes.len() - back;
        if self.path_hashes[start..].contains(&h) {
            return true;
        }
        false
    }

    fn is_killer(&self, m: Move, ply: usize) -> bool {
        (self.killers[ply * 2] == Some(m)) || (self.killers[ply * 2 + 1] == Some(m))
    }

    fn store_killer(&mut self, m: Move, ply: usize) {
        let i = ply * 2;
        if self.killers[i] != Some(m) {
            self.killers[i + 1] = self.killers[i];
            self.killers[i] = Some(m);
        }
    }

    /// Sort `moves` by descending `score(m)` without allocating. Move lists
    /// are short (tens), so insertion sort is plenty.
    fn order_by(&self, moves: &mut MoveList, score: impl Fn(&Move) -> i32) {
        let n = moves.len();
        if n <= 1 {
            return;
        }
        let mut buf = [std::mem::MaybeUninit::<i32>::uninit(); MAX_MOVES];
        for (i, m) in moves.as_slice().iter().enumerate() {
            buf[i].write(score(m));
        }
        // SAFETY: exactly `n` scores were written above.
        let scores = unsafe {
            std::slice::from_raw_parts_mut(buf.as_mut_ptr().cast::<i32>(), n)
        };
        sort_moves_by_score(moves, scores);
    }

    fn order_moves(&self, board: &Board, moves: &mut MoveList, tt_move: Option<Move>, ply: usize) {
        self.order_by(moves, |m| self.move_score(board, m, tt_move, ply));
    }

    /// Ordering score for one move (higher = tried earlier).
    fn move_score(&self, board: &Board, m: &Move, tt_move: Option<Move>, ply: usize) -> i32 {
        if Some(*m) == tt_move {
            return 10_000_000;
        }
        if m.is_capture || m.promo.is_some() {
            // SEE ordering tried and measured negative at 500 ms
            // (nodes rose); MVV-LVA alone is kept.
            let mut s = 9_000_000 + capture_score(board, m);
            if m.promo.is_some() {
                s += 1000;
            }
            return s;
        }
        if self.is_killer(*m, ply) {
            return 7_000_000;
        }
        self.history[self.slot(board, m.from)][m.to as usize]
    }

    fn order_captures(&self, board: &Board, moves: &mut MoveList) {
        self.order_by(moves, |m| capture_score(board, m));
    }
}

/// MVV-LVA score for captures/promotions (used in move ordering and
/// qsearch capture ordering). Higher = better.
fn capture_score(board: &Board, m: &Move) -> i32 {
    let victim = m
        .promo
        .map(|_| 900)
        .unwrap_or_else(|| captured_value_from_board(board, m));
    let attacker = board
        .piece_on(m.from)
        .map(|p| piece_value(p.kind))
        .unwrap_or(100);
    victim * 10 - attacker / 10
}

/// Insertion-sort `moves` by descending `scores`. Move lists are short
/// (tens), so insertion sort is plenty and keeps moves and scores
/// together without a secondary buffer.
fn sort_moves_by_score(moves: &mut crate::types::MoveList, scores: &mut [i32]) {
    debug_assert_eq!(scores.len(), moves.len());
    for i in 1..moves.len() {
        let mut j = i;
        while j > 0 && scores[j] > scores[j - 1] {
            scores.swap(j, j - 1);
            moves.as_mut_slice().swap(j, j - 1);
            j -= 1;
        }
    }
}

fn adjust_from_tt(score: i32, ply: usize) -> i32 {
    if score > MATE - 1000 {
        score - ply as i32
    } else if score < -(MATE - 1000) {
        score + ply as i32
    } else {
        score
    }
}

fn adjust_to_tt(score: i32, ply: usize) -> i32 {
    if score > MATE - 1000 {
        score + ply as i32
    } else if score < -(MATE - 1000) {
        score - ply as i32
    } else {
        score
    }
}

/// Moves to mate for a mate score, or `None` for a normal score.
/// Positive = the side to move mates, negative = it is being mated.
/// Shared by the UCI info line and the FFI JSON so both report the same
/// number (and the same sign).
pub fn mate_in(score: i32) -> Option<i32> {
    if score > MATE - 1000 {
        Some((MATE - score + 1) / 2)
    } else if score < -(MATE - 1000) {
        Some(-((MATE + score + 1) / 2))
    } else {
        None
    }
}

/// Centipawn value of a piece kind, used for move ordering, SEE and
/// qsearch delta pruning (not for evaluation, which has its own tables).
fn piece_value(k: PieceType) -> i32 {
    match k {
        PieceType::Pawn => 100,
        PieceType::Knight => 320,
        PieceType::Bishop => 330,
        PieceType::Rook => 500,
        PieceType::Queen => 900,
        PieceType::King => 20000,
    }
}

/// Read the victim's value from the board (pre-make). Used for ordering
/// and qsearch delta pruning.
fn captured_value_from_board(board: &Board, m: &Move) -> i32 {
    if m.is_en_passant {
        return 100;
    }
    match board.piece_on(m.to) {
        Some(p) => piece_value(p.kind),
        None => 0,
    }
}

/// Least-valuable attacker of `to` for `side` given `occ` (and copied pieces).
/// Returns `(square, value)` or `None`. King is considered last (value large).
/// Used by SEE below; written as an explicit piece-by-piece probe so the
/// exchange can be verified by eye.
fn least_attacker(board: &Board, side: Color, to: u8, occ: u64) -> Option<(u8, i32)> {
    let base = side.idx() * 6;
    let pieces = &board.pieces;
    // Pawn
    let bb = pawn_attackers_to(to, side) & pieces[base];
    if bb != 0 { return Some((pop_lsb(bb).0, 100)); }
    // Knight
    let bb = knight_attacks(to) & pieces[base + 1];
    if bb != 0 { return Some((pop_lsb(bb).0, 320)); }
    // Bishop / queen diagonal
    let bb = bishop_attacks(to, occ) & (pieces[base + 2] | pieces[base + 4]);
    if bb != 0 {
        let sq = pop_lsb(bb).0;
        let v = if pieces[base + 2] & bit(sq) != 0 { 330 } else { 900 };
        return Some((sq, v));
    }
    // Rook / queen straight
    let bb = rook_attacks(to, occ) & (pieces[base + 3] | pieces[base + 4]);
    if bb != 0 {
        let sq = pop_lsb(bb).0;
        let v = if pieces[base + 3] & bit(sq) != 0 { 500 } else { 900 };
        return Some((sq, v));
    }
    // King
    let bb = king_attacks(to) & pieces[base + 5];
    if bb != 0 { return Some((pop_lsb(bb).0, 20000)); }
    None
}

/// Static Exchange Evaluation: net material gain for the mover if the
/// exchange on `m.to` resolves optimally (pure material). >=0 wins,
/// <0 loses. Used to prune hopeless captures in qsearch.
fn see(board: &Board, m: &Move) -> i32 {
    if !m.is_capture && m.promo.is_none() { return 0; }
    let attacker = match board.piece_on(m.from) {
        Some(p) => p,
        None => {
            // The mover must still be on `from`: SEE is a pre-move test.
            debug_assert!(false, "see() was called after make_move");
            return 0;
        }
    };
    let victim = if m.is_en_passant {
        100
    } else {
        board.piece_on(m.to).map(|p| piece_value(p.kind)).unwrap_or(0)
    };
    let promo_bonus = m.promo.map(|p| piece_value(p) - 100).unwrap_or(0);
    let mut gain = [0i32; 33];
    gain[0] = victim + promo_bonus;
    let mut occ = board.occupied ^ bit(m.from);
    if m.is_en_passant {
        let cap = if attacker.color == Color::White { m.to - 8 } else { m.to + 8 };
        occ &= !bit(cap);
    }
    let mut attacker_val = match m.promo {
        Some(p) => piece_value(p),
        None => piece_value(attacker.kind),
    };
    // Fast path: if the victim alone is worth at least the moving piece,
    // the capture cannot lose material in the static exchange (the mover
    // can stop after it, worst case trading attacker for victim at a
    // profit). Only the sign is used (SEE pruning), so return 0 (non-
    // negative) instead of scanning every recapture line.
    if gain[0] >= attacker_val {
        return 0;
    }
    let mut side = attacker.color.other();
    let mut d = 0usize;
    while let Some((sq, val)) = least_attacker(board, side, m.to, occ) {
        d += 1;
        if d >= gain.len() { d -= 1; break; }
        gain[d] = attacker_val - gain[d - 1];
        occ ^= bit(sq);
        attacker_val = val;
        side = side.other();
    }
    let mut s = gain[0];
    for i in (1..=d).rev() {
        s = (-s).max(gain[i]);
    }
    s
}

/// Only used by Gamma's null-move pruning.
#[cfg(feature = "qsearch")]
fn has_non_pawn_material(board: &Board) -> bool {
    // True when either side has a knight, bishop, rook or queen.
    let w = board.pieces[1] | board.pieces[2] | board.pieces[3] | board.pieces[4];
    let b = board.pieces[7] | board.pieces[8] | board.pieces[9] | board.pieces[10];
    (w | b) != 0
}

pub fn is_insufficient_material(board: &Board) -> bool {
    // Any pawn, rook or queen can mate → sufficient. Pawns are checked
    // first because they are by far the most common reason to bail out.
    if board.pieces[0] | board.pieces[6] != 0 {
        return false;
    }
    if board.pieces[3] | board.pieces[4] | board.pieces[9] | board.pieces[10] != 0 {
        return false;
    }
    // Count minors via popcounts: this runs at every search node, so it
    // must not allocate (the old version collected a Vec of minor pieces
    // whenever no pawns, rooks or queens were left).
    let white_knights = board.pieces[1];
    let white_bishops = board.pieces[2];
    let black_knights = board.pieces[7];
    let black_bishops = board.pieces[8];
    let minors = white_knights | white_bishops | black_knights | black_bishops;
    match popcount(minors) {
        // K vs K, or K+N/B vs K.
        0 | 1 => true,
        2 => {
            // K+B vs K+B with both bishops on the same square colour is a
            // dead draw. A knight, or two bishops of the same colour, can
            // still mate.
            if white_knights | black_knights != 0 {
                return false;
            }
            if popcount(white_bishops) != 1 || popcount(black_bishops) != 1 {
                return false;
            }
            // Squares with (rank + file) even, the parity the old
            // square-list code compared.
            const EVEN_PARITY: u64 = 0xAA55_AA55_AA55_AA55;
            let bishops = white_bishops | black_bishops;
            bishops & EVEN_PARITY == 0 || bishops & !EVEN_PARITY == 0
        }
        _ => false,
    }
}

/// Convenience: search a position synchronously (no threads, no info).
pub fn search_position(board: &mut Board, game_history: &[u64], limits: &SearchLimits, tt: &TranspositionTable) -> SearchInfo {
    search_position_with_params(board, game_history, limits, tt, EvalParams::DEFAULT)
}

/// Same, with explicit evaluation weights (used by the training harness).
pub fn search_position_with_params(
    board: &mut Board,
    game_history: &[u64],
    limits: &SearchLimits,
    tt: &TranspositionTable,
    params: EvalParams,
) -> SearchInfo {
    search_position_with_settings(board, game_history, limits, tt, params, SearchParams::DEFAULT)
}

/// Full training hook: evaluation weights plus search prunings.
pub fn search_position_with_settings(
    board: &mut Board,
    game_history: &[u64],
    limits: &SearchLimits,
    tt: &TranspositionTable,
    params: EvalParams,
    sparams: SearchParams,
) -> SearchInfo {
    let mut s = Searcher::with_settings(tt.clone(), params, sparams);
    s.search(board, game_history, limits, None, &|_| {})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::Board;
    use crate::tt::TranspositionTable;

    fn search_fen(fen: &str, depth: i32) -> SearchInfo {
        let mut b = Board::from_fen(fen).unwrap();
        let tt = TranspositionTable::new(8);
        search_position(&mut b, &[], &SearchLimits::fixed_depth(depth), &tt)
    }

    fn search_fen_with(fen: &str, depth: i32, spec: &str) -> SearchInfo {
        let mut b = Board::from_fen(fen).unwrap();
        let tt = TranspositionTable::new(8);
        let sp = SearchParams::from_spec(spec).unwrap();
        search_position_with_settings(
            &mut b,
            &[],
            &SearchLimits::fixed_depth(depth),
            &tt,
            EvalParams::DEFAULT,
            sp,
        )
    }

    #[test]
    fn mate_in_one() {
        // White to play and mate: Qxf7# (from the Scholar's mate pattern).
        let info = search_fen("r1bqkbnr/pppp1ppp/2n5/4p2Q/2B1P3/8/PPPP1PPP/RNB1K1NR w KQkq - 0 1", 3);
        let best = info.best_move.expect("a best move");
        assert_eq!(best.to_uci(), "h5f7", "expected Qxf7# got {} score {}", best.to_uci(), info.score);
        assert!(info.score > MATE - 100, "score {} should be mate", info.score);
        // Library/FFI callers need the PV, not just the move.
        assert_eq!(info.pv.first().map(|m| m.to_uci()), Some("h5f7".to_string()));
    }

    #[test]
    fn finds_best_capture() {
        // White knight on c7 wins the black queen on a8 (Nxa8).
        let mut b = Board::from_fen("q7/2N5/8/8/8/8/8/K6k w - - 0 1").unwrap();
        let tt = TranspositionTable::new(8);
        let info = search_position(&mut b, &[], &SearchLimits::fixed_depth(2), &tt);
        let best = info.best_move.unwrap();
        assert_eq!(best.to_uci(), "c7a8", "expected Nxa8 (win queen), got {}", best.to_uci());
    }

    #[test]
    fn optional_search_knobs_keep_mate_and_legality() {
        // ProbCut is off by default; switching it on must still find
        // Qxf7# and return a legal root move.
        let info = search_fen_with(
            "r1bqkbnr/pppp1ppp/2n5/4p2Q/2B1P3/8/PPPP1PPP/RNB1K1NR w KQkq - 0 1",
            5,
            "probcut_margin=200",
        );
        assert_eq!(info.best_move.map(|m| m.to_uci()), Some("h5f7".to_string()));
        assert!(info.score > MATE - 100, "score {} should be mate", info.score);

        let fen = "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1";
        let mut b = Board::from_fen(fen).unwrap();
        let legal = b.generate_legal_moves();
        let info = search_fen_with(fen, 6, "probcut_margin=150");
        let best = info.best_move.expect("root move");
        assert!(legal.iter().any(|m| *m == best), "illegal best move {}", best.to_uci());
    }

    #[test]
    fn stop_flag_halts_search() {
        use std::sync::{Arc, atomic::AtomicBool};
        let mut b = Board::startpos();
        let tt = TranspositionTable::new(8);
        let stop = Arc::new(AtomicBool::new(true));
        let mut s = Searcher::new(tt);
        let info = s.search(&mut b, &[], &SearchLimits::fixed_depth(20), Some(stop), &|_| {});
        // Search must return promptly despite depth 20 (flag already set),
        // and still supply a legal move for a caller that needs one.
        assert!(info.nodes < 100_000);
        assert!(info.best_move.is_some());
    }

    #[test]
    fn startpos_returns_a_legal_move() {
        let mut b = Board::startpos();
        let tt = TranspositionTable::new(8);
        let info = search_position(&mut b, &[], &SearchLimits::fixed_depth(2), &tt);
        let best = info.best_move.expect("root move");
        let legal = b.generate_legal_moves();
        assert!(legal.iter().any(|m| *m == best));
    }

    #[test]
    fn search_param_spec_roundtrip() {
        let spec = SearchParams::DEFAULT.to_spec();
        assert_eq!(SearchParams::from_spec(&spec).unwrap(), SearchParams::DEFAULT);
        assert!(SearchParams::from_spec("nope=1").is_err());
        assert!(SearchParams::from_spec("lmr_div=x").is_err());
        let p = SearchParams::from_spec("qsearch=0; lmp_base=7").unwrap();
        assert_eq!(p.qsearch, 0);
        assert_eq!(p.lmp_base, 7);
    }

    #[test]
    fn mate_in_reports_both_signs() {
        assert_eq!(mate_in(MATE - 3), Some(2));
        assert_eq!(mate_in(-(MATE - 3)), Some(-2));
        assert_eq!(mate_in(0), None);
        assert_eq!(mate_in(500), None);
    }

    #[test]
    fn insufficient_material_cases() {
        let check = |fen: &str| is_insufficient_material(&Board::from_fen(fen).unwrap());
        // Dead draws: K vs K, K+N vs K, K+B vs K.
        assert!(check("8/8/8/4k3/8/8/8/4K3 w - - 0 1"));
        assert!(check("8/8/8/4k3/8/8/8/3NK3 w - - 0 1"));
        assert!(check("8/8/8/4k3/8/8/8/3BK3 w - - 0 1"));
        // K+B vs K+B on the same square colour is a dead draw...
        assert!(check("5b2/8/8/4k3/8/8/8/2B1K3 w - - 0 1"));
        // ...but opposite-coloured bishops can still mate.
        assert!(!check("6b1/8/8/4k3/8/8/8/2B1K3 w - - 0 1"));
        // Two knights cannot force mate, but the position is not in the
        // engine's dead-draw list.
        assert!(!check("5n2/8/8/4k3/8/8/8/3NK3 w - - 0 1"));
        // Two bishops on one side are not the K+B vs K+B draw.
        assert!(!check("8/8/8/4k3/8/8/8/2B1K2B w - - 0 1"));
        // Any pawn, rook or queen is sufficient material.
        assert!(!check("8/8/8/4k3/8/8/4P3/4K3 w - - 0 1"));
        assert!(!check("8/8/8/4k3/8/8/8/R3K3 w - - 0 1"));
    }
}
