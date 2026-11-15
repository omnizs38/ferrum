//! C ABI for embedding Ferrum in servers and bots.
//!
//! The engine is a normal Rust library; this module lifts it over the FFI
//! boundary so a Node/Python/Java/Go process can load `ferrum.dll`
//! (`libferrum.so` / `libferrum.dylib`) and search without spawning and
//! piping a subprocess.
//!
//! # Lifetime and threading model
//!
//! * [`ferrum_create`] returns an opaque `*mut FerrumEngine`; every call
//!   takes that handle. [`ferrum_destroy`] frees it.
//! * Handles are completely independent (own board, history and TT). Two
//!   handles may be used from two threads concurrently. A *single* handle
//!   must not be used from two threads at the same time — give each game or
//!   bot worker its own handle.
//! * Every entry point catches panics and returns an error code instead of
//!   unwinding across the FFI boundary, so a bad position cannot kill the
//!   host process.
//!
//! # Ownership of returned strings
//!
//! Functions returning `char *` allocate a Rust `CString`; the caller must
//! release it with [`ferrum_string_free`]. `const char *` returns point at
//! static data and must not be freed.
//!
//! # Error codes
//!
//! `0` success; `-1` null handle; `-2` invalid UTF-8; `-3` invalid FEN;
//! `-4` illegal move; `-5` unknown option; `-6` bad option value;
//! `-7` internal panic (the engine is still usable; reset the position).
//!
//! [`ferrum_evaluate`] returns `i32::MIN` instead of an error code so a real
//! score can never be confused with a failure.
//!
//! # Minimal C usage
//!
//! ```c
//! FerrumEngine *e = ferrum_create();
//! ferrum_set_position(e, "startpos", "e2e4 e7e5");
//! char *json = ferrum_go_json(e, 100, 0, 0, 1);
//! puts(json);
//! ferrum_string_free(json);
//! ferrum_destroy(e);
//! ```

//! # Safety (all `unsafe` entry points)
//!
//! Handles must come from [`ferrum_create`] and remain alive until the call
//! returns; string pointers must be NUL-terminated and valid for reading.
//! Null handles/strings are rejected with an error code (they are never
//! dereferenced), and no function retains a caller pointer after returning.
//! Strings returned by this module must be freed exactly once with
//! [`ferrum_string_free`]. The shared contract is documented here rather
//! than repeated on every function.
#![allow(clippy::missing_safety_doc)] // see the module-level safety contract

use crate::board::Board;
use crate::eval::{evaluate_with, EvalParams};
use crate::search::{SearchInfo, SearchLimits, SearchParams, Searcher, MATE};
use crate::tt::{default_hash_mb, TranspositionTable, MAX_HASH_MB};
use std::ffi::{c_char, CStr, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[cfg(feature = "threads")]
const FFI_MAX_THREADS: usize = 8;
#[cfg(not(feature = "threads"))]
const FFI_MAX_THREADS: usize = 1;

pub const ERR_NULL: i32 = -1;
pub const ERR_UTF8: i32 = -2;
pub const ERR_FEN: i32 = -3;
pub const ERR_MOVE: i32 = -4;
pub const ERR_OPTION: i32 = -5;
pub const ERR_VALUE: i32 = -6;
pub const ERR_PANIC: i32 = -7;

/// Opaque engine handle. One per game/worker.
pub struct FerrumEngine {
    board: Board,
    game_hashes: Vec<u64>,
    tt: TranspositionTable,
    hash_mb: usize,
    threads: usize,
    eval: EvalParams,
    search: SearchParams,
}

impl FerrumEngine {
    fn new() -> FerrumEngine {
        let hash_mb = default_hash_mb();
        FerrumEngine {
            board: Board::startpos(),
            game_hashes: vec![Board::startpos().hash],
            tt: TranspositionTable::new(hash_mb),
            hash_mb,
            threads: 1,
            eval: EvalParams::DEFAULT,
            search: SearchParams::DEFAULT,
        }
    }

    fn reset(&mut self) {
        self.board = Board::startpos();
        self.game_hashes.clear();
        self.game_hashes.push(self.board.hash);
        self.tt = TranspositionTable::new(self.hash_mb);
    }
}

fn guard_int<F: FnOnce() -> i32>(f: F) -> i32 {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or(ERR_PANIC)
}

fn guard_ptr<F: FnOnce() -> *mut c_char>(f: F) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or(std::ptr::null_mut())
}

