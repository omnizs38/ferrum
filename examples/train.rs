//! In-process paired training matches.
//!
//! Plays A-vs-B games entirely inside one process (no UCI subprocesses, no
//! Python), using the engine's own search and evaluation. This is the
//! "training" harness for Ferrum's playstyle work:
//!
//! * both sides get the same fixed node budget per move (default 20 000),
//!   so the comparison is about evaluation, not speed;
//! * openings are short random legal sequences, and every opening is played
//!   twice with colours swapped (paired games), which removes most of the
//!   opening/colour variance from the Elo estimate;
//! * weights are passed as `key=value,...` specs (`eval::EvalParams`), so a
//!   parameter set can be compared without rebuilding the engine;
//! * playstyle metrics (`eval::style_stats`) are accumulated for both sides:
//!   own loose material / trapped pieces / king pressure for the defensive
//!   bias, enemy loose material for the trapping bias.
//!
//! Usage:
//!   cargo run --release --example train -- \
//!     --a "pressure_scale=10,trap_bonus=6" --b default --games 200 --nodes 20000
//!   cargo run --release --no-default-features --example train -- \
//!     --a "alpha_king_danger=4" --b default --games 200
//!
//! Output: W/D/L, score, Elo(A-B) with an approximate 95% CI, style metrics,
//! and optionally a JSON file (`--json results/train_x.json`).

use ferrum::board::Board;
use ferrum::eval::{style_stats, EvalParams, StyleStats};
use ferrum::search::{is_insufficient_material, SearchLimits, SearchParams, Searcher};
use ferrum::tt::TranspositionTable;
use ferrum::types::Color;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

#[derive(Clone, Debug)]
struct Config {
    a_spec: String,
    b_spec: String,
    a_search: String,
    b_search: String,
    games: usize,
    nodes: u64,
    /// If set, play with `go movetime` instead of a node budget.
    movetime: Option<u64>,
    max_plies: usize,
    seed: u64,
    parallel: usize,
    tt_mb: usize,
    json: Option<String>,
    tag: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            a_spec: "default".to_string(),
            b_spec: "default".to_string(),
            a_search: "default".to_string(),
            b_search: "default".to_string(),
            games: 100,
            nodes: 20_000,
            movetime: None,
            max_plies: 240,
            seed: 1,
            parallel: 8,
            tt_mb: 8,
            json: None,
            tag: String::new(),
        }
    }
}

const HELP: &str = "\
Ferrum in-process training match

  --a SPEC       A-side eval weights (key=value,... or 'default')
  --b SPEC       B-side eval weights (key=value,... or 'default')
  --sa SPEC      A-side search prunings (key=value,... or 'default')
  --sb SPEC      B-side search prunings (key=value,... or 'default')
  --games N      total paired games (even, default 100)
  --nodes N      node budget per move (default 20000)
  --movetime N   use a millisecond clock instead of a node budget
  --maxplies N   move-limit adjudication threshold (default 240)
  --seed N       opening/jitter seed (default 1)
  --parallel N   games played concurrently (default 8)
  --tt N         transposition table MB per side (default 8)
  --json PATH    write the full result as JSON
  --tag TEXT     label for logs
";

