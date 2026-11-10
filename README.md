# Ferrum — a clean, from-scratch chess engine in Rust

Ferrum is a UCI-compatible chess engine written from scratch in Rust. It
contains no forks and no code borrowed from Stockfish — every line is
intended to be understandable, and the engine is developed solo with an AI
assistant where the human is architect/reviewer and the AI implements.

## Two variants, one repository

Differences are managed with Cargo features. **Ferrum Alpha** is created by
*removing* features from **Ferrum Gamma**, not by separate development:
first comes the common core, then the differences.

| Variant | Build | Threads | Hash (default) |  Quiescence | Evaluation | Design target |
|---|---|---|---|---|---|---|
| **Ferrum Gamma** (standard) | `cargo build --release` (default) | Lazy SMP, `Threads` 1–8 | 64 MB, `Hash` 1–4096 | Yes (`qsearch`) + null-move, LMR | Advanced: tapered MG/EG, mobility, pawn structure, bishop pair, rook files, king safety | ~2000–2400 Elo |
| **Ferrum Alpha** (lightweight) | `cargo build --release --no-default-features` | Single-threaded (`Threads` max 1) | 8 MB | No (static eval at horizon; no null-move/LMR) | Simple: material + single PST | ~1200–1500 Elo |

The name *Alpha* refers to the level, not the stage of development: it is a
fully-fledged lightweight engine, not a test build. Overtaking Stockfish is
not a goal — that is unrealistic for a solo project. The Elo ranges above
are **design targets** for the feature sets (depth + eval + threads +
hash) on typical hardware; your measured Elo depends on time controls, hash
size and CPU.

```toml
[features]
default = ["gamma"]
gamma   = ["threads", "qsearch", "advanced-eval", "large-tt"]
threads = []
qsearch = []
advanced-eval = []
large-tt = []
```

## Quick start

```sh
# Gamma (standard)
cargo build --release
./target/release/ferrum   # UCI engine — connect via any UCI GUI

# Alpha (lightweight)
cargo build --release --no-default-features
./target/release/ferrum

# Tests (both variants must pass)
cargo test
cargo test --no-default-features
cargo test --release
```

Through a GUI: load the binary, set `Hash` (MB) and `Threads`, start a
game. From the terminal you can also drive UCI by hand:

```
uci
isready
position startpos moves e2e4 e7e5
go depth 6
quit
```

Debug helpers (not part of the UCI spec): `d`, `eval`, `perft <n>`,
`divide <n>`.

## Strength probes (examples)

```sh
cargo run --release --example bench    # startpos depth 4/6/7/8: nodes, PV
cargo run --release --example tactics  # mate-in-1/2, win-of-queen, center
cargo run --release --no-default-features --example bench
```

## Design overview

```
src/
  types.rs    Colours, pieces, Move / MoveList, square <-> string.
  bitboard.rs Bitboard type, precomputed leaper tables (built at startup by
              looping over the board — no magic numbers), attack tests.
  sliders.rs  Sliding attacks from occupancy-indexed tables (bishop and rook,
              64 x 4096 entries each), filled at startup by ray walking.
  board.rs    Board (bitboards + mailbox), Zobrist hashing, FEN I/O,
              pseudo-legal movegen + legality filter, make/unmake with an
              undo stack, perft. King captures are never generated.
  eval.rs     Simple (Alpha) vs advanced tapered (Gamma) evaluation.
              PSTs are White-oriented, rank-8-first tables with a documented
              mirror rule. Advanced adds mobility, doubled/isolated/passed
              pawns, bishop pair, rook open/semi files, king-safety, and the
              Gamma playstyle term (hunting loose material); the simple eval
              adds Alpha's king-shield term.
  tt.rs       Power-of-two transposition table (hash/depth/flag/score/best/
              age), shared behind a mutex for SMP.
  search.rs   Negamax alpha-beta + PVS, iterative deepening, aspiration
              (re-centered windows — see the comment on the edge case),
              TT, MVV-LVA + killers + history, check extension, razoring,
              repetition / 50-move / insufficient-material draws, mate
              scored as MATE - ply. Gamma adds null-move pruning,
              late-move reduction, qsearch (stand-pat, delta pruning, check
              evasions) and Lazy SMP; Alpha searches every move in full
              with static eval at the horizon.
  uci.rs      UCI loop: uci/isready/setoption/position/go + in-engine time
              management (go wtime/btime/winc/binc/movestogo/movetime/
              depth/nodes/infinite). Search runs on a background thread so
              `stop`/`quit` never deadlock stdout.
  main.rs     Thin entry point (prints the variant name to stderr).
```

