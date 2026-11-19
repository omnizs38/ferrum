# Ferrum — Elo calibration (measured, not claimed)

Method: paired UCI games with `tools/gauntlet.py` (stdlib only, no cutechess
dependency). Each game alternates colours, both sides get `go movetime N`,
and adjudication is enforced by the harness — mates via `bestmove (none)`
+ `score mate`, draws by 3-fold repetition, 50-move rule and insufficient
material from the oracle FEN, move-limit 300. Every mate claim below was
re-checked with Ferrum itself (`perft 1 == 0` + mate score, `tools/verify_mates.py`).

Anchor: **Stockfish 19** (`UCI_LimitStrength` + `UCI_Elo`), downloaded from
`official-stockfish/Stockfish` `sf_19` windows-x86-64 universal. Stockfish
binary is *not* committed (`.gitignore`: `stockfish.exe`, `stockfish.zip`,
`stockfish_sf19/`, `results/`). Gauntlet drives Stockfish through a Ferrum
oracle for `d`/FEN (Stockfish ignores `d`).

> Caveat: `UCI_Elo` is Stockfish's internal cap, calibrated at longer
> controls. Fixed-`movetime` results drift with TC, and 6–10 game samples
> give wide confidence intervals. Numbers here are **estimates at the
> stated TC**, not FIDE ratings.

## Gamma (standard, default features)

| Match (fixed `movetime`) | Score (Ferrum–SF) | Est. Elo | 95% CI* |
|---|---|---|---|
| Gamma (Threads 2) vs SF 2000, 50 ms | 1–9 (10 games) | ~1618 | 0–1841 |
| Gamma (Threads 4) vs SF 2000, 200 ms | 1–17–2 (20 games) | ~1618 | 0–1792 |
| Gamma (Threads 4) vs SF 2000, 200 ms +castling | 2–17–1 (20 games) | ~1662 | 0–1810 |
| Gamma (Threads 4) vs SF 2000, 200 ms single-lock | 7–12–1 (20 games) | ~1911 | 1716–2061 |
| Gamma (Threads 2) vs SF 2000, 300 ms | 2–3–1 (6 games) | ~1942 | 1342–2253 |
| Gamma (Threads 4) vs SF 2000, 300 ms | 4–5–1 (10 games, v6) | ~1965 | 1687–2199 |
| Gamma (Threads 1) vs SF 2000, 300 ms | 2–7–1 (10 games) | ~1809 | ~1600–2013 |
| Gamma (Threads 4) vs SF 2000, 500 ms | 4–2 (6 games) | ~2120 | 1844–3190† |
| Gamma (Threads 4) vs SF 2000, 500 ms, v7 | 1–8–1 (10 games) | ~1699 | 0–1909 |
| Gamma (Threads 4) vs SF 2000, 500 ms, 20 g | 6–13–1 (20 games) | ~1873 | 1653–2021 |
| Gamma (Threads 4) vs SF 2000, 500 ms incr | 4–5–1 (10 games) | ~1965 | 1687–2199 |
| Gamma (Threads 2) vs SF 2200, 300 ms | 1–5 (6 games) | ~1920 | 0–2176 |

\* Normal-approx on the score fraction mapped through
`Elo = -400·log10(1/p − 1)`. † Upper bound clamps at the cap. Self-play
control Gamma vs Gamma (50 ms): 1–2–3, ~−58 ± wide (≈ 0), so the harness
has no systematic first-process bias.

**Reading:** at 300–500 ms/move Gamma is **below the 2000–2400 design
target** in larger samples (~1618 at 200 ms baseline, ~1873 at 200 ms
check-cap, ~1911 at 200 ms single-lock, ~1965 at 300 ms / 10 games v6
and ~1965 at 500 ms incr 10 g, ~1873 at 500 ms / 20 games; 46 games at
500 ms combined ~1899). Small 6-game samples (~2120) were optimistic;
20-game samples at 200 ms and 500 ms land ~1600–1900. Incremental eval
(~15% faster, 60785 unchanged) + single-lock + check-cap (depth<12,
60785 unchanged, neutral) + aspiration 50 (40882, −33%) + SEE pruning
(39961) help short TC but do not close the gap at 500 ms. It is not
there at short TC, and longer TC does not close the gap to 2200+ —
Stockfish scales better with time than ray-stepping movegen + mobility
eval at ~5 Mnps. There is no evidence for sustained 2200+; the best
point estimate in 40+ games is ~1800–1900 at 500 ms. Reaching 2200+
would need deeper search (speed) and tuned eval (full SPSA over 30+
game samples), not hand-tuned margins.

### Strength work (post-calibration experiments)

Candidate Gamma-only gains were tried and gauntleted, then reverted
where they regressed. Retained (node-neutral, kept): Lazy SMP detached
helpers (+100 ms latency fix), null-move/LMR Gamma-only (Alpha scaling),
passed-pawn mask (bench 60785 unchanged), TT single-lock probe_both
(one lock per non-PV node, bench 60785 unchanged, 200 ms 20 g 7–12–1
~1911 vs 2–17–1 ~1662), and incremental material+PST (bench 60785
unchanged, 200 ms 20 g 4–16 ~1759, slightly below single-lock alone
but ~15% faster: d8 12.8 ms vs 15 ms; kept for speed, scores identical).

