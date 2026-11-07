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
Code is therefore the baseline search/eval (bench 39961 with
aspiration 50 + SEE) with six retained node-neutral speed changes:
Lazy SMP helpers are detached so `bestmove` returns when the main
thread finishes instead of idling for helpers (+100 ms latency fix),
null-move/LMR are Gamma-only (see Alpha scaling), passed-pawn mask
and incremental material+PST (both keep scores identical, ~15% faster),
TT single-lock probe_both (one lock per non-PV node, 60785 unchanged),
check-extension cap at depth 12 (60785 unchanged, neutral), aspiration
delta 50 (40882 nodes, −33% vs 60785, neutral Elo), and SEE pruning
in qsearch (39961 nodes, ~1911 vs 1892, neutral).

Longer TC did not close the gap: reverted Gamma vs SF 2000 at **1000 ms,
Threads 4: 1–4–1, ~1809** (`results/gauntlet_gamma_sf2000_1000.json`).
Stockfish scales better with time than our ray-stepping movegen +
mobility eval at ~4 Mnps. Next work would need search-speed
(movegen/eval optimisation) and full SPSA over 30+ game samples,
not hand-tuned margins.

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

**Reading:** Alpha was **~1540–1640** (above target) before scaling;
after gating null-move/LMR to Gamma it measures **~1508–1530** —
at the **top edge of its 1200–1500 design target**, not comfortably
inside. The Gamma–Alpha gap from SF anchors is ~+440, in line with
the 50 ms ladder.

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