fn parse_args() -> Result<Config, String> {
    let mut c = Config::default();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        let key = args[i].as_str();
        let mut val = || -> Result<String, String> {
            i += 1;
            args.get(i)
                .cloned()
                .ok_or_else(|| format!("missing value for {key}"))
        };
        match key {
            "--a" => c.a_spec = val()?,
            "--b" => c.b_spec = val()?,
            "--sa" => c.a_search = val()?,
            "--sb" => c.b_search = val()?,
            "--games" => c.games = val()?.parse().map_err(|_| "bad --games")?,
            "--nodes" => c.nodes = val()?.parse().map_err(|_| "bad --nodes")?,
            "--movetime" => c.movetime = Some(val()?.parse().map_err(|_| "bad --movetime")?),
            "--maxplies" => c.max_plies = val()?.parse().map_err(|_| "bad --maxplies")?,
            "--seed" => c.seed = val()?.parse().map_err(|_| "bad --seed")?,
            "--parallel" => c.parallel = val()?.parse().map_err(|_| "bad --parallel")?,
            "--tt" => c.tt_mb = val()?.parse().map_err(|_| "bad --tt")?,
            "--json" => c.json = Some(val()?),
            "--tag" => c.tag = val()?,
            "--help" | "-h" => {
                println!("{HELP}");
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument '{other}'")),
        }
        i += 1;
    }
    if c.games == 0 {
        return Err("--games must be > 0".into());
    }
    if c.games % 2 != 0 {
        return Err("--games must be even (paired colours)".into());
    }
    c.parallel = c.parallel.max(1);
    Ok(c)
}

// ---------------------------------------------------------------- RNG

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        // SplitMix64: deterministic, dependency-free.
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { self.next_u64() % n }
    }
}

// ------------------------------------------------------------- game data

#[derive(Clone, Copy, Debug, Default)]
struct StyleAcc {
    samples: u64,
    own_loose: f64,
    own_trapped: f64,
    king_ring_against: f64,
    king_ring_for: f64,
    enemy_trapped: f64,
    enemy_loose: f64,
}

impl StyleAcc {
    fn add(&mut self, s: StyleStats) {
        self.samples += 1;
        self.own_loose += s.own_loose as f64;
        self.own_trapped += s.own_trapped as f64;
        self.king_ring_against += s.king_ring_against as f64;
        self.king_ring_for += s.king_ring_for as f64;
        self.enemy_trapped += s.enemy_trapped as f64;
        self.enemy_loose += s.enemy_loose as f64;
    }
    fn merge(&mut self, o: &StyleAcc) {
        self.samples += o.samples;
        self.own_loose += o.own_loose;
        self.own_trapped += o.own_trapped;
        self.king_ring_against += o.king_ring_against;
        self.king_ring_for += o.king_ring_for;
        self.enemy_trapped += o.enemy_trapped;
        self.enemy_loose += o.enemy_loose;
    }
    fn means(&self) -> [f64; 6] {
        if self.samples == 0 {
            return [0.0; 6];
        }
        let n = self.samples as f64;
        [
            self.own_loose / n,
            self.own_trapped / n,
            self.king_ring_against / n,
            self.king_ring_for / n,
            self.enemy_trapped / n,
            self.enemy_loose / n,
        ]
    }
}

#[derive(Clone, Debug)]
struct GameResult {
    /// Score for A: 1.0 win, 0.5 draw, 0.0 loss.
    a_score: f64,
    reason: String,
    plies: usize,
    style_a: StyleAcc,
    style_b: StyleAcc,
}

// ----------------------------------------------------------- game logic

const SIMPLE_VALUE: [i32; 6] = [100, 320, 330, 500, 900, 0];

fn material_balance(board: &Board) -> i32 {
    let mut score = 0;
    for sq in 0..64u8 {
        if let Some(p) = board.mailbox[sq as usize] {
            let v = SIMPLE_VALUE[p.kind.idx()];
            match p.color {
                Color::White => score += v,
                Color::Black => score -= v,
            }
        }
    }
    score
}