/// Read an optional C string. `None` for a null pointer.
unsafe fn opt_str<'a>(p: *const c_char) -> Result<Option<&'a str>, i32> {
    if p.is_null() {
        return Ok(None);
    }
    CStr::from_ptr(p).to_str().map(Some).map_err(|_| ERR_UTF8)
}

/// Required C string.
unsafe fn req_str<'a>(p: *const c_char) -> Result<&'a str, i32> {
    opt_str(p)?.ok_or(ERR_NULL)
}

fn to_c_string(s: String) -> *mut c_char {
    match CString::new(s) {
        Ok(c) => c.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

// ---------------------------------------------------------------- version

/// Engine name, e.g. `"Ferrum Gamma"`. Static; do not free.
#[no_mangle]
pub extern "C" fn ferrum_name() -> *const c_char {
    if cfg!(feature = "gamma") {
        c"Ferrum Gamma".as_ptr()
    } else {
        c"Ferrum Alpha".as_ptr()
    }
}

/// Crate version, e.g. `"0.1.0"`. Static; do not free.
#[no_mangle]
pub extern "C" fn ferrum_version() -> *const c_char {
    const VERSION: &std::ffi::CStr = unsafe {
        CStr::from_bytes_with_nul_unchecked(concat!(env!("CARGO_PKG_VERSION"), "\0").as_bytes())
    };
    VERSION.as_ptr()
}

/// Human-readable text for an error code. Static; do not free.
#[no_mangle]
pub extern "C" fn ferrum_error_string(code: i32) -> *const c_char {
    let s: &'static [u8] = match code {
        0 => b"ok\0",
        ERR_NULL => b"null pointer\0",
        ERR_UTF8 => b"invalid UTF-8\0",
        ERR_FEN => b"invalid FEN\0",
        ERR_MOVE => b"illegal move\0",
        ERR_OPTION => b"unknown option\0",
        ERR_VALUE => b"bad option value\0",
        ERR_PANIC => b"internal panic\0",
        _ => b"unknown error\0",
    };
    s.as_ptr() as *const c_char
}

// ---------------------------------------------------------------- lifetime

/// Create an engine handle. Returns null on allocation failure.
#[no_mangle]
pub extern "C" fn ferrum_create() -> *mut FerrumEngine {
    catch_unwind(AssertUnwindSafe(|| Box::into_raw(Box::new(FerrumEngine::new()))))
        .unwrap_or(std::ptr::null_mut())
}

/// Destroy an engine handle. Null is ignored.
#[no_mangle]
pub unsafe extern "C" fn ferrum_destroy(engine: *mut FerrumEngine) {
    if engine.is_null() {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| unsafe {
        drop(Box::from_raw(engine));
    }));
}

/// Reset to the start position and clear the transposition table.
#[no_mangle]
pub unsafe extern "C" fn ferrum_reset(engine: *mut FerrumEngine) -> i32 {
    guard_int(|| {
        let Some(e) = (unsafe { engine.as_mut() }) else {
            return ERR_NULL;
        };
        e.reset();
        0
    })
}

// ---------------------------------------------------------------- position

/// Set the position. `fen` is either `NULL`/`""`/`"startpos"` or a strict
/// FEN; `moves` is a space-separated list of UCI moves (may be `NULL`).
///
/// The update is atomic: on an invalid FEN or illegal move the previous
/// position is kept and an error code is returned.
#[no_mangle]
pub unsafe extern "C" fn ferrum_set_position(
    engine: *mut FerrumEngine,
    fen: *const c_char,
    moves: *const c_char,
) -> i32 {
    guard_int(|| {
        let Some(e) = (unsafe { engine.as_mut() }) else {
            return ERR_NULL;
        };
        let fen = match unsafe { opt_str(fen) } {
            Ok(f) => f.unwrap_or(""),
            Err(c) => return c,
        };
        let moves = match unsafe { opt_str(moves) } {
            Ok(m) => m.unwrap_or(""),
            Err(c) => return c,
        };
        let mut board = if fen.is_empty() || fen.eq_ignore_ascii_case("startpos") {
            Board::startpos()
        } else {
            match Board::from_fen_strict(fen) {
                Ok(b) => b,
                Err(_) => return ERR_FEN,
            }
        };
        let mut hashes = vec![board.hash];
        for tok in moves.split_whitespace() {
            match board.find_legal(tok) {
                Some(m) => {
                    board.make_move(&m);
                    hashes.push(board.hash);
                }
                None => return ERR_MOVE,
            }
        }
        e.board = board;
        e.game_hashes = hashes;
        0
    })
}

