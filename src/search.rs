//! Search: negamax alpha-beta with iterative deepening.
//!
//! The Gamma build adds quiescence search (feature `qsearch`), null-move
//! pruning, late-move reduction, advanced evaluation and optional
//! multithreading (feature `threads`); Alpha builds skip all of those and
//! evaluate directly at the horizon. What remains shared — transposition
//! table, move ordering, killers/history and check extension — is the
//! common core, because Alpha *is* Gamma with features removed.
//!
//! Scores are centipawns from the side-to-move's perspective.
//! `MATE` = 32000; mate in `n` is `MATE - n`.

use crate::board::Board;
#[cfg(feature = "qsearch")]
use crate::bitboard::{bishop_attacks, bit, king_attacks, knight_attacks, pawn_attackers_to, pop_lsb, rook_attacks};
use crate::bitboard::popcount;
use crate::eval::evaluate;
use crate::tt::{Bound, TranspositionTable};
#[cfg(feature = "qsearch")]
use crate::types::Color;
use crate::types::{Move, PieceType};
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
        Searcher {
            tt,
            nodes: 0,
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
                let mut pv_str = String::new();
                for m in pv_moves.iter() {
                    if !pv_str.is_empty() {
                        pv_str.push(' ');
                    }
                    pv_str.push_str(&m.to_uci());
                }
                let score_str = if best_score.abs() > MATE - 1000 {
                    let mate_in = if best_score > 0 { (MATE - best_score + 1) / 2 } else { (MATE + best_score + 1) / 2 };
                    format!("mate {}", mate_in)
                } else {
                    format!("cp {}", best_score)
                };
                let info = if pv_str.is_empty() {
                    format!("info depth {} score {} nodes {} nps {}", completed_depth, score_str, self.nodes, nps)
                } else {
                    format!("info depth {} score {} nodes {} nps {} pv {}", completed_depth, score_str, self.nodes, nps, pv_str)
                };
                on_info(info);
            }
            if limits.infinite {
                continue;
            }
        }

        let time_ms = start.elapsed().as_millis() as u64;
        let pv = self.extract_pv();
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
        let mut delta = 50;
        let (mut alpha, mut beta) = if depth == 1 {
            (-INF, INF)
        } else {
            (prev_score - delta, prev_score + delta)
        };
        for attempt in 0..5 {
            let (score, mv) = self.negamax(board, depth, alpha, beta, 0, true, true);
            if self.should_stop() || attempt == 4 {
                if self.should_stop() {
                    return (prev_score, mv);
                }
                // Last attempt used the full window; its score is final.
                if score > alpha && score < beta {
                    return (score, mv);
                }
            }
            if score > alpha && score < beta {
                return (score, mv);
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
            return (evaluate(board), None);
        }
        if self.should_stop() {
            return (0, None);
        }
        self.nodes += 1;

        let in_check = board.is_in_check(board.side);
        // Razoring (Gamma only): at very low depth, a position whose static
        // evaluation is hopeless against beta has no future in the full
        // search — drop straight into qsearch. `pv_node` is excluded so the
        // principal variation is never truncated.
        #[cfg(feature = "qsearch")]
        if depth <= 2 && !pv_node && !in_check {
            let sp = evaluate(board);
            if sp + 250 < beta {
                return (self.qsearch(board, alpha, beta, ply), None);
            }
        }
        // Check extension (capped, Gamma-only via the `qsearch` feature):
        // checks deserve extra depth because tactics hide there, but an
        // uncapped extension explodes in checking sequences (10 checks =
        // +10 depth). Cap at 12 so the search stays focused without magic
        // depth blow-ups. Alpha searches checks at the same depth, which
        // keeps the lightweight variant inside its rating band.
        #[cfg(feature = "qsearch")]
        let depth = if in_check && depth < 12 { depth + 1 } else { depth };

        if depth == 0 {
            #[cfg(feature = "qsearch")]
            {
                let s = self.qsearch(board, alpha, beta, ply);
                return (s, None);
            }
            #[cfg(not(feature = "qsearch"))]
            {
                let s = evaluate(board);
                return (s, None);
            }
        }

        // TT probe share the same single lock for both cases: one lock per
        // node. PV nodes keep the depth-sufficient entry so the singular
        // extension can use its bound (used only for the bound test).
        let (tt_move, tt_entry) = self.tt.probe_both(board.hash, depth);
        let mut tt_score: Option<i32> = None;
        if !pv_node {
            if let Some(e) = tt_entry {
                let s = adjust_from_tt(e.score, ply);
                match e.flag {
                    Bound::Exact => return (s, e.best),
                    Bound::Lower => tt_score = Some(s.max(tt_score.unwrap_or(-INF))),
                    Bound::Upper => tt_score = Some(s.min(tt_score.unwrap_or(INF))),
                }
                if let Some(s) = tt_score {
                    if e.flag == Bound::Lower && s >= beta {
                        return (s, e.best);
                    }
                    if e.flag == Bound::Upper && s <= alpha {
                        return (s, e.best);
                    }
                }
            }
        }

        // Null-move pruning (Gamma only, gated by the `qsearch` feature —
        // Gamma's feature set): skip a whole move and see if we still
        // fail low. Alpha has no null-move — it searches every move in
        // full, which is both slower and weaker (by design).
        #[cfg(feature = "qsearch")]
        if allow_null && !pv_node && !in_check && depth >= 3 && has_non_pawn_material(board) {
            let eval = evaluate(board);
            if eval >= beta {
                let saved_side = board.side;
                let saved_ep = board.ep_square;
                let saved_hash = board.hash;
                let saved_half = board.halfmove;
                board.side = board.side.other();
                board.ep_square = None;
                board.halfmove += 1;
                board.hash = board.compute_hash();
                self.path_hashes.push(board.hash);
                let r = 2 + depth / 4;
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

        // Move generation + ordering (reuse the check test above).
        let mut moves = board.generate_moves_with_check(false, in_check);
        self.order_moves(board, &mut moves, tt_move, ply);
        let us = board.side;
        let mut best = -INF;
        let mut best_move: Option<Move> = None;
        let mut legal_moves = 0i32;
        let alpha_orig = alpha;
        self.pv_len[ply] = ply;

        self.path_hashes.push(board.hash);
        for i in 0..moves.len() {
            let m = moves[i];

            board.make_move(&m);
            if board.is_in_check(us) {
                board.unmake_move();
                continue; // illegal: leaves own king in check
            }
            legal_moves += 1;

            let is_capture = m.is_capture || m.promo.is_some();
            // Killer test is only needed for Gamma's LMR; Alpha never
            // reduces, so it does not compute it.
            #[cfg(feature = "qsearch")]
            let is_killer = self.is_killer(m, ply) && !is_capture;

            let new_depth = depth - 1;
            let mut score: i32;
            if i == 0 {
                let (s, _) = self.negamax(board, new_depth, -beta, -alpha, ply + 1, pv_node, true);
                score = -s;
            } else {
                // Late-move reduction (Gamma only): quiet late moves are
                // tried shallower first and re-searched only if they
                // improve the score. Alpha never reduces. Reduced search
                // is skipped entirely when the reduced depth would be
                // tiny: at depth 1-2 the LMR saving is marginal and the
                // risk of missing a tactic that the plain search would
                // find is real (measure true at short TC / depth).
                #[cfg(feature = "qsearch")]
                let reduce = if !pv_node && !is_capture && !is_killer && i > 1 && new_depth > 0 {
                    1
                } else {
                    0
                };
                #[cfg(not(feature = "qsearch"))]
                let reduce = 0;
                let reduced = (new_depth - reduce).max(0);
                let (reduced_score, _) = self.negamax(board, reduced, -alpha - 1, -alpha, ply + 1, false, true);
                score = -reduced_score;
                if score > alpha && score < beta {
                    // Re-search with the full window. This was previously
                    // gated to PV nodes only, so at cut nodes a late move
                    // that improved alpha on a zero-window (and possibly
                    // reduced) search kept its inexact score as the node's
                    // best — that inexact value then propagates toward the
                    // root. Re-searching whenever the score lands inside
                    // the window keeps cut-node scores exact too.
                    let (full_score, _) = self.negamax(board, new_depth, -beta, -alpha, ply + 1, pv_node, true);
                    score = -full_score;
                }
            }

            board.unmake_move();
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

        // TT store.
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

    #[cfg(feature = "qsearch")]
    fn qsearch(&mut self, board: &mut Board, mut alpha: i32, beta: i32, ply: usize) -> i32 {
        self.nodes += 1;
        if ply >= MAX_PLY - 1 {
            return evaluate(board);
        }
        if self.should_stop() {
            return evaluate(board);
        }
        let in_check = board.is_in_check(board.side);
        let stand_pat = evaluate(board);
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
        self.order_captures(board, &mut moves, ply);
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
            if !in_check && stand_pat + victim + promo_gain + 200 < alpha {
                continue;
            }
            // SEE pruning (pre-make): skip captures that lose material
            // in a static exchange (SEE < 0) when not in check. In check
            // every evasion must be tried, so no pruning there. This
            // cuts hopeless qnodes without deep search — tactics that
            // win material (SEE >= 0) are always searched.
            if !in_check && see(board, &m) < 0 {
                continue;
            }
            board.make_move(&m);
            if board.is_in_check(us) {
                board.unmake_move();
                continue;
            }
            legal += 1;
            let score = -self.qsearch(board, -beta, -alpha, ply + 1);
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

    fn order_moves(&self, board: &Board, moves: &mut crate::types::MoveList, tt_move: Option<Move>, ply: usize) {
        let n = moves.len();
        if n <= 1 {
            return;
        }
        // Score into a stack array that is only initialised on write (no
        // 1 KiB memset per node), then insertion-sort the live slice by
        // descending score.
        let mut scores = [std::mem::MaybeUninit::<i32>::uninit(); crate::types::MAX_MOVES];
        for (i, m) in moves.as_slice().iter().enumerate() {
            let s = if Some(*m) == tt_move {
                10_000_000
            } else if m.is_capture || m.promo.is_some() {
                // SEE ordering tried and measured negative at 500 ms
                // (nodes rose); MVV-LVA alone is kept.
                let mut s = 9_000_000 + capture_score(board, m);
                if m.promo.is_some() {
                    s += 1000;
                }
                s
            } else if self.is_killer(*m, ply) {
                7_000_000
            } else {
                let slot = self.slot(board, m.from);
                self.history[slot][m.to as usize]
            };
            scores[i].write(s);
        }
        // SAFETY: exactly `n` scores were written above, and the sort only
        // reads indices below `n`.
        let scores = unsafe { std::slice::from_raw_parts_mut(scores.as_mut_ptr().cast::<i32>(), n) };
        sort_moves_by_score(moves, scores);
    }

    #[cfg(feature = "qsearch")]
    fn order_captures(&self, board: &Board, moves: &mut crate::types::MoveList, _ply: usize) {
        let n = moves.len();
        if n <= 1 {
            return;
        }
        let mut scores = [std::mem::MaybeUninit::<i32>::uninit(); crate::types::MAX_MOVES];
        for (i, m) in moves.as_slice().iter().enumerate() {
            scores[i].write(capture_score(board, m));
        }
        // SAFETY: as in `order_moves`.
        let scores = unsafe { std::slice::from_raw_parts_mut(scores.as_mut_ptr().cast::<i32>(), n) };
        sort_moves_by_score(moves, scores);
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
        .map(|p| mk_victim_value(p.kind))
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

fn mk_victim_value(k: PieceType) -> i32 {
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
        Some(p) => mk_victim_value(p.kind),
        None => 0,
    }
}

/// Least-valuable attacker of `to` for `side` given `occ` (and copied pieces).
/// Returns `(square, value)` or `None`. King is considered last (value large).
/// Used by SEE below; written as an explicit piece-by-piece probe so the
/// exchange can be verified by eye.
#[cfg(feature = "qsearch")]
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
#[cfg(feature = "qsearch")]
fn see(board: &Board, m: &Move) -> i32 {
    if !m.is_capture && m.promo.is_none() { return 0; }
    let attacker = match board.piece_on(m.from) { Some(p) => p, None => return 0 };
    let victim = if m.is_en_passant {
        100
    } else {
        board.piece_on(m.to).map(|p| mk_victim_value(p.kind)).unwrap_or(0)
    };
    let promo_bonus = m.promo.map(|p| mk_victim_value(p) - 100).unwrap_or(0);
    let mut gain = [0i32; 33];
    gain[0] = victim + promo_bonus;
    let mut occ = board.occupied ^ bit(m.from);
    if m.is_en_passant {
        let cap = if attacker.color == Color::White { m.to - 8 } else { m.to + 8 };
        occ &= !bit(cap);
    }
    let mut attacker_val = match m.promo {
        Some(p) => mk_victim_value(p),
        None => mk_victim_value(attacker.kind),
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
    let mut s = Searcher::new(tt.clone());
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

    #[test]
    fn mate_in_one() {
        // White to play and mate: Qxf7# (from the Scholar's mate pattern).
        let info = search_fen("r1bqkbnr/pppp1ppp/2n5/4p2Q/2B1P3/8/PPPP1PPP/RNB1K1NR w KQkq - 0 1", 3);
        let best = info.best_move.expect("a best move");
        assert_eq!(best.to_uci(), "h5f7", "expected Qxf7# got {} score {}", best.to_uci(), info.score);
        assert!(info.score > MATE - 100, "score {} should be mate", info.score);
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
    fn stop_flag_halts_search() {
        use std::sync::{Arc, atomic::AtomicBool};
        let mut b = Board::startpos();
        let tt = TranspositionTable::new(8);
        let stop = Arc::new(AtomicBool::new(true));
        let mut s = Searcher::new(tt);
        let info = s.search(&mut b, &[], &SearchLimits::fixed_depth(20), Some(stop), &|_| {});
        // Search must return promptly despite depth 20 (flag already set).
        assert!(info.nodes < 100_000);
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