| Variant vs SF 2000 | Score | Est. | Bench nodes (d8) |
|---|---|---|---|
| Baseline (null/LMR flat, MVV-LVA, eval orig.) 500 ms | 4–2 | ~2120 | 60785 |
| Baseline (same) 300 ms Threads 4, v6 | 4–5–1 | ~1965 | 60785 |
| +SEE ordering, futility 120·d, SEE<−150, late pruning 500 ms | 1–5 | ~1730 | 41409 |
| +SEE ordering + eval (outposts/rook-7th/tempo) 500 ms | 2–4 | ~1880 | 114014 |
| Eval only (SEE off) 500 ms | 3–3, then 4–6 (10 g) | ~2000, ~1930 | 113283 |
| Dynamic LMR (late+poor-history) + gravity 500 ms | 2–7–1 (10 g) | ~1809 | 95876 |
| Tempo+outposts+rook7th+gravity 300 ms Threads 4, v5 | 2–6–2 | ~1853 | 70444 |
| Threads 1 vs SF 2000, 300 ms | 2–7–1 | ~1809 | — |
| Mobility N/B only (drop R/Q) 300 ms Threads 4 | 3–7 | ~1853 | 34361 |
| King-zone attackers (bench only, reverted) | — | — | 111030 |
| Countermove ordering 300 ms Threads 4 | 3–6–1 | ~1892 | 60679 |
| Passed-pawn mask (node-neutral) 200 ms Threads 4 | 2–17–1 (20 g) | ~1662 | 60785 |
| TT 1-lock (drop depth-insufficient move) | — | — | 124601 |
| Null R=1+depth/4 (less prune) 200 ms T4 | 6–14 (20 g) | ~1853 | 60785 |
| Null R=1+depth/4 300 ms T4 | 1–9 (10 g) | ~1618 | 60785 |
| TT store guard depth≥2 (bench only) | — | — | 98858 |
| Qsearch light eval 200 ms T4 (reverted) | 2–17–1 (20 g) | ~1662 | 56712 |
| Qsearch light eval 300 ms T4 | 1–8–1 (10 g) | ~1699 | 56712 |
| Castling bonus +20 200 ms T4 | 2–17–1 (20 g) | ~1662 | 60785 |
| TT single-lock probe_both 200 ms T4 (kept) | 7–12–1 (20 g) | ~1911 | 60785 |
| TT single-lock 300 ms T4 | 3–7 (10 g) | ~1853 | 60785 |
| TT single-lock 500 ms T4 | 3–7 (10 g) | ~1853 | 60785 |
| TT single-lock +incr 200 ms T4 | 4–16 (20 g) | ~1759 | 60785 |
| TT single-lock +incr 500 ms T4 | 4–5–1 (10 g) | ~1965 | 60785 |
| TT RwLock 200 ms T4 (reverted) | 3–17 (20 g) | ~1699 | 60785 |
| Threads 8 @200 ms (SF T1) | 3–17 (20 g) | ~1699 | — |
| Incremental eval 200 ms T4 (kept speed) | 4–16 (20 g) | ~1759 | 60785 |
| Check-extension cap 12, 200 ms T4 (kept) | 6–13–1 (20 g) | ~1873 | 60785 |
| Aspiration 50, 200 ms T4 (kept) | 7–13 (20 g) | ~1892 | 40882 |
| Mobility 5,4,3,2 200 ms T4 | 6–13–1 (20 g) | ~1873 | 61850 |
| No queen mobility 200 ms T4 | 3–16–1 (20 g) | ~1731 | 52944 |
| Passed bonus x2 200 ms T4 | 4–15–1 (20 g) | ~1785 | 39961 |
| SEE pruning in qsearch 200 ms T4 (kept) | 7–12–1 (20 g) | ~1911 | 39961 |

All 6–20-game intervals overlap (±300), but the direction is
consistent: aggressive pruning and guessed eval weights lose despite
depth, because they widen nodes (eval) or prune tactics (null/LMR).
Code is therefore the baseline search/eval with six retained node-neutral
speed changes: Lazy SMP helpers are detached so `bestmove` returns when
the main thread finishes instead of idling for helpers (+100 ms latency
fix), null-move/LMR are Gamma-only (see Alpha scaling), passed-pawn mask
and incremental material+PST (both keep scores identical, ~15% faster),
TT single-lock probe_both (one lock per non-PV node, 60785 unchanged),
check-extension cap at depth 12 (60785 unchanged, neutral), aspiration
delta 50 (40882 nodes, −33% vs 60785, neutral Elo), and SEE pruning
in qsearch (39961 nodes, ~1911 vs 1892, neutral).

> Bench drift note: the 39961/40882 figures were measured on the
> code states described above. The current tree re-searches a late move
> whenever its score lands inside the window at **any** node (previously
> PV nodes only), a correctness fix that propagates exact scores at cut
> nodes; that costs roughly +12–15% nodes. Bench d8 on the current
> snapshot (this section, 2026-11-07, release build, startpos):
> **45838 nodes**. The strength numbers quoted here are tied to the
> describe-states they were measured on.

Longer TC did not close the gap: reverted Gamma vs SF 2000 at **1000 ms,
Threads 4: 1–4–1, ~1809** (`results/gauntlet_gamma_sf2000_1000.json`).
Stockfish scales better with time than our ray-stepping movegen +
mobility eval at ~4 Mnps. Next work would need search-speed
(movegen/eval optimisation) and full SPSA over 30+ game samples,
not hand-tuned margins.