/// Replace the position from a FEN only (no move list).
#[no_mangle]
pub unsafe extern "C" fn ferrum_set_fen(engine: *mut FerrumEngine, fen: *const c_char) -> i32 {
    ferrum_set_position(engine, fen, std::ptr::null())
}

// ----------------------------------------------------------------- options

/// Set an engine option, mirroring UCI names: `Hash`, `Threads`,
/// `EvalParams`, `SearchParams`, `Clear Hash`.
#[no_mangle]
pub unsafe extern "C" fn ferrum_set_option(
    engine: *mut FerrumEngine,
    name: *const c_char,
    value: *const c_char,
) -> i32 {
    guard_int(|| {
        let Some(e) = (unsafe { engine.as_mut() }) else {
            return ERR_NULL;
        };
        let name = match unsafe { req_str(name) } {
            Ok(n) => n,
            Err(c) => return c,
        };
        let value = match unsafe { opt_str(value) } {
            Ok(v) => v.unwrap_or(""),
            Err(c) => return c,
        };
        if name.eq_ignore_ascii_case("hash") {
            match value.parse::<usize>() {
                Ok(v) => {
                    e.hash_mb = v.clamp(1, MAX_HASH_MB);
                    e.tt.resize(e.hash_mb);
                    0
                }
                Err(_) => ERR_VALUE,
            }
        } else if name.eq_ignore_ascii_case("threads") {
            match value.parse::<usize>() {
                Ok(v) => {
                    e.threads = v.clamp(1, FFI_MAX_THREADS);
                    0
                }
                Err(_) => ERR_VALUE,
            }
        } else if name.eq_ignore_ascii_case("evalparams") {
            if value.is_empty() || value.eq_ignore_ascii_case("default") {
                e.eval = EvalParams::DEFAULT;
                return 0;
            }
            match EvalParams::from_spec(value) {
                Ok(p) => {
                    e.eval = p;
                    0
                }
                Err(_) => ERR_VALUE,
            }
        } else if name.eq_ignore_ascii_case("searchparams") {
            if value.is_empty() || value.eq_ignore_ascii_case("default") {
                e.search = SearchParams::DEFAULT;
                return 0;
            }
            match SearchParams::from_spec(value) {
                Ok(p) => {
                    e.search = p;
                    0
                }
                Err(_) => ERR_VALUE,
            }
        } else if name.eq_ignore_ascii_case("clear hash") {
            e.tt = TranspositionTable::new(e.hash_mb);
            0
        } else {
            ERR_OPTION
        }
    })
}

// ------------------------------------------------------------------ search

fn limits_from(movetime_ms: u64, depth: i32, nodes: u64) -> SearchLimits {
    let max_depth = if depth > 0 { depth } else { 128 };
    if nodes > 0 {
        SearchLimits {
            time_ms: None,
            nodes: Some(nodes),
            depth: max_depth,
            infinite: false,
        }
    } else if movetime_ms > 0 {
        SearchLimits {
            time_ms: Some(movetime_ms),
            nodes: None,
            depth: max_depth,
            infinite: false,
        }
    } else {
        // No limit supplied: a bounded default so a bot cannot hang.
        SearchLimits {
            time_ms: None,
            nodes: None,
            depth: if depth > 0 { depth } else { 8 },
            infinite: false,
        }
    }
}

fn search_handle(e: &mut FerrumEngine, limits: &SearchLimits) -> SearchInfo {
    let threads = e.threads.clamp(1, FFI_MAX_THREADS);
    if threads <= 1 {
        let mut s = Searcher::with_settings(e.tt.clone(), e.eval, e.search);
        return s.search(&mut e.board, &e.game_hashes, limits, None, &|_| {});
    }
    // Lazy SMP for embedded multi-thread searches: helpers warm the shared
    // TT, the calling thread runs the main search, then the scope joins.
    let stop = Arc::new(AtomicBool::new(false));
    std::thread::scope(|scope| {
        for h in 0..(threads - 1) {
            let tt = e.tt.clone();
            let mut board = e.board.clone();
            let history = e.game_hashes.clone();
            let flag = stop.clone();
            let lim = SearchLimits {
                time_ms: limits.time_ms,
                nodes: None,
                depth: (limits.depth - 1 - h as i32).max(1),
                infinite: limits.infinite,
            };
            let params = e.eval;
            let sparams = e.search;
            scope.spawn(move || {
                let mut s = Searcher::with_settings(tt, params, sparams);
                let _ = s.search(&mut board, &history, &lim, Some(flag), &|_| {});
            });
        }
        let mut s = Searcher::with_settings(e.tt.clone(), e.eval, e.search);
        let info = s.search(&mut e.board, &e.game_hashes, limits, Some(stop.clone()), &|_| {});
        stop.store(true, Ordering::Relaxed);
        info
    })
}

