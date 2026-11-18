# AGENTS.md

Ferrum is a dependency-free Rust UCI chess engine. One source tree builds two
variants via Cargo features: **Gamma** (default: threads, qsearch,
advanced-eval, large-tt) and **Alpha** (`--no-default-features`: single-thread,
8 MB TT, lightweight eval: material + PST, defensive king/loose terms, cheap
mobility, passed pawns, bishop pair). Both variants share quiescence, reverse futility,
late-move pruning and depth-1 futility; the `qsearch` feature now means
**Gamma's extra aggressive set** (null-move, LMR, razoring, check extensions),
not quiescence itself. Alpha is Gamma with features removed, so keep shared
code shared and gate behavior on the individual features (`threads`,
`qsearch`, `advanced-eval`, `large-tt`) — `gamma` is only the umbrella/naming
feature.

`README.md` has the module map; `ELO.md` is the measured-strength log;
`docs/variant-styles.md` explains the eval/playstyle differences;
`docs/training.md` describes the in-process trainer and training workflow;
`docs/speed.md` records search-speed benchmarks against Stockfish 19;
`docs/server.md` covers the C ABI/embedding model. No CI exists:
verification is local. The project is deliberately dependency-free (no
crates) and contains no code borrowed from Stockfish — implement from
scratch rather than vendoring or copying.

## Build, test, lint

```sh
cargo test                          # both variants must pass
cargo test --no-default-features
cargo test --release
cargo build --release               # Gamma -> target/release/ferrum(.exe)
cargo build --release --no-default-features   # Alpha
cargo clippy --all-targets -- -D warnings     # currently clean
```

- Single test: `cargo test board::tests::perft_kiwipete` (add
  `--no-default-features` to check Alpha-only paths).
- Both variants build to the **same** `target/release/ferrum(.exe)` path, so
  a later Alpha build silently replaces Gamma. Copy/rename between variants
  before comparing (see `ELO.md` "How to reproduce").
- **Do not run `cargo fmt`**: the tree is not rustfmt-clean (it would rewrite
  nearly every file). Match the surrounding style instead.
- `tests/uci_smoke.rs` spawns the real binary via `CARGO_BIN_EXE_ferrum`.
  Changing stdout, threading or command ordering can break it.
- Benchmarks must be release builds. `cargo run --release --example bench`
  prints startpos nodes by depth; current Gamma d8 baseline is 19951 nodes
  (Alpha d8 is 216764: it has a cheaper eval and no null-move/LMR). Report
  the node delta for search/eval changes, and use `examples/tactics.rs` for
  mate/material sanity checks.
- Training: `cargo run --release --example train -- --a <spec> --b <spec>`
  plays in-process paired games; `python tools/train.py` drives SPSA + style
  search. Weights are `eval::EvalParams`, prunings are
  `search::SearchParams`, both accept `key=value,...` specs and both can be
  set over UCI (`setoption name EvalParams value ...`).
- Manual UCI debugging: run the binary and type `uci`, `position ...`,
  `go depth 6`; extra commands `d`, `eval`, `perft <n>`, `divide <n>`.

## Engine invariants that are easy to break

- Squares are `a1 = 0`, so the bit index is the square number.
- PST tables in `src/eval.rs` are White-oriented and **rank 8 first**. White
  index is `(7 - rank) * 8 + file`, Black is `rank * 8 + file`. Adding tables
  with the wrong row order silently inverts the eval.
- All board mutation must go through `Board::put_piece` / `remove_from`: they
  maintain the Zobrist hash and the Gamma incremental material+PST scores
  (`mg_scores`/`eg_scores`). `eval::tests::incremental_scores_match_recomputed`
  catches drift.
- Evaluation scores are centipawns from the side-to-move perspective; mates
  use `MATE = 32000`, mate in `n` is `MATE - n`.
- King captures are never generated.
- UCI move matching compares only from/to/promotion; move flags are
  generation-dependent and must not take part in matching.
- stdout is strictly UCI (plus the debug commands). Put diagnostics on stderr.
  A search runs on a background thread: `stop`, `position` and `ucinewgame`
  must stop-and-join the running search before touching board/TT state, and
  its `bestmove` must be printed before the reset. This ordering is tested.

## Server / embedding surface

- `src/ffi.rs` is the C ABI (`cdylib`). Keep every pointer-taking entry
  point `unsafe`, null-safe, `catch_unwind`-guarded, and returning the
  documented error codes; the module-level `# Safety` contract is the shared
  documentation. Do not add global mutable state: handles must stay
  independent for one-handle-per-worker servers.
- `uci::run_with_io` / `run_with_config` must stay generic over
  `BufRead` and `Write + Send + 'static`; `Output<W>` is the only writer
  passed to the search thread.
- Release builds use `panic = "unwind"` (not abort) so the FFI can catch
  panics. Do not switch it back to abort.
- `FERRUM_HASH_MB` caps the default TT size for containers; `Hash` still
  overrides it per instance.
- Embedding is tested by `tests/uci_embed.rs` (in-memory UCI loop) and the
  `src/ffi.rs` unit tests (lifecycle, options, atomic rejection, mate JSON,
  concurrent handles). `tools/embed_demo.py` loads the built DLL/SO.
- On Windows, Cargo warns that the bin and lib both produce `ferrum.pdb`;
  this is a known benign collision (Linux/macOS are unaffected).

## Speed workflow

- `python tools/speed.py --engine <exe> --compare stockfish.exe --nodes 5000000`
  is the canonical benchmark: identical node counts, interleaved engines,
  wall-clock time. Never compare NPS from separate, non-interleaved runs.
- Speed changes must stay node-identical (`cargo run --release --example bench`
  keeps Gamma d8 at 19951 and Alpha d8 at 216764). If the tree changes, A/B
  strength in-process before keeping it.
- The default build is portable; `RUSTFLAGS="-C target-cpu=native"` is the
  recommended server/benchmark build (the Dockerfile defaults to it).
- Record new numbers in `docs/speed.md`, not in `ELO.md`.

## Strength / gauntlet workflow

- `tools/*.py` are stdlib-only except the texel tools (NumPy); they need
  Python 3.10+.
- `tools/gauntlet.py` expects separate binaries. From `ELO.md`'s "How to
  reproduce": build Gamma, copy `ferrum.exe` to `ferrum-gamma.exe`, build
  Alpha, copy to `ferrum-alpha.exe`, then build Gamma again last. Stockfish
  needs `--oracle <ferrum binary>` because it has no `d` command.
- `results/`, `training/`, `stockfish.exe`, `stockfish.zip`,
  `stockfish_sf19/` and `tools/_game.pgn` / `tools/analyze_game.py` are
  git-ignored; never commit them.
- Don't quote Elo figures without a gauntlet run; add new measurements and
  bench node counts to `ELO.md` rather than rewriting past results.