fn random_opening(rng: &mut Rng, plies: usize) -> Vec<String> {
    let mut board = Board::startpos();
    let mut out = Vec::with_capacity(plies);
    for _ in 0..plies {
        let legal = board.generate_legal_moves();
        if legal.is_empty() {
            break;
        }
        let pick = legal.as_slice()[rng.below(legal.len() as u64) as usize];
        out.push(pick.to_uci());
        board.make_move(&pick);
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn play_game(
    a: EvalParams,
    b: EvalParams,
    sa: SearchParams,
    sb: SearchParams,
    opening: &[String],
    cfg: &Config,
    a_is_white: bool,
    rng_seed: u64,
) -> GameResult {
    let mut board = Board::startpos();
    let mut hashes: Vec<u64> = vec![board.hash];
    let mut reps: HashMap<u64, u32> = HashMap::new();
    reps.insert(board.hash, 1);
    for uci in opening {
        if let Some(m) = board.find_legal(uci) {
            board.make_move(&m);
            hashes.push(board.hash);
            *reps.entry(board.hash).or_insert(0) += 1;
        }
    }

    let tt_a = TranspositionTable::new(cfg.tt_mb);
    let tt_b = TranspositionTable::new(cfg.tt_mb);
    let mut searcher_a = Searcher::with_settings(tt_a, a, sa);
    let mut searcher_b = Searcher::with_settings(tt_b, b, sb);

    let mut rng = Rng::new(rng_seed);
    let mut style_a = StyleAcc::default();
    let mut style_b = StyleAcc::default();

    let mut plies = 0usize;
    loop {
        if plies >= cfg.max_plies {
            let bal = material_balance(&board);
            let (a_score, reason) = if bal >= 400 {
                (if a_is_white { 1.0 } else { 0.0 }, "material adjudication (White +)".to_string())
            } else if bal <= -400 {
                (if a_is_white { 0.0 } else { 1.0 }, "material adjudication (Black +)".to_string())
            } else {
                (0.5, "move limit adjudicated draw".to_string())
            };
            return GameResult { a_score, reason, plies, style_a, style_b };
        }

        // Draw rules before searching (same order as tools/gauntlet.py).
        if board.halfmove >= 100 {
            return GameResult { a_score: 0.5, reason: "fifty-move rule".into(), plies, style_a, style_b };
        }
        if is_insufficient_material(&board) {
            return GameResult { a_score: 0.5, reason: "insufficient material".into(), plies, style_a, style_b };
        }
        if reps.get(&board.hash).copied().unwrap_or(0) >= 3 {
            return GameResult { a_score: 0.5, reason: "threefold repetition".into(), plies, style_a, style_b };
        }

        let a_to_move = (board.side == Color::White) == a_is_white;
        // Sample playstyle every 8 plies once the opening is over.
        if plies >= 8 && plies.is_multiple_of(8) {
            style_a.add(style_stats(&board, if a_to_move { board.side } else { board.side.other() }));
            style_b.add(style_stats(&board, if a_to_move { board.side.other() } else { board.side }));
        }

        let jitter = 90 + rng.below(21); // 90..=110 % of the budget
        let limits = if let Some(ms) = cfg.movetime {
            SearchLimits {
                time_ms: Some((ms * jitter / 100).max(1)),
                nodes: None,
                depth: 128,
                infinite: false,
            }
        } else {
            SearchLimits {
                time_ms: None,
                nodes: Some(cfg.nodes * jitter / 100),
                depth: 128,
                infinite: false,
            }
        };
        let info = if a_to_move {
            searcher_a.search(&mut board, &hashes, &limits, None, &|_| {})
        } else {
            searcher_b.search(&mut board, &hashes, &limits, None, &|_| {})
        };

        let mv = match info.best_move {
            Some(m) => m,
            None => {
                let legal = board.generate_legal_moves();
                if legal.is_empty() {
                    let mated = board.is_in_check(board.side);
                    let winner_white = board.side == Color::Black;
                    let (a_score, reason) = if mated {
                        let s = if winner_white == a_is_white { 1.0 } else { 0.0 };
                        (s, "checkmate".to_string())
                    } else {
                        (0.5, "stalemate".to_string())
                    };
                    return GameResult { a_score, reason, plies, style_a, style_b };
                }
                legal.as_slice()[0]
            }
        };
        board.make_move(&mv);
        hashes.push(board.hash);
        *reps.entry(board.hash).or_insert(0) += 1;
        plies += 1;
    }
}

// ------------------------------------------------------------- aggregate

#[derive(Default)]
struct Totals {
    a_wins: usize,
    b_wins: usize,
    draws: usize,
    plies: usize,
    reasons: HashMap<String, usize>,
    style_a: StyleAcc,
    style_b: StyleAcc,
}

fn elo_from_p(p: f64) -> f64 {
    let p = p.clamp(1e-6, 1.0 - 1e-6);
    -400.0 * (1.0 / p - 1.0).log10()
}

fn main() {
    let cfg = match parse_args() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("train: {e}");
            std::process::exit(2);
        }
    };
    let a = match EvalParams::from_spec_or_default(&cfg.a_spec) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("train: A spec: {e}");
            std::process::exit(2);
        }
    };
    let b = match EvalParams::from_spec_or_default(&cfg.b_spec) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("train: B spec: {e}");
            std::process::exit(2);
        }
    };
    let sa = match SearchParams::from_spec_or_default(&cfg.a_search) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("train: A search spec: {e}");
            std::process::exit(2);
        }
    };
    let sb = match SearchParams::from_spec_or_default(&cfg.b_search) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("train: B search spec: {e}");
            std::process::exit(2);
        }
    };
    if a == b {
        eprintln!("train: warning: A and B parameter sets are identical (self-play control)");
    }

    let pairs = cfg.games / 2;
    let workers = cfg.parallel.min(pairs).max(1);
    let next = AtomicUsize::new(0);
    let start = Instant::now();

    let results: Vec<GameResult> = std::thread::scope(|scope| {
        let mut handles = Vec::with_capacity(workers);
        for w in 0..workers {
            let next = &next;
            let cfg = &cfg;
            handles.push(scope.spawn(move || {
                let mut out: Vec<GameResult> = Vec::new();
                loop {
                    let pair = next.fetch_add(1, Ordering::Relaxed);
                    if pair >= pairs {
                        break;
                    }
                    let mut rng = Rng::new(cfg.seed.wrapping_add((pair as u64) << 32).wrapping_add(w as u64));
                    let opening_plies = 2 + rng.below(5) as usize;
                    let opening = random_opening(&mut rng, opening_plies);
                    // Same opening, colours swapped.
                    let s1 = cfg.seed ^ (pair as u64).wrapping_mul(0x2545_F491_4F6C_DD1D);
                    out.push(play_game(a, b, sa, sb, &opening, cfg, true, s1));
                    out.push(play_game(a, b, sa, sb, &opening, cfg, false, s1 ^ 0xA5A5_A5A5_5A5A_5A5A));
                }
                out
            }));
        }
        let mut all = Vec::with_capacity(cfg.games);
        for h in handles {
            all.extend(h.join().expect("training worker panicked"));
        }
        all
    });

    let mut t = Totals::default();
    for r in &results {
        if r.a_score >= 0.75 {
            t.a_wins += 1;
        } else if r.a_score <= 0.25 {
            t.b_wins += 1;
        } else {
            t.draws += 1;
        }
        t.plies += r.plies;
        *t.reasons.entry(r.reason.clone()).or_insert(0) += 1;
        t.style_a.merge(&r.style_a);
        t.style_b.merge(&r.style_b);
    }
    let total = results.len();
    let score_a = t.a_wins as f64 + 0.5 * t.draws as f64;
    let p = score_a / total as f64;
    let se = (p * (1.0 - p) / total as f64).sqrt();
    let lo = (p - 1.96 * se).clamp(1e-6, 1.0 - 1e-6);
    let hi = (p + 1.96 * se).clamp(1e-6, 1.0 - 1e-6);
    let elo = elo_from_p(p);
    let elo_lo = elo_from_p(lo);
    let elo_hi = elo_from_p(hi);
    let elapsed = start.elapsed().as_secs_f64();

    match cfg.tag.as_str() {
        "" => println!("Ferrum training match"),
        tag => println!("Ferrum training match [{tag}]"),
    }
    println!("A: {}", a.to_spec());
    println!("B: {}", b.to_spec());
    if sa != SearchParams::DEFAULT || sb != SearchParams::DEFAULT {
        println!("A search: {}", sa.to_spec());
        println!("B search: {}", sb.to_spec());
    }
    let budget = match cfg.movetime {
        Some(ms) => format!("movetime={ms}ms"),
        None => format!("nodes/move={}", cfg.nodes),
    };
    println!(
        "games={} {} maxplies={} seed={} parallel={}",
        total, budget, cfg.max_plies, cfg.seed, workers
    );
    println!(
        "A wins={} B wins={} draws={} score(A)={:.3} Elo(A-B)={:.0} (95% CI [{:.0}, {:.0}])",
        t.a_wins, t.b_wins, t.draws, p, elo, elo_lo, elo_hi
    );
    println!(
        "avg plies={:.1} elapsed={:.1}s ({:.1} games/s)",
        t.plies as f64 / total as f64,
        elapsed,
        total as f64 / elapsed
    );
    let m_a = t.style_a.means();
    let m_b = t.style_b.means();
    println!(
        "style A: own_loose={:.1} own_trapped={:.2} king_ring_against={:.2} king_ring_for={:.2} enemy_trapped={:.2} enemy_loose={:.1}",
        m_a[0], m_a[1], m_a[2], m_a[3], m_a[4], m_a[5]
    );
    println!(
        "style B: own_loose={:.1} own_trapped={:.2} king_ring_against={:.2} king_ring_for={:.2} enemy_trapped={:.2} enemy_loose={:.1}",
        m_b[0], m_b[1], m_b[2], m_b[3], m_b[4], m_b[5]
    );
    let mut reasons: Vec<_> = t.reasons.iter().collect();
    reasons.sort_by_key(|(_, n)| std::cmp::Reverse(**n));
    println!(
        "endings: {}",
        reasons
            .iter()
            .map(|(r, n)| format!("{r}={n}"))
            .collect::<Vec<_>>()
            .join(", ")
    );

    if let Some(path) = &cfg.json {
        let style = |m: [f64; 6]| {
            format!(
                "{{\"own_loose\":{:.3},\"own_trapped\":{:.3},\"king_ring_against\":{:.3},\"king_ring_for\":{:.3},\"enemy_trapped\":{:.3},\"enemy_loose\":{:.3}}}",
                m[0], m[1], m[2], m[3], m[4], m[5]
            )
        };
        let reasons_json = reasons
            .iter()
            .map(|(r, n)| format!("\"{}\":{}", r.replace('"', "'"), n))
            .collect::<Vec<_>>()
            .join(",");
        let json = format!(
            "{{\"tag\":\"{}\",\"a_spec\":\"{}\",\"b_spec\":\"{}\",\"a_search\":\"{}\",\"b_search\":\"{}\",\
             \"games\":{},\"nodes\":{},\"movetime\":{},\"seed\":{},\
             \"a_wins\":{},\"b_wins\":{},\"draws\":{},\"p_a\":{:.4},\"elo\":{:.1},\"elo_lo\":{:.1},\"elo_hi\":{:.1},\
             \"avg_plies\":{:.1},\"elapsed_s\":{:.1},\"style_a\":{},\"style_b\":{},\"endings\":{{{}}}}}",
            cfg.tag, a.to_spec(), b.to_spec(), sa.to_spec(), sb.to_spec(),
            total, cfg.nodes, cfg.movetime.map(|m| m.to_string()).unwrap_or("null".into()), cfg.seed,
            t.a_wins, t.b_wins, t.draws, p, elo, elo_lo, elo_hi,
            t.plies as f64 / total as f64, elapsed, style(m_a), style(m_b), reasons_json
        );
        if let Some(parent) = std::path::Path::new(path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match std::fs::write(path, json) {
            Ok(()) => println!("wrote {path}"),
            Err(e) => eprintln!("train: could not write {path}: {e}"),
        }
    }
}