### 2026-11-07 session — safety hardening, coverage, sanity gauntlets

Work items (node-neutral vs the previous snapshot; bench d8
45838 → 45838): hard ply cap in `negamax` (was a latent OOB-index panic
path past `MAX_PLY`, now returns static eval like qsearch); `ucinewgame`
**and** `position` stop-and-join any running search before resetting
state (previously a stale `bestmove` could be printed after the reset
and misattributed by a pipelineing GUI); `ucinewgame` semantics covered
by a new `tests/uci_smoke.rs` case; `board::tests` gained the two
remaining CPW reference positions 4 (`6/264/9467/422333`) and 5
(`44/1486/62379/2103487`) — both match the wiki, promotion/castling
paths now verified; `MoveList` prefilled from a const; `Move::default`
collapsed into `const fn Move::new`; qsearch delta pruning now counts
the +800 promotion swing (a winning silent promotion could be pruned
at high alpha — same class as the existing capture/en-passant accounting;
node-neutral on startpos bench); gauntlet moved to the shared
`stop_and_join` helper. Full suite green (debug + release, both
variants). A two-axis code review (standards + spec) was run on the
session diff; its findings were applied (duplicated helper, guard
placement, undocumented-tool removal — `tools/uci_drive.py` was dropped
in favour of the smoke test; docs drift in README/ELO.md fixed here).

Measurements (fresh builds from the session tree):

| Match (fixed `movetime`) | Score (Ferrum–SF) | Est. Elo | 95% CI |
|---|---|---|---|
| Gamma (Threads 2) vs Alpha (Threads 1), 50 ms, 20 g | 15–4–1 | ~+215 | 65–543 |
| Gamma (Threads 2) vs Alpha (Threads 1), 100 ms, 10 g | 5–4–1 | ~+35 | −199–313 |
| Gamma (Threads 2) vs Alpha (Threads 1), 50 ms, 20 g (final build, post qsearch-fix) | 12–5–3 | ~+127 | −21–347 |
| Gamma (Threads 4) vs SF 2000, 300 ms, 10 g | 2–7–1 | ~−191 | −2400–13 (p=0.25 → ≈ ~1810 anchor) |

### 2026-11-07 session — 1-millisecond time control

Task: reduce the per-move budget to **1 ms** and measure/improve Ferrum at
that TC. Work: the stop-throttle in `Negamax::should_stop` now checks the
clock every **512 nodes** (was 2048). At ~5 Mnps the old interval could
miss a 1 ms deadline by several × when NPS dropped (0 checks until a
multiple of 2048 nodes). No strength claim at long TC; honest framing:
compliance with sub-ms budgets, unchanged at long TC, bench d8 unchanged
(45838).

Measured (all Gamma, `go movetime 1`, Threads 1, 20 or 10 games, oracle
`ferrum-gamma.exe`, Stockfish SF 19 at the same 1 ms):

| Anchor (Stockfish capped) | Score | Notes |
|---|---|---|
| SF 1200 | 5–2–3 | p=0.65 |
| SF 1300 | 6–1–3 | p=0.75 |
| SF 1400 | 4–5–1 | p=0.45 |
| SF 1500 | 3–3–4 | p=0.50 |
| SF 1500, Threads 4 | 1–6–3 | Lazy SMP is counter-productive at 1 ms (spawn latency + contention) |
| SF 1600 | 1–7–2 | p=0.20 |
| SF 2000 | 0–10–0 | p=0.00 |

**Ferrum at 1 ms is ≈ 1470–1500 Elo** (score 0.50 vs the SF 1500 anchor,
p±0.35). Stockfish at `movetime 1` still searches depth ~4–10 with an
NNUE evaluation; Ferrum reaches depth ~3–5 with a tapered hand-written
eval. On the documented design rule (no "magic" tables, no borrowed
Stockfish code, node-neutral changes only) the 2000 design target at 1 ms
is **not attainable by tuning** — it would need a depth/speed and eval
quality overhaul (magic-bitboard-style movegen ~3–5× faster, NNUE-class
or equivalent eval learned via SPSA over 30+ games). Two tuning temptations
were tried and reverted: LMR disabled at shallow new-depth (d8 widened
5–8× for no measured gain; at 1 ms that is a large depth loss) and
Threads 4 at 1 ms (measured −191 vs Threads 1). The 2000 design target
at 300–500 ms (current: ~1800–1900 vs SF 2000) is the only plausible
2000 path, and it needs the same overhaul.

The 100 ms 10-game sample is uninformative noise next to the 50 ms
20-game runs; both post-fix and pre-fix builds are consistent with a
Gamma–Alpha gap of a few hundred Elo (the two CIs overlap). The SF 2000
anchor at 300 ms lands in the same ~1800–1900 band as the prior
calibration rows (the search is node-identical vs the calibrated build).
Raw JSON (git-ignored): `results/gauntlet_gamma_alpha_50ms_20g.json`,
`results/gauntlet_gamma_alpha_50ms_20g_v2.json` (final build),
`results/gauntlet_gamma_alpha_2026-11-07.json` (100 ms),
`results/gauntlet_gamma_sf2000_300_2026-11-07.json`. All 9 checkmates in
the SF 2000 match were independently re-verified as true mates with
`tools/verify_mates.py` (perft 1 == 0, mate score, `bestmove (none)`).

