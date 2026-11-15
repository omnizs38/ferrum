# Training Ferrum

Ferrum's engines are trained, not just hand-tuned. This document describes
the machinery added in the 2026-11-15 session and how to reproduce a run.

The two objectives driving the work:

| Variant | Style | Long-term Elo goal |
|---|---|---|
| **Ferrum Alpha** | defensive — shelter the king, do not leave material hanging | 2000 |
| **Ferrum Gamma** | trapping — create loose enemy material, trap pieces, press the king | 3500 |

Measured results are in [`ELO.md`](../ELO.md) and the style anatomy is in
[`variant-styles.md`](variant-styles.md). The goals are long-term: current
measurements at 300 ms are ≈1870 (Alpha) and ≈1930 (Gamma).

## The three pieces

### 1. Trainable weights (`src/eval.rs`, `src/search.rs`)

Every positional weight is a field of `eval::EvalParams` (29 knobs) and every
optional pruning/limit is a field of `search::SearchParams` (12 knobs). The
`DEFAULT` constants are the trained baseline; a spec string overrides them:

```
pressure_scale=16,trap_bonus=8,king_pressure=3
rfp_margin=120,futility_margin=100,lmp_base=4,qsearch=1
```

Unknown keys are an error, so a typo cannot silently fall back to the
baseline. The same spec is accepted by the UCI engine:

```
setoption name EvalParams value pressure_scale=16,trap_bonus=8,king_pressure=3
```

which makes an external gauntlet reproduce the exact trained build without
recompiling.

### 2. In-process trainer (`examples/train.rs`)

Plays A-vs-B games inside one process — no subprocesses, no Python:

* both sides get the same node budget per move (default 20 000) or the same
  clock; the comparison is therefore about evaluation/search, not hardware;
* each game starts from a short random legal opening, and **every opening
  is played twice with colours swapped**, removing most opening/colour
  variance;
* draw and mate adjudication mirrors `tools/gauntlet.py` (threefold, 50-move,
  insufficient material, `bestmove (none)` mates, material adjudication at
  the move limit);
* games run on all available cores (`--parallel`), which makes 400–800-game
  matches take minutes rather than hours;
* playstyle metrics (`eval::style_stats`) are accumulated for both sides:
  own/enemy loose material, own/enemy trapped pieces, and attacks on each
  king ring.

Example:

```sh
cargo run --release --example train -- \
  --a "pressure_scale=16,trap_bonus=8" --b default \
  --games 600 --nodes 20000 --seed 51 --parallel 10
```

Output: W/D/L, score fraction, Elo(A−B) with a 95% interval, average game
length, and the style table. `--json` writes the same data.

### 3. Training driver (`tools/train.py`)

Builds the example for the requested variant itself and runs two phases:

1. **SPSA** over all `EvalParams` with paired games as the objective
   (`--mode spsa`). Every iteration perturbs all coordinates in a random
   direction, plays the two perturbed sets against each other, and moves
   the parameters along the observed score gradient. Steps decay.
2. **Style search** (`--mode style`): with strength roughly fixed, try a
   small grid of style weights, keep the candidate with the best
   `style_index` provided it does not lose strength (`--style-floor`).

```sh
python tools/train.py --variant gamma --mode spsa --iters 24 --games 200
python tools/train.py --variant gamma --mode style --games 400
python tools/train.py --variant gamma --mode validate --games 600
```

Artifacts land in `training/` (git-ignored): the SPSA trajectory, the style
trials, the final spec, and validation runs.

## What training found

The largest gains came from search, not eval, and the two were coupled:

| Change | Engine | Measurement |
|---|---|---|
| LMP + RFP + futility | Gamma | +160 Elo at 20k nodes (600 g), +77 at 80k (400 g) |
| LMP + RFP + futility | Alpha | +160 Elo at 50 ms (200 g) |
| quiescence *on top* of prunings | Alpha | +98 Elo at 50 ms (200 g) |
| quiescence *without* prunings | Alpha | **−262 Elo** (cost too much depth) |
| own-loose penalty 24 + tempo | Alpha | +47 Elo at 80k nodes (400 g) |
| trapping trio | Gamma | +41 Elo at 20k (600 g), +15 at 80k |
| bishop pair 30 → 40 | Gamma | +30 Elo at 20k (600 g), +15 at 80k (400 g) |
| castled-king bonus | Alpha | strength-neutral; defensive metric improved |
| whole package vs old settings | Gamma | +153 (20k, 800 g), +73 (80k, 400 g) |
| whole package vs old settings | Alpha | +214 at 50 ms (400 g) |

The quiescence result is the main lesson: a change is not good in
isolation, it is good *with the pruning set that pays for it*. Training the
prunings and qsearch together (and A/B-ing the combination) is what turned
a 262-Elo regression into a 98-Elo gain.

## Reproducing the numbers

The in-process matches used `--nodes 20000` / `--nodes 80000` (Gamma) and
`--movetime 50` (Alpha). Absolute anchors use the UCI gauntlet:

```powershell
python tools/gauntlet.py --white target/release/ferrum-gamma-trained.exe `
  --black stockfish.exe --games 10 --movetime 300 --threads-w 4 `
  --elo-b 2000 --oracle target/release/ferrum-gamma-trained.exe `
  --out results/gauntlet_gamma_trained_sf2000_300.json
```

Raw game JSON is git-ignored; the summary tables live in `ELO.md`.

## Caveats

* Fixed-node matches overstate fixed-time gains when a change adds
  per-node cost (qsearch is the example). Every adopted change was
  re-measured at a higher node budget or a fixed clock.
* Paired openings remove variance but not all of it; 400–800 games give
  roughly ±30 Elo. 10-game Stockfish anchors are ±150 or worse.
* SPSA was run for a few thousand games, not tens of thousands. A
  20-iteration Alpha run from the trained baseline moved material weights
  by only ±1–2 cp and changed nothing measurable (score indistinguishable
  from 0.5); the adopted gains came from the larger, 600-game targeted
  sweeps. Resolving 1–2 cp weight changes needs an order of magnitude more
  games, so treat SPSA as a starting point, not a finished eval tune.
* The long-term goals (2000 / 3500) need more than this harness: a learned
  or heavily tuned eval, faster movegen, and far larger training budgets.
* The 2026-11-16 sweep (fail-soft qsearch, 2-way TT, LMR log formula,
  null-move R, aspiration window, mobility weights, IID) found no change
  that cleared the keep bar; they are recorded in `ELO.md`. Treat the
  current defaults as a local optimum at 600 games per candidate, not as
  the global optimum.
