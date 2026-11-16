# Running Ferrum server-side

Ferrum is a plain UCI engine, but it is also a library. Bots and servers can
either **spawn** the binary and speak UCI, or **load** it in-process through
the C ABI. The in-process path avoids process/pipe overhead and makes
per-request isolation trivial (one handle per game).

## Which integration to use

| Situation | Use |
|---|---|
| One engine per process, standard UCI GUI/tooling | the `ferrum` binary |
| A bot with many short games, or a search endpoint | the shared library + C ABI |
| A Rust service | `ferrum::uci::run_with_io` / `run_with_config`, or the `Searcher` API |
| Container deployment | the `Dockerfile` (binary + library in one image) |

## C ABI

Build with `cargo build --release`; the artifacts are:

| Platform | Binary | Shared library |
|---|---|---|
| Linux | `target/release/ferrum` | `target/release/libferrum.so` |
| macOS | `target/release/ferrum` | `target/release/libferrum.dylib` |
| Windows | `target/release/ferrum.exe` | `target/release/ferrum.dll` |

`src/ffi.rs` is the complete surface:

| Function | Purpose |
|---|---|
| `ferrum_create() -> FerrumEngine*` | new independent engine handle |
| `ferrum_destroy(e)` | free it |
| `ferrum_reset(e) -> int` | start position + clear TT (`ucinewgame`) |
| `ferrum_set_position(e, fen, moves) -> int` | FEN (`NULL` = startpos) + UCI moves |
| `ferrum_set_fen(e, fen) -> int` | convenience for FEN only |
| `ferrum_set_option(e, name, value) -> int` | `Hash`, `Threads`, `EvalParams`, `SearchParams`, `Clear Hash` |
| `ferrum_go(e, ms, depth, nodes, threads) -> char*` | best move (blocking) |
| `ferrum_go_json(...) -> char*` | best move + score/mate/depth/nodes/time/PV |
| `ferrum_evaluate(e) -> int` | static eval, cp from side to move (`i32::MIN` on error) |
| `ferrum_legal_move(e) -> char*` | first legal move (fallback) |
| `ferrum_fen(e) -> char*` | current position |
| `ferrum_name()` / `ferrum_version()` / `ferrum_error_string(code)` | static strings |
| `ferrum_string_free(s)` | free a returned string |

Limits: pass any combination; `nodes > 0` wins over `movetime`, and
`depth > 0` caps both. With none set the search is bounded to depth 8.
`threads <= 0` keeps the handle's configured value.

Returned `char*` values are allocated by Rust and **must** be released with
`ferrum_string_free`; `const char*` values are static.

### Error codes

| Code | Meaning |
|---|---|
| 0 | success |
| -1 | null handle/string |
| -2 | invalid UTF-8 |
| -3 | invalid FEN |
| -4 | illegal move |
| -5 | unknown option |
| -6 | bad option value |
| -7 | internal panic (caught; handle still usable) |

`ferrum_set_position` is atomic: an invalid FEN or one illegal move leaves
the previous position untouched. FENs are validated strictly (one king per
side, no pawns on back ranks, no adjacent kings), which matters when the
input comes from users.

### C

```c
#include <stdio.h>

typedef struct FerrumEngine FerrumEngine;
extern FerrumEngine *ferrum_create(void);
extern void ferrum_destroy(FerrumEngine *);
extern int ferrum_set_position(FerrumEngine *, const char *, const char *);
extern char *ferrum_go_json(FerrumEngine *, unsigned long long, int,
                            unsigned long long, int);
extern void ferrum_string_free(char *);

int main(void) {
    FerrumEngine *e = ferrum_create();
    ferrum_set_position(e, "startpos", "e2e4 e7e5");
    char *json = ferrum_go_json(e, 100, 0, 0, 1);
    puts(json);
    ferrum_string_free(json);
    ferrum_destroy(e);
    return 0;
}
```

### Python (ctypes)

`tools/embed_demo.py` is a runnable version of this (and a good smoke test
after a build):

```python
import ctypes, json
lib = ctypes.CDLL("target/release/libferrum.so")
lib.ferrum_create.restype = ctypes.c_void_p
lib.ferrum_go_json.restype = ctypes.c_void_p
lib.ferrum_string_free.argtypes = [ctypes.c_void_p]
e = lib.ferrum_create()
lib.ferrum_set_position(e, b"startpos", b"e2e4 e7e5")
ptr = lib.ferrum_go_json(e, 100, 0, 0, 1)
print(json.loads(ctypes.string_at(ptr)))
lib.ferrum_string_free(ptr)
lib.ferrum_destroy(e)
```