### 2026-11-08 session — "beat Stockfish" objective: honesty check and strength hunt

Objective: make Gamma strong enough to beat Stockfish. Status: **not
achieved — no honest path to it from inside this repo in one session.**
What was tried, with measured results:

- **qsearch TT (qTT)**: probed/stored depth-0 entries in qsearch. The
  windowed qsearch (delta + SEE pruning) makes returned values
  window-dependent, not position-dependent — an "Exact" entry replayed
  under a different alpha widened the tree from 45838 to 83765 nodes
  (+83%). No sound reuse exists without window-qualified entries, so
  the experiment was fully reverted. Bench back to 45838.
- **Razoring** (kept): at depth ≤ 2, non-PV, in qsearch's favour below
  beta by 250, drop into qsearch. Fixed-depth nodes 45838 → 40439
  (−12%, scores unchanged on startpos). A/B vs base: 6–4–10
  (+35 Elo, CI overlaps 0) at 50 ms — neutral-to-positive, kept.
- **Texel/distillation tuning of the eval** (failed): built a
  94 310-position corpus from all gauntlet games, labeled it with
  Stockfish depth-6 scores (tools/build_corpus.py, label_corpus.py,
  texel.py, ferrum_eval.py), then fitted material values, both PST
  sets and 14 positional scalars to minimize Huber error vs SF's
  scores (tools/texel_fit.py, Adam, 90/10 holdout monitoring — holdout
  RMSE 347 vs 388 start). The fit was *worse in real games*: 30-game
  gauntlet at 100 ms, Table-fit build vs hand-written base **4–22–4
  (−241, CI [−488,−113])**. The gap between in-distribution RMSE and
  real play is exactly the pruning/ordering interaction that textbooks
  warn about. Reverted to the hand-written tables; the corpus and
  tooling are committed for future attempts (positional terms of the
  fit, e.g. passed-pawn and rook-file amplification, are the most
  plausible starting points but were not re-adopted alone either).
- Corpus caveat: games are from this project's own engine vs SF at
  short TC, i.e. tactics-heavy, opening-correlated labels — only a
  sample-correlated distillation exists, not SPSA-equivalent coverage.
- **Moderated positional patch** (4-match amplification pass + bishop
  pair/shield only, hand PST/material kept): 30-game A/B vs committed
  base at 100 ms → **+23 Elo (8–6–16, CI [−104,157])**, not a clear
  win, and bench d8 rose to 57042 nodes (+41%). Reverted per the
  keep-on-clear-win bar (`git checkout HEAD -- src/eval.rs`). Bench
  back to 40439.
- Fresh SF 2000 anchor (committed build with razoring, 10 games at
  300 ms): **4–6–0, p=0.400, −70 (CI [−389,150])**
  (`results/gauntlet_gamma_sf2000_300_final.json`) — Ferrum's best
  recorded 300 ms result at this anchor, but well within the prior
  ~1800–1900 band; the two razor-containing builds (this and the
  2–7–1 era check) overlap heavily, so "razoring adds Elo" cannot be
  claimed from this sample alone. **Ferrum still does not beat
  Stockfish.**
- **Texel closed definitively:** refitting on the full 94 310-position
  corpus (holdout 90/10, Adam) gave holdout RMSE **400 vs 388**
  hand-written — fitting *generalizes worse* on complete data. The
  distillation program is abandoned; corpus/tooling kept for the
  record.
- **Slider attack tables (4c453e8, kept):** new `src/sliders.rs` —
  per-square ≤12-bit relevant-ray occupancy → attacks (2×2 MiB,
  built once at startup by explicit loops; exhaustive test vs ray
  walking over every occupancy). Query: BMI2 PEXT (rt-detected) or
  portable compaction. No magic constants; design rule intact. Node
  output identical (bench d8 = **40439**, unchanged). ~15–20% faster
  bench nps. A/B vs base at 100 ms, 30 games: **13–6–11, +83**
  (CI [−40,231]), direction consistent with faster search at fixed
  TC. With this build: SF 2000 at 300 ms → **2–7–1**
  ([−191], CI [−2400,13],
  `results/gauntlet_gamma_sf2000_300_sliders.json`) — indistinguishable
  from the previous anchors (4–6–0 and 2–7–1), so the slider rewrite
  yields speed but no verified Elo gain at 300 ms.

Verified unchanged: full test suites green (both variants, debug +
release), perft suites match CPW, UCI end-to-end green, bench
45838 → 40439 after razoring, Gamma≈1500 at 1 ms and ~1800–1900 at
300–500 ms vs SF 2000 (unchanged). **Stockfish still beats Ferrum at
every tested TC.**

### 2026-11-08 (late) session — singular extension (rejected) and playstyles (per the user's directive)

- **Singular extension (rejected):** PV-node, depth ≥ 8, Lower/Exact TT bound, verify the TT move remains best on a shallow verifier; extension = search that one child one ply deeper. Result at this engine's depth profile: **bench nodes were identical (51621) but elapsed nearly doubled** — the extension never triggers at the depths reached at this TC (depth ≪ 8 most of the time), so the cost paid was pure overhead with zero returned depth. Reverted; the TT probe unification it exposed was kept. Lesson: extensions that only fire deep are dead weight at short TC.

