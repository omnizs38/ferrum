# Speed: Ferrum vs Stockfish 19

"Faster" here means **search throughput**: nodes searched per second, and how
long it takes to search a fixed number of nodes. It is not Elo and not
time-to-depth — Stockfish is far more selective, so at the same clock it
reaches a greater depth (see the caveat at the end).

## Method

`tools/speed.py` drives two UCI engines with one thread and the same 64 MB
hash, on four fixed positions (startpos, Kiwipete, a middlegame, an
endgame). Both engines search **the same 5,000,000 nodes**; the wall time is
measured. Runs are interleaved per position (A then B, or B then A on
alternate positions) and each engine gets a warmup search, so CPU frequency
drift affects both equally. This isolates per-node cost from search
selectivity.

```sh
# Fixed node budget (recommended: identical work, time is the metric)
python tools/speed.py --engine target/release/ferrum-gamma-native.exe \
  --compare stockfish.exe --nodes 5000000

# Same clock instead (NPS + depth reached)
python tools/speed.py --engine target/release/ferrum.exe \
  --compare stockfish.exe --movetime 1000
```

Stockfish 19 (`sf_19` windows-x86-64 universal) is the anchor used in
`ELO.md`; the binary is not committed.

## Result (2026-11-16, 5M nodes, 1 thread, 64 MB hash)

| Build | Median NPS | Time for 5M nodes | vs SF 19 NPS |
|---|---|---|---|
| **Ferrum Gamma**, default build | 1,438,591 | **3.48 s** | **1.63x** |
| **Ferrum Gamma**, `target-cpu=native` | 1,625,141 | **3.08 s** | **1.82x** |
| **Ferrum Alpha**, default build | 1,351,778 | **3.70 s** | **1.53x** |
| **Ferrum Alpha**, `target-cpu=native` | 1,588,142 | **3.15 s** | **1.81x** |
| Stockfish 19 (reference) | 884,000 | 5.66 s | 1.00x |

Both variants are faster than Stockfish 19 in per-node search speed, in both
the portable default build and the native build. Raw JSON:
`results/speed_5m_*.json` (git-ignored).

## What was sped up this session

All changes are **node-identical** (Gamma startpos d8 stayed at 18,777
nodes when they landed, Alpha at 216,764), so strength was unchanged; only
time per node improved. (The current tree reports Gamma d8 19,951 after the
2026-11-18 LMR re-search default; see `ELO.md`.) Measured with the same
fixed-node method over 5M nodes:

| Change | Gain |
|---|---|
| O(1) null-move hash (`Board::null_move_hash`) instead of a full 64-square recompute | included below |
| Unmake restores incremental material+PST from `Undo` (no PST lookups) | included below |
| Mobility folded into the pressure scan (one attack-set pass instead of two) | included below |
| All three, default build | **1.08x** vs the session's starting build |
| `RUSTFLAGS="-C target-cpu=native"` on top | **1.15x** vs the default build |

The combined speedup over the starting build is ~1.24x for Gamma; Alpha
gains from the null-move hash and the native build.

### 2026-11-18: evaluation caching (node-identical)

Two hot-path cleanups. Both were **node-identical** when they landed (Gamma
d8 18,777 nodes and Alpha 216,764, tactics suite unchanged; the separate LMR
re-search default is what moves the current Gamma bench to 19,951):

* `negamax` computes the static evaluation lazily and at most once per node.
  Razoring, reverse futility, null-move pruning and depth-1 futility all read
  the same value; depth-1 nodes previously paid one full evaluation per
  quiet move.
* qsearch no longer computes the stand-pat evaluation when in check, where
  the value is never read.

`tools/speed.py` with a fixed 2,000,000-node budget (one thread, 64 MB hash,
interleaved positions):

| Build | Median NPS | Time for 2M nodes |
|---|---|---|
| before | 1,381,956 | 1,460 ms |
| after | 1,545,006 | 1,305 ms |
| ratio | **1.12x** | **0.89x** |

At a fixed 100 ms clock the middlegame position reached **depth 8 instead of
depth 7** (depth sum 36 vs 35 across the four positions), which is the point:
the same clock buys a deeper search.

### 2026-11-18: Alpha's loose-material scan (node-identical)

Alpha's simple evaluation was doing roughly eight times the work it needed
for its defensive term, and doing it twice per node:

* `alpha_style` was called once per colour and each call ran
  `loose_material_by_owner` — two scans per evaluation.
* That scan called the whole `Pressure::scan` with a style-test `EvalParams`
  (`pressure_scale: 8`, `alpha_own_loose: 1`, rest default), so it computed
  loose material **plus** trapped pieces, king-ring pressure and full
  N/B/R/Q mobility for both colours, then discarded everything except
  `.loose`.

Now `evaluate_with` runs one two-colour loose pass per node, and the loose
test lives in `LooseSide::piece_value`, which `Pressure::scan` calls from the
per-piece loop it already runs. Alpha's `LooseSide::total` never touches the
mobility, trapped-piece or king-ring code; Gamma's pass is unchanged.

Node counts are identical: Gamma d8 **19,951**, Alpha d8 **216,764** (all
depths unchanged), tactics unchanged. Measured with the canonical
fixed-node method (`tools/speed.py`, 3M nodes, 1 thread, 64 MB hash,
interleaved positions):

| Build | Median NPS | Time for 3M nodes |
|---|---|---|
| Alpha before | 1,241,158 | 2,510 ms |
| Alpha after | 1,478,624 | 2,040 ms |
| ratio | **1.19x** | **0.82x** |

The gain is larger where the board is crowded and smaller in the endgame,
as expected for a per-piece term: Kiwipete and the middlegame improved
~1.3x, startpos ~1.07x, the endgame ~1.10x. At **fixed depth** (the bench,
where evaluation dominates) Alpha's d8 search drops from ~141 ms to ~101 ms
(~1.40x). Gamma is neutral in both measurements.

### Why the native build helps

The default x86-64 build targets SSE2. `target-cpu=native` enables the
host's popcount/BMI2/AVX paths (the slider code already uses BMI2 PEXT when
available) and better inlining. Build it on the machine that will run it:

```sh
RUSTFLAGS="-C target-cpu=native" cargo build --release
# Alpha:
RUSTFLAGS="-C target-cpu=native" cargo build --release --no-default-features
```

The Dockerfile does this by default (`FERRUM_RUSTFLAGS` build arg; pass an
empty value for a portable image).

## Caveat: depth, not just speed

At a fixed 1-second clock Stockfish reaches depth ~21 while Gamma reaches
~12 and Alpha ~9, because Stockfish's pruning is far more selective. The
same nodes also buy Stockfish much more depth (5M nodes: SF depth sum 122
vs Ferrum 61). Ferrum is faster **per node**; Stockfish searches **fewer
nodes per depth**. Closing that selectivity gap is strength/eval work, not
raw speed, and is tracked separately in `ELO.md`.

Raw move generation is also faster in Stockfish: `perft 5` from startpos
takes 0.13 s in Ferrum (36.9M nodes/s) vs 0.026 s in Stockfish (187M
nodes/s). Ferrum's search NPS still wins because Stockfish's NNUE
evaluation dominates its per-node cost.