### Node (ffi-napi)

```js
const ffi = require("ffi-napi");
const lib = ffi.Library("./libferrum.so", {
  ferrum_create: ["pointer", []],
  ferrum_set_position: ["int", ["pointer", "string", "string"]],
  ferrum_go_json: ["pointer", ["pointer", "uint64", "int", "uint64", "int"]],
  ferrum_string_free: ["void", ["pointer"]],
  ferrum_destroy: ["void", ["pointer"]],
});
const e = lib.ferrum_create();
lib.ferrum_set_position(e, "startpos", "e2e4 e7e5");
const ptr = lib.ferrum_go_json(e, 100, 0, 0, 1);
console.log(JSON.parse(ptr.readCString()));
lib.ferrum_string_free(ptr);
lib.ferrum_destroy(e);
```

## Rust embedding (UCI over any I/O)

`ferrum::uci` is not tied to stdin/stdout:

```rust
use ferrum::uci;
let cfg = uci::Config { hash_mb: 16, threads: 1, ..Default::default() };
uci::run_with_config(reader, writer, cfg);   // blocks until quit/EOF
```

`writer` only needs `Write + Send + 'static`; the search thread shares it
behind a mutex. `tests/uci_embed.rs` drives the loop over in-memory buffers.
For lower-level control, use `ferrum::search::Searcher` directly.

## Threading and memory model

* **One handle = one game.** Handles are independent (board, history, TT).
  Different handles may run on different threads concurrently.
* **A single handle is not re-entrant.** `ferrum_go` blocks; do not call it
  from two threads on the same handle.
* **Memory:** the TT is `Hash` MB per handle. Gamma defaults to 64 MB and
  Alpha to 8 MB; override with `FERRUM_HASH_MB` (environment) or
  `setoption Hash`. A bot with 100 concurrent games should use 8–16 MB per
  handle (≈0.8–1.6 GB), or fewer handles playing games sequentially.
* **CPU:** for many concurrent games keep `threads = 1` per handle and let
  the OS schedule handles. For a single strong instance use 2–8 threads
  (Gamma only; Alpha clamps to 1).

## Deployment

```sh
# Linux binary + shared library (native codegen for this host)
RUSTFLAGS="-C target-cpu=native" cargo build --release
./target/release/ferrum --version
python tools/embed_demo.py            # loads libferrum.so and searches

# Docker (bin + shared library, non-root user, 64 MB hash cap)
docker build -t ferrum .
docker run -i --rm ferrum             # UCI over stdio
docker run --rm -e FERRUM_HASH_MB=16 ferrum --version
```

The Dockerfile builds with `-C target-cpu=native` by default (the image is
compiled on the machine that will run it); pass
`--build-arg FERRUM_RUSTFLAGS=` for a portable image. The default
`cargo build --release` remains portable and, per `docs/speed.md`, is
already ~1.5–1.6x faster than Stockfish 19 in NPS; the native build pushes
that to ~1.8x.

For Alpha use `--build-arg CARGO_FLAGS=--no-default-features`. For a
musl/scratch image, cross-compile with
`--target x86_64-unknown-linux-musl` and copy the static binary
(`rustup target add x86_64-unknown-linux-musl` plus a musl linker).

### CLI flags for service wrappers

```text
ferrum --hash 16 --threads 4 --eval-params "pressure_scale=16,trap_bonus=8"
ferrum --version
ferrum --help
```

Flags set the initial options; UCI `setoption` still overrides them. Without
flags the binary is a normal UCI engine.

## Operational notes

* stdout is strictly UCI (plus the debug commands `d`, `eval`, `perft`,
  `divide`). Diagnostics and rejected options go to stderr.
* A search runs on a background thread; `stop`, `position`, `ucinewgame`
  and `quit` stop-and-join it. A search stopped before finishing still
  returns a legal move (or `bestmove (none)` for mate/stalemate).
* A panic in a search thread unwinds, is ignored by the join, and the loop
  keeps running: one bad request does not take the server down.
* Not implemented: pondering, MultiPV, Syzygy/Gaviota tablebases, chess960.
  Requests for them are ignored rather than answered incorrectly.