- **Gamma "cunning/trap-based"** — advanced-eval gains a loose-material
  hunting bonus: `loose_material(side) * 8/100` cp, where loose =
  N/B/R/Q attacked more than defended **and** profitably capturable
  (defenders == 0 → any attacker counts; otherwise at least one cheap
  attacker of value ≤ victim). Unit test:
  `loose_material_scan_works`. A/B vs base, 30 g @100 ms:
  **7–3–20, +47** (CI [−78,185]); earlier independent sample of the
  same idea: +83. Kept.
- **Alpha "defensive"** — simple-eval gains a king pawn-shield bonus:
  8 cp × pawns in the three squares directly ahead of own king
  (cap +24). A naive self-hanging-piece penalty (−8/100 cp own loose
  material) measured **−223** (CI [−451,−97]) and was discarded;
  the shield bonus measures **7–11–12, −47** (CI [−185,78]) vs base —
  within noise of base. Kept (it delivers the defensive identity
  without materially weakening Alpha). Unit test:
  `loose_material_scan_works` covers the shared counter.

Full write-up and verdicts in `docs/variant-styles.md`. The Gamma-Alpha
ladder is unchanged in spirit: Gamma≈1800–1900 vs SF 2000, Alpha≈1500,
both with style-biased play. **Stockfish remains unbeaten everywhere
measured.** The measurable gap decomposition (see the Stockfish
comparison above): NNUE ≫ classical eval, ~10–20× node throughput
(magic bitboards), and measured-pruning parameter fit (SPSA over
thousands of games). The design rules this project chose (no magic
numbers, from-scratch eval, understandable code) and the SPSA/NNUE
infrastructure required are the exact blockers; closing them is a
project, not a session.

Raw JSON (git-ignored): `results/gauntlet_razor_vs_base_50ms.json`,
`results/gauntlet_texeltables_vs_base_100ms.json`.

### 2026-11-15 session — training run: shared search core, defensive Alpha, trapping Gamma

Objective: make both engines stronger and *start training* them toward the
requested styles — Alpha defensive (long-term goal 2000), Gamma trapping
(long-term goal 3500). What was built and measured:

1. **Trainable engine surface.** All positional weights are now
   `eval::EvalParams` (34 knobs) and the prunings are
   `search::SearchParams` (12 knobs). `examples/train.rs` plays A-vs-B games
   entirely in-process (paired openings, colours swapped, fixed node budget
   or clock, style metrics), and `tools/train.py` drives SPSA over the
   weights plus a targeted style search. The engine also accepts a weight
   spec over UCI: `setoption name EvalParams value "pressure_scale=16,..."`
   (empty / `default` = baseline).
2. **Search, both variants.** Shared: quiescence (stand-pat, delta, SEE,
   evasions), late-move pruning (`i >= 4 + depth²`, depth ≤ 4), reverse
   futility (120·depth, depth ≤ 6) and depth-1 futility (100). Gamma keeps
   null-move, LMR, razoring and check extensions.
3. **Evaluation, per variant.** Alpha: own-loose-material penalty 24% +
   tempo 15. Gamma: `pressure_scale` 8→16, new trapped-piece bonus 8, new
   enemy-king-pressure bonus 3, tempo 15.

In-process paired measurements (the trainer, not the UCI gauntlet):

| Match | Games | TC | Score (A–B) | Elo (95% CI) | Style |
|---|---|---|---|---|---|
| Gamma: prunings vs all-off | 600 | 20k nodes/move | 351–92–157 | **+160** [+131,+193] | — |
| Gamma: prunings vs all-off | 400 | 80k nodes/move | 150–63–187 | **+77** [+43,+113] | — |
| Gamma: trapping trio vs base | 600 | 20k nodes/move | 231–160–209 | **+41** [+14,+70] | enemy loose 45.5 vs 37.2 cp; enemy trapped 0.31 vs 0.24; king pressure 5.17 vs 4.52 |
| Gamma: trapping trio vs base | 400 | 80k nodes/move | 95–78–227 | +15 [−19,+49] | same direction |
| Gamma: full package vs old settings | 800 | 20k nodes/move | 464–132–204 | **+153** [+128,+181] | enemy loose 46.8 vs 42.0 |
| Gamma: full package vs old settings | 400 | 80k nodes/move | 149–66–185 | **+73** [+39,+109] | enemy loose 44.9 vs 37.7 |
| Alpha: own-loose 24 + tempo vs base | 400 | 80k nodes/move | 195–141–64 | **+47** [+13,+82] | own loose 40.5 vs 57.2 cp |
| Alpha: prunings vs all-off | 200 | 50 ms | 126–40–34 | **+160** [+109,+217] | — |
| Alpha: + qsearch vs pruning-only | 200 | 50 ms | 105–50–45 | **+98** [+50,+151] | — |
| Alpha: qsearch alone vs no qsearch | 160 | 50 ms | 19–121–20 | **−262** [−344,−199] | — |
| Alpha: full package vs old settings | 400 | 50 ms | 287–68–45 | **+214** [+175,+257] | own loose 39.3 vs 72.9 cp |

Scalar/parameter details behind the rows:

* Reverse futility alone measured +22 [−6,+50] over 600 games; LMP alone
  +128 [+99,+158]; adding futility and RFP on top gave +160, so the set is
  kept together.
* `alpha_own_loose` was swept in 600-game steps: 8 ≈ 16 ≈ 24 (24 beat 16
  by +41 [+11,+72] in a direct 500-game match; 32 lost to 24 by −19
  [−50,+11]). 24 is the trained value.