/// Run a search and return the best move as a UCI string (allocated; free
/// with [`ferrum_string_free`]). Returns `"(none)"` when there is no legal
/// move. Null on a null handle/panic.
#[no_mangle]
pub unsafe extern "C" fn ferrum_go(
    engine: *mut FerrumEngine,
    movetime_ms: u64,
    depth: i32,
    nodes: u64,
    threads: i32,
) -> *mut c_char {
    guard_ptr(|| {
        let Some(e) = (unsafe { engine.as_mut() }) else {
            return std::ptr::null_mut();
        };
        if threads > 0 {
            e.threads = (threads as usize).clamp(1, FFI_MAX_THREADS);
        }
        let limits = limits_from(movetime_ms, depth, nodes);
        let info = search_handle(e, &limits);
        let mv = info.best_move.map(|m| m.to_uci()).unwrap_or_else(|| "(none)".to_string());
        to_c_string(mv)
    })
}

/// Run a search and return a JSON object (allocated; free with
/// [`ferrum_string_free`]):
/// `{"bestmove":"e2e4","score":34,"mate":null,"depth":9,"nodes":12345,
///   "time_ms":50,"pv":["e2e4","e7e5"]}`
///
/// `score` is centipawns from the side to move; `mate` is the number of
/// moves to mate (positive = engine mates) or `null`.
#[no_mangle]
pub unsafe extern "C" fn ferrum_go_json(
    engine: *mut FerrumEngine,
    movetime_ms: u64,
    depth: i32,
    nodes: u64,
    threads: i32,
) -> *mut c_char {
    guard_ptr(|| {
        let Some(e) = (unsafe { engine.as_mut() }) else {
            return std::ptr::null_mut();
        };
        if threads > 0 {
            e.threads = (threads as usize).clamp(1, FFI_MAX_THREADS);
        }
        let limits = limits_from(movetime_ms, depth, nodes);
        let info = search_handle(e, &limits);
        let best = info.best_move.map(|m| m.to_uci()).unwrap_or_else(|| "(none)".to_string());
        let mate = if info.score.abs() > MATE - 1000 {
            let n = if info.score > 0 {
                (MATE - info.score + 1) / 2
            } else {
                -((MATE + info.score + 1) / 2)
            };
            n.to_string()
        } else {
            "null".to_string()
        };
        let pv = info
            .pv
            .iter()
            .map(|m| format!("\"{}\"", m.to_uci()))
            .collect::<Vec<_>>()
            .join(",");
        to_c_string(format!(
            "{{\"bestmove\":\"{best}\",\"score\":{},\"mate\":{mate},\"depth\":{},\"nodes\":{},\"time_ms\":{},\"pv\":[{pv}]}}",
            info.score, info.depth, info.nodes, info.time_ms
        ))
    })
}

// -------------------------------------------------------------------- eval

/// Static evaluation in centipawns from the side to move.
///
/// Returns `i32::MIN` on a null handle or internal panic (no legal position
/// evaluates anywhere near that value).
#[no_mangle]
pub unsafe extern "C" fn ferrum_evaluate(engine: *mut FerrumEngine) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(e) = (unsafe { engine.as_ref() }) else {
            return i32::MIN;
        };
        evaluate_with(&e.board, &e.eval)
    }))
    .unwrap_or(i32::MIN)
}

/// Best move for the current position without searching (null if none):
/// a cheap legal-move probe for bots that only need a fallback.
#[no_mangle]
pub unsafe extern "C" fn ferrum_legal_move(engine: *mut FerrumEngine) -> *mut c_char {
    guard_ptr(|| {
        let Some(e) = (unsafe { engine.as_mut() }) else {
            return std::ptr::null_mut();
        };
        let legal = e.board.generate_legal_moves();
        match legal.as_slice().first() {
            Some(m) => to_c_string(m.to_uci()),
            None => to_c_string("(none)".to_string()),
        }
    })
}

/// Current position as a FEN (allocated; free with [`ferrum_string_free`]).
#[no_mangle]
pub unsafe extern "C" fn ferrum_fen(engine: *mut FerrumEngine) -> *mut c_char {
    guard_ptr(|| {
        let Some(e) = (unsafe { engine.as_ref() }) else {
            return std::ptr::null_mut();
        };
        to_c_string(e.board.to_fen())
    })
}

