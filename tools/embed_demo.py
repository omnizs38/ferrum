#!/usr/bin/env python3
"""Load the Ferrum shared library in-process (server/bot integration demo).

This is the "server-side loading" path: no subprocess, no stdio pipes. The
same pattern works from Node (`ffi-napi`), Java (JNA), Go (cgo) or any
language with a C FFI.

Usage:
    python tools/embed_demo.py
    python tools/embed_demo.py --fen "r1bqkbnr/pppp1ppp/2n5/4p2Q/2B1P3/8/PPPP1PPP/RNB1K1NR w KQkq - 0 1" --movetime 200
    python tools/embed_demo.py --lib target/release/libferrum.so
"""
from __future__ import annotations

import argparse
import ctypes
import json
import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent


def library_path(explicit: str | None) -> pathlib.Path:
    if explicit:
        return pathlib.Path(explicit)
    names = ["ferrum.dll", "libferrum.so", "libferrum.dylib"]
    for name in names:
        p = ROOT / "target" / "release" / name
        if p.exists():
            return p
    raise SystemExit(
        "no Ferrum shared library found; run `cargo build --release` first "
        "(expected ferrum.dll / libferrum.so / libferrum.dylib)"
    )


class Engine:
    def __init__(self, lib: ctypes.CDLL):
        self.lib = lib
        lib.ferrum_create.restype = ctypes.c_void_p
        lib.ferrum_destroy.argtypes = [ctypes.c_void_p]
        lib.ferrum_name.restype = ctypes.c_char_p
        lib.ferrum_version.restype = ctypes.c_char_p
        lib.ferrum_error_string.argtypes = [ctypes.c_int]
        lib.ferrum_error_string.restype = ctypes.c_char_p
        lib.ferrum_set_position.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_char_p]
        lib.ferrum_set_position.restype = ctypes.c_int
        lib.ferrum_set_option.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_char_p]
        lib.ferrum_set_option.restype = ctypes.c_int
        lib.ferrum_go_json.argtypes = [
            ctypes.c_void_p, ctypes.c_uint64, ctypes.c_int32, ctypes.c_uint64, ctypes.c_int32
        ]
        lib.ferrum_go_json.restype = ctypes.c_void_p
        lib.ferrum_go.argtypes = [
            ctypes.c_void_p, ctypes.c_uint64, ctypes.c_int32, ctypes.c_uint64, ctypes.c_int32
        ]
        lib.ferrum_go.restype = ctypes.c_void_p
        lib.ferrum_evaluate.argtypes = [ctypes.c_void_p]
        lib.ferrum_evaluate.restype = ctypes.c_int32
        lib.ferrum_fen.argtypes = [ctypes.c_void_p]
        lib.ferrum_fen.restype = ctypes.c_void_p
        lib.ferrum_string_free.argtypes = [ctypes.c_void_p]
        self.handle = lib.ferrum_create()
        if not self.handle:
            raise RuntimeError("ferrum_create returned NULL")

    def close(self):
        if self.handle:
            self.lib.ferrum_destroy(self.handle)
            self.handle = None

    def _take(self, ptr) -> str:
        if not ptr:
            raise RuntimeError("engine returned NULL (see ferrum_error_string)")
        try:
            return ctypes.string_at(ptr).decode("utf-8")
        finally:
            self.lib.ferrum_string_free(ptr)

    def set_position(self, fen: str | None, moves: str | None) -> int:
        return self.lib.ferrum_set_position(
            self.handle,
            fen.encode() if fen else None,
            moves.encode() if moves else None,
        )

    def set_option(self, name: str, value: str | None = None) -> int:
        return self.lib.ferrum_set_option(
            self.handle, name.encode(), value.encode() if value else None
        )

    def go_json(self, movetime_ms=100, depth=0, nodes=0, threads=1) -> dict:
        return json.loads(self._take(
            self.lib.ferrum_go_json(self.handle, movetime_ms, depth, nodes, threads)
        ))

    def go(self, movetime_ms=100, depth=0, nodes=0, threads=1) -> str:
        return self._take(self.lib.ferrum_go(self.handle, movetime_ms, depth, nodes, threads))

    def evaluate(self) -> int:
        return int(self.lib.ferrum_evaluate(self.handle))

    def fen(self) -> str:
        return self._take(self.lib.ferrum_fen(self.handle))


def main() -> int:
    p = argparse.ArgumentParser(description=__doc__,
                                formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--lib", default=None, help="path to the shared library")
    p.add_argument("--fen", default=None, help="strict FEN (default: startpos)")
    p.add_argument("--moves", default="", help="space-separated UCI moves")
    p.add_argument("--movetime", type=int, default=100, help="ms per search")
    p.add_argument("--depth", type=int, default=0, help="fixed depth instead")
    args = p.parse_args()

    lib_path = library_path(args.lib)
    lib = ctypes.CDLL(str(lib_path))
    print(f"loaded {lib_path}")
    e = Engine(lib)
    try:
        name = lib.ferrum_name().decode()
        version = lib.ferrum_version().decode()
        print(f"engine: {name} {version}")

        rc = e.set_option("Hash", "16")
        print(f"setoption Hash=16 -> {rc}")
        rc = e.set_option("EvalParams", "default")
        print(f"setoption EvalParams=default -> {rc}")

        rc = e.set_position(args.fen, args.moves)
        print(f"set_position -> {rc}")
        if rc != 0:
            print(f"  error: {lib.ferrum_error_string(rc).decode()}", file=sys.stderr)
            return 2
        print(f"fen: {e.fen()}")
        print(f"eval: {e.evaluate()} cp")

        info = e.go_json(args.movetime, args.depth, 0, 1)
        print("search:", json.dumps(info))
        best = e.go(args.movetime, args.depth, 0, 1)
        print("ferrum_go:", best)

        # Bad input must be rejected without corrupting the position.
        rc = e.set_position("8/8/8/8/8/8/8/8 w - - 0 1", None)
        print(f"bad FEN rejected with {rc} ({lib.ferrum_error_string(rc).decode()})")
        print(f"fen after rejection: {e.fen()}")
        return 0 if rc == -3 else 1
    finally:
        e.close()


if __name__ == "__main__":
    raise SystemExit(main())