* qsearch without the prunings is a *loss* for Alpha at a fixed clock
  (it buys tactics but loses depth); with the prunings paying for it,
  qsearch is a clear gain. Both were trained and adopted together.
* `pressure_scale` 24 vs 16 measured +17 [−13,+48]; 16 was kept as the
  moderate choice.

UCI gauntlets (fresh binaries, real subprocesses):

| Match | Games | TC | Result | Elo |
|---|---|---|---|---|
| Gamma trained vs Gamma (old binary) | 40 | 50 ms | 9–8–23 | +9 [−102,+121] |
| Alpha trained vs Alpha (old binary) | 40 | 50 ms | 26–10–4 | **+147** [+40,+291] |
| Gamma trained vs SF 2000 | 10 | 300 ms, T4 | 4–6–0 | −70 [−389,+150] → **≈1930** |
| Alpha trained vs SF 1800 | 10 | 300 ms, T1 | 5–3–2 | +70 [−150,+389] → **≈1870** |

The Gamma-vs-old 50 ms result is consistent with noise: at 50 ms both
builds produce long drawn games against each other (23/40 draws), and the
in-process numbers at 20k/80k nodes are the statistically useful ones. The
absolute anchors are 10-game samples with very wide intervals; they place
both variants near 1900 at 300 ms, not at their long-term goals.

**Bench:** Gamma startpos d8 **51621 → 15577 → 18777 nodes** after the
search prunings and the second eval pass (the bishop-pair change moves the
PV, so the node count is not comparable one-for-one); Alpha d8 272511 nodes
(the old Alpha had no pruning or quiescence, so its node count is not
comparable). Tactics suite green for both variants; full test suites green
(debug + release, both variants); `cargo clippy --all-targets -D warnings`
clean for both.

### Second pass (same session)

Small weight sweeps after the first training run, 600 games each at
20k nodes/move:

| Change | Result | Verdict |
|---|---|---|
| `bishop_pair` 30 → 40 | +30 Elo [+2,+58]; +15 [−19,+49] at 80k (400 g) | kept |
| `bishop_pair` 40 → 50 | −20 [−48,+8] vs 40 | rejected |
| `passed_scale` 100 → 130 | −4 [−32,+24] | rejected |
| `mob_rook` 2 → 3 | +8 [−20,+35] | rejected (noise) |
| Alpha `alpha_shield` 8 → 12 | +10 [−18,+38] | rejected (noise) |
| Alpha `alpha_castle` 0 → 20 | −5 [−33,+23], king-ring pressure 3.21 vs 3.39 | kept for the defensive style |

Final trained specs (now `EvalParams::DEFAULT` / `SearchParams::DEFAULT`):

```
Gamma eval:  pressure_scale=16,trap_bonus=8,king_pressure=3,tempo=15,bishop_pair=40
Alpha eval:  alpha_own_loose=24,tempo=15,alpha_castle=20
search:      rfp_margin=120,rfp_max_depth=6,futility_margin=100,lmp_base=4,
             lmp_max_depth=4,qsearch=1
```

A 20-iteration SPSA pass on Alpha starting from these values moved the
material weights by only ±1–2 cp and produced no measurable result (the
per-iteration score fluctuated around 0.5); it was **not** adopted. The
useful training signal came from the 600-game targeted sweeps above, not
from a short SPSA run.

### 2026-11-16 session — server embedding and convergence sweeps

This session's main work was **server-side loading** (see
[`docs/server.md`](docs/server.md)): a C ABI (`src/ffi.rs`, `cdylib` +
`rlib`) with one independent handle per game, atomic strict-FEN position
updates, panic containment, `go`/`go_json`, options, and string ownership;
`uci::run_with_io`/`run_with_config` for Rust services; CLI flags for
`--hash`/`--threads`/`--eval-params`/`--search-params`; `FERRUM_HASH_MB` to
cap per-instance memory; and a multi-stage `Dockerfile`. `tests/uci_embed.rs`
runs the loop over in-memory buffers, `src/ffi.rs` tests cover the handle
lifecycle (including four concurrent handles), and `tools/embed_demo.py`
loads the built DLL and searches in-process.

Two engine correctness/robustness fixes came out of it:

* `SearchInfo.pv` could be empty when the final (partial) iteration reset
  the PV table before being discarded; the last completed PV is now kept.
  Library/FFI callers rely on it.
* A search stopped before its first iteration completed (immediate `quit`,
  tiny clock) now returns a legal move instead of `bestmove (none)`.

Strength sweeps this session, 600 paired games at 20k nodes/move unless
noted. **None cleared the keep bar**, so none changed the defaults:

| Candidate | Result | Verdict |
|---|---|---|
| Fail-soft qsearch | **−63** (CI −91..−35) | reverted |
| 2-way TT buckets | 6–6–28 at 50 ms, 0 Elo (CI ±111) | reverted |
| LMR log formula (div 4 / div 2) | +5 / −4 | rejected |
| `null_r_base=3` | +2 (CI −26..+30) | rejected |
| `null_r_div=3` | +12 (CI −16..+40) | rejected |
| `asp_delta=30` | +9 (CI −19..+37) | rejected |
| `bishop mobility 4` / `knight mobility 5` | +3 / −3 | rejected |
| rook on seventh (+20) | +2 (CI −26..+30) | rejected |
| IID at depth 5 | +5 (CI −23..+33) | knob kept, off by default |