/// Free a string returned by this module. Null is ignored.
#[no_mangle]
pub unsafe extern "C" fn ferrum_string_free(s: *mut c_char) {
    if s.is_null() {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| unsafe {
        drop(CString::from_raw(s));
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(s: &str) -> CString {
        CString::new(s).unwrap()
    }

    #[test]
    fn handle_roundtrip_and_search() {
        let e = ferrum_create();
        assert!(!e.is_null());
        let fen = c("startpos");
        let moves = c("e2e4 e7e5");
        unsafe {
            assert_eq!(ferrum_set_position(e, fen.as_ptr(), moves.as_ptr()), 0);
            let json = ferrum_go_json(e, 0, 3, 0, 1);
            assert!(!json.is_null());
            let text = CStr::from_ptr(json).to_str().unwrap().to_string();
            ferrum_string_free(json);
            assert!(text.contains("\"bestmove\":\""));
            assert!(text.contains("\"depth\":"));
            ferrum_destroy(e);
        }
    }

    #[test]
    fn invalid_fen_and_moves_are_rejected_atomically() {
        unsafe {
            let e = ferrum_create();
            let bad = c("8/8/8/8/8/8/8/8 w - - 0 1");
            assert_eq!(ferrum_set_fen(e, bad.as_ptr()), ERR_FEN);
            let start = c("startpos");
            let illegal = c("e2e5");
            assert_eq!(ferrum_set_position(e, start.as_ptr(), illegal.as_ptr()), ERR_MOVE);
            // A rejected update must leave the previous (start) position intact.
            let fen_out = ferrum_fen(e);
            let fen = CStr::from_ptr(fen_out).to_str().unwrap().to_string();
            ferrum_string_free(fen_out);
            assert!(fen.starts_with("rnbqkbnr/pppppppp"));
            ferrum_destroy(e);
        }
    }

    #[test]
    fn options_and_eval_work() {
        unsafe {
            let e = ferrum_create();
            let name = c("EvalParams");
            let value = c("tempo=100");
            assert_eq!(ferrum_set_option(e, name.as_ptr(), value.as_ptr()), 0);
            assert_eq!(ferrum_evaluate(e), 100);
            let bad = c("nonsense=1");
            assert_eq!(ferrum_set_option(e, name.as_ptr(), bad.as_ptr()), ERR_VALUE);
            let unknown = c("Nope");
            assert_eq!(ferrum_set_option(e, unknown.as_ptr(), value.as_ptr()), ERR_OPTION);
            ferrum_destroy(e);
        }
    }

    #[test]
    fn handles_are_independent_across_threads() {
        // Server pattern: one handle per worker, searched concurrently. The
        // shared read-only tables are OnceLock-initialized; nothing else is
        // global, so this must be race-free.
        std::thread::scope(|scope| {
            for _ in 0..4 {
                scope.spawn(|| unsafe {
                    let e = ferrum_create();
                    assert!(!e.is_null());
                    assert_eq!(ferrum_set_position(e, std::ptr::null(), std::ptr::null()), 0);
                    let json = ferrum_go_json(e, 0, 4, 0, 1);
                    assert!(!json.is_null());
                    let text = CStr::from_ptr(json).to_str().unwrap().to_string();
                    ferrum_string_free(json);
                    assert!(text.contains("\"bestmove\":\""));
                    ferrum_destroy(e);
                });
            }
        });
    }

    #[test]
    fn mate_is_reported_in_json() {
        unsafe {
            let e = ferrum_create();
            let fen = c("r1bqkbnr/pppp1ppp/2n5/4p2Q/2B1P3/8/PPPP1PPP/RNB1K1NR w KQkq - 0 1");
            assert_eq!(ferrum_set_fen(e, fen.as_ptr()), 0);
            let json = ferrum_go_json(e, 0, 4, 0, 1);
            let text = CStr::from_ptr(json).to_str().unwrap().to_string();
            ferrum_string_free(json);
            assert!(text.contains("\"bestmove\":\"h5f7\""), "{text}");
            assert!(text.contains("\"mate\":1"), "{text}");
            ferrum_destroy(e);
        }
    }

    #[test]
    fn null_handles_are_safe() {
        unsafe {
            let null = std::ptr::null_mut();
            assert_eq!(ferrum_reset(null), ERR_NULL);
            assert_eq!(ferrum_evaluate(null), i32::MIN);
            assert_eq!(ferrum_set_fen(null, std::ptr::null()), ERR_NULL);
            ferrum_destroy(null);
            ferrum_string_free(std::ptr::null_mut());
        }
    }
}