Representation notes:

* Squares are `0..=63` with `a1 = 0`, so the bit index *is* the square
  number. Ranks are `0..=7` (0 = rank 1).
* The board keeps 12 bitboards (`colour*6 + kind`) plus a 64-entry mailbox
  for cheap piece lookups. Occupancy is the OR of all bitboards.
* Zobrist uses a fixed-seed xorshift generator, so hashes are deterministic
  across runs. The hash is maintained incrementally on every make/unmake.
* Leaper tables are generated at startup by explicit board loops; sliding
  attacks come from occupancy-indexed tables (`sliders.rs`) whose contents
  are filled by ray walking at startup — the ray-stepping part is the
  table build, the lookup is O(1). This trades a little raw speed for
  code you can verify by inspection.
* UCI move matching compares only from/to/promotion (flags such as
  `is_double_push` are generation-dependent and must not break matching).

## Playstyles

The two variants deliberately differ in *bias*, not just depth:

* **Gamma** hunts loose material — the advanced eval adds a bonus for
  enemy pieces that are attacked more than they are defended.
* **Alpha** values king shelter — the simple eval adds a bonus for pawns
  shielding the king.

Both are measured A/B changes, not aspirations; `docs/variant-styles.md`
has the anatomy, the numbers and the rejected alternatives.

## Tools

Python helpers under `tools/` (stdlib only, except the texel tools, which
need NumPy):

| Tool | Purpose |
|---|---|
| `gauntlet.py` | UCI match harness — plays paired games against another engine or a Stockfish anchor, writes `results/*.json` |
| `verify_mates.py` | Re-checks mate claims with Ferrum itself (`perft 1 == 0` + mate score) |
| `session_summary.py` | Tabulates every gauntlet JSON in `results/` into one session table |
| `probe_fen.py` | Asks an engine for its best move on a FEN (`go depth N`) |
| `compare_evals.py` | Evaluates a battery of positions with both engines and diffs them |
| `build_corpus.py` | Builds a (FEN, side-to-move) corpus from the gauntlet games |
| `label_corpus.py` | Labels that corpus with Stockfish scores |
| `ferrum_eval.py` | NumPy replica of the Gamma evaluation (used by the tuner) |
| `texel.py` / `texel_fit.py` | Texel feature extraction and weight fitting against Stockfish labels |

## Testing

* Perft against the Chess Programming Wiki reference counts:
  startpos `20/400/8902`, Kiwipete `48/2039/97862`, en-passant position
  `14/191/2812/43238`, promotion position 4 `6/264/9467/422333` and
  position 5 `44/1486/62379/2103487` — all asserted in `board::tests`.
* Search: mate-in-one (`Qxf7#`), win-of-queen (`Nxa8`), stop-flag handling,
  legal root move — in `search::tests` and `examples/tactics.rs`.
* UCI end-to-end (handshake, `go depth`, mate-in-one, clock + `stop`,
  `Threads`/`Hash` options, `ucinewgame` full reset) — `tests/uci_smoke.rs`
  spawns the real binary via `CARGO_BIN_EXE_ferrum`.
* Protocol rule: while a search is running, `stop` stops it and `position` /
  `ucinewgame` stop-and-join it (printing its `bestmove`, possibly
  `bestmove (none)`) before the board/TT reset — a stale bestmove is never
  attributed to a new position.

## Notes on Elo

See `ELO.md` for the gauntlet method and measured estimates
(`tools/gauntlet.py` vs Stockfish `UCI_Elo` anchors). In short: Gamma
targets 2000–2400 and Alpha 1200–1500; earlier calibration put Gamma at
bottom-of-target and Alpha above target (see `ELO.md` for the current
numbers after the latest strength/scaling work). Estimates move
with TC, Hash and Threads — always use the `--release` build, and prefer
more games / longer controls over single short matches. At
`go movetime 1` Ferrum measures ≈ 1470–1500 vs Stockfish anchors (see
`ELO.md`); the 2000 target is a ≥300 ms target, not a 1 ms one. Hitting 2200+
would require more strength work (deeper search, better eval) rather
than better testing.

## License

GPL-3.0-or-later (see `LICENSE`).