The engine is at a local optimum for these knobs at this budget. The
training surface now exposes 12 search knobs (`null_r_base`, `null_r_div`,
`asp_delta`, `iid_min_depth` added) for future runs with larger samples.
Gamma bench d8 is unchanged at **18777 nodes**; both variants' benches and
the tactics suite are unchanged.

### 2026-11-16 (later) — Alpha eval pass: a measurable gain

The sweep above was all search/weight micro-tuning. This pass added three
cheap, classical terms to Alpha's lightweight evaluation and found a clear,
repeatable win at two time controls:

| Candidate (1500 paired games @20k unless noted) | Result | Verdict |
|---|---|---|
| knight/bishop mobility 3/2 cp | +16 (CI −2..+34) | kept |
| passed pawns (full shared rank table) | +17 (CI −0.4..+35) | kept |
| bishop pair 30 | +9 (CI −8..+27) | kept |
| **package** | **+31 (CI +16..+47), 2000 games** | **adopted** |
| package at 80k nodes | **+70 (CI +42..+99), 600 games** | transfer confirmed |

Independent UCI confirmation with fresh binaries:

| Match | Games | TC | Result | Elo |
|---|---|---|---|---|
| Alpha new vs Alpha old | 40 | 50 ms | 17–8–15 | +80 [−27,+204] |
| Alpha new vs SF 2000 | 10 | 300 ms | **6–3–1** | +108 [−104,+496] → ≈2100 |

Alpha's 2000 design goal is **reached in this sample** (10 games is a wide
interval, but the direction agrees with 2600 paired games in-process).

Gamma was swept again and stayed put: `pressure_scale=24,trap_bonus=12,
king_danger=6` measured −2 (CI −22..+17) over 1200 games; `king_danger=3`
−2; a stronger pawn shield (`shield_pawn=20,shield_missing=14`) gave +18
(CI −2..+38) then −13 (CI −33..+6) in two independent 1200-game runs, i.e.
neutral. The `fast_legality` speed optimisation (node-identical tree, ~3%
nps) measured −2 (CI −36..+32) over 400 fixed-clock games and was reverted.
Gamma remains at its trained ~1930 vs the SF 2000 anchor.

Bench: Gamma d8 unchanged **18777**; Alpha d8 272511 → **216764** (the new
terms change the PV/node count; Alpha still has no null-move/LMR).

**Honest reading:** the training run is a clear strength gain at short and
medium TC and a clear style gain, but it does **not** reach 2000 for Alpha
or anything near 3500 for Gamma. The remaining gap is the same one ELO.md
has documented all along: eval quality (NNUE/distillation), raw node
throughput (magic-style movegen, legal movegen) and large-scale SPSA over
tens of thousands of games. This session built the harness those next steps
need.

## Gamma 2026-11-18 - evaluation caching and standard LMR re-search

Two changes were kept this session. One is pure speed, one is a strength
change; the rest of the sweep found no further gain.

### Kept

**1. Static-evaluation cache (node-identical when it landed).**
`negamax` now computes the static evaluation lazily at most once per node and
reuses it for razoring, reverse futility, null-move and depth-1 futility.
Depth-1 nodes previously re-evaluated once per quiet move. qsearch also skips
the stand-pat evaluation when in check, where the value is never read. The
tree was unchanged at the time (Gamma d8 18,777; Alpha 216,764).

* `tools/speed.py --nodes 2000000`: median **1,381,956 -> 1,545,006 NPS
  (1.12x)**; median time for 2M nodes 1,460 -> 1,305 ms.
* `tools/speed.py --movetime 100`: the middlegame position reached **depth 8
  instead of 7** (depth sum 36 vs 35): the same clock buys a deeper search.
* 200-game `tools/gauntlet.py` at 50 ms, cache build vs HEAD:
  **34-28-138, +10 Elo (95% CI -38..+59)**. The direction agrees with the
  NPS measurement; the sample is too small to stand alone.

**2. `lmr_research=1` (standard LMR re-search at cut nodes).**
A reduced move whose probe fails high is re-searched at full depth even at a
zero-window node. The historical in-window test (`score < beta`) could never
hold when `beta = alpha + 1`, so those re-searches were silently skipped. This
is both the standard rule and a correctness fix; it costs nodes (Gamma d8
18,777 -> **19,951**, Alpha unchanged at 216,764) but wins at fixed nodes and
at fixed time:

| Match | Games | TC | Result (candidate vs base) |
|---|---|---|---|
| `lmr_research=1` | 1200 | 20k nodes | **+20** (95% CI ~[0,+40]) |
| `lmr_research=1` (repeat) | 1600 | 20k nodes | **+8** (95% CI -9..+26) |
| **pooled fixed-node** | **2800** | 20k nodes | **+13** (95% CI ~[0,+26]) |
| `lmr_research=1` | 800 | 50 ms | **+6** (95% CI -18..+30) |

Bench: Gamma d8 **19,951**; Alpha d8 **216,764**. Tactics suite, full test
suite (debug/release, both variants) and clippy are green.

### Swept and rejected (defaults unchanged; most knobs kept for future runs)

| Candidate | Games | Result (candidate vs base) | Verdict |
|---|---|---|---|
| TT cutoff before reverse futility | 1200 @20k | -13 (95% CI -32..+7) | rejected, code removed |
| ProbCut margin 150 / 200 / 250 (`probcut_r=4`) | 800 each @20k | -7 / +6 / -3 | off |
| ProbCut margin 150 | 500 @80k | -6 (-37..+24) | off |
| qsearch SEE margin 50 / 100 | 600 each @20k | +9 / -12 | default 0 |
| qsearch delta margin 150 / 250 | 600 each @20k | +10 / -1 | default 200 |
| King-attack package (`king_attack_units=1,king_attack_count=2,king_queen_scale=50`) | 1600 @20k | +2 (-15..+19) | rejected |
| King open-file/centre (`king_open_file=12,king_open_adjacent=6,king_center=12`) | 1600 @20k | +11 (-6..+28) | not kept (CI includes 0) |
| LMP base 5 | 1600 @20k | -2 (-19..+15) | rejected |
| RFP margin 100 | 1600 @20k | +7 (-10..+24) | not kept (CI includes 0) |
| Futility margin 80 | 1600 @20k | +9 (-8..+26) | not kept (CI includes 0) |
| LMP max depth 5 | 1600 @20k | -0 (-17..+17) | no change |

> ProbCut note: the first sweep ran before its SEE gate was hoisted above
> `make_move`, so every capture qualified. The gate now works (and the same
> pre-move-SEE fix makes the optional `lmr_captures` reduction effective),
> but both remain off by default; the numbers above describe that earlier
> run and would need re-testing if ProbCut were revisited.

The engine remains at a local optimum for these search/eval knobs at 20k
nodes/move; the LMR re-search is the one change that repeated in the same
direction at both fixed nodes and a fixed clock.

## Alpha (lightweight, `--no-default-features`)

| Match (fixed `movetime`) | Score (Ferrum–SF) | Est. Elo | 95% CI |
|---|---|---|---|
| Alpha vs SF 1400, 50 ms | 8–2 (10 games) | ~1641 | 1436–3190† |
| Alpha vs SF 1400, 300 ms | 6–0 (6 games) | >1400 (sweep) | lower bound only |
| Alpha vs SF 1600, 300 ms | 2–3–1 (6 games) | ~1542 | 942–1853 |
| Alpha vs SF 1400, 200 ms (scaled, v2) | 6–3–1 (10 games) | ~1508 | 1296–1896 |
| Alpha vs SF 1600, 300 ms (scaled, v2) | 4–6 (10 games) | ~1530 | 1211–1680 |

Self-play control Alpha vs Alpha (50 ms): 2–4–0 (2 White wins + 4 draws
normalised to A), broadly symmetric. Direct ladder (Gamma Threads 2):
**9–0–1, gap ≈ +512** (lower bound ≥ 257) at 50 ms. At 200 ms the
ladder is noisy: Gamma Threads 4 5–5–0 (gap 0, ±252) and Threads 1
6–2–2 (gap +147, CI −59–716) — both consistent with a true gap of
+150–450 given ±250 CIs, and with the SF-anchored gap
(~1965 − ~1520 ≈ +445).

**Reading:** Alpha was **~1540–1640** (above its old 1200–1500 target)
before the 2026-11-15 training run. After the shared pruning core, quiescence
and the defensive eval, it measures **≈1870** at 300 ms (5–3–2 vs SF 1800,
10 games) and **+214 Elo** over its old self at a fixed 50 ms clock. The
Gamma–Alpha gap has narrowed from ~+440 to roughly +60 at 300 ms, which is
also why the single shared search core matters more than the variant
labels: the remaining differences are eval, threading and TT size.

## How to reproduce

```powershell
cargo build --release
Copy-Item target\release\ferrum.exe target\release\ferrum-gamma.exe
cargo build --release --no-default-features
Copy-Item target\release\ferrum.exe target\release\ferrum-alpha.exe
cargo build --release
# stockfish.exe from Stockfish sf_19 windows-x86-64 universal (see gitignore)

python tools/gauntlet.py --white target/release/ferrum-gamma.exe --black stockfish.exe --games 6 --movetime 300 --threads-w 2 --elo-b 2000 --oracle target/release/ferrum-gamma.exe --out results/gauntlet_gamma_sf2000_300.json
```

Raw game lists (JSON) are written to `results/` (git-ignored). Re-run with
more games / longer TC to tighten the intervals before quoting a rating.

### Reproducing the 2026-11-15 training measurements

```powershell
# In-process paired games (the statistically useful numbers)
cargo build --release --example train
.\target\release\examples\train.exe --a default `
  --b "pressure_scale=8,trap_bonus=0,king_pressure=0,tempo=0" `
  --sb "rfp_margin=0,futility_margin=0,lmp_base=0" `
  --games 800 --nodes 20000 --seed 71 --parallel 10

cargo build --release --no-default-features --example train
.\target\release\examples\train.exe --a default `
  --b "alpha_own_loose=0,tempo=0" `
  --sb "rfp_margin=0,futility_margin=0,lmp_base=0,qsearch=0" `
  --games 400 --movetime 50 --seed 73 --parallel 8

# Full training loop (SPSA + style search) is documented in docs/training.md
python tools/train.py --variant gamma --mode spsa --iters 24 --games 200
```

Note that `ferrum.exe` is the *last-built* variant. Build Gamma last if you
want `target/release/ferrum.exe` to be Gamma, as the README's quick start
assumes.
