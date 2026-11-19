#!/usr/bin/env python3
"""Measure UCI engine search speed (NPS) fairly.

Two modes:

* ``--nodes N`` (recommended): both engines search the same number of nodes;
  the metric is how long that takes (and the NPS implied by it). This is a
  pure speed comparison: work is identical, only time differs.
* ``--movetime MS``: both engines search for the same clock; the metric is
  nodes per second and the depth reached.

Runs are **interleaved per position** (A then B, or B then A on alternate
positions) so CPU frequency drift affects both engines equally. Each engine
gets one thread and the same hash unless overridden.

Usage:
    python tools/speed.py --engine target/release/ferrum.exe --nodes 1000000
    python tools/speed.py --engine ferrum-native.exe --compare stockfish.exe --nodes 1000000
    python tools/speed.py --engine ferrum.exe --compare stockfish.exe --movetime 1000
"""
from __future__ import annotations

import argparse
import json
import statistics
import subprocess
import threading
import time

POSITIONS = [
    ("startpos", "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1"),
    ("kiwipete", "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1"),
    ("middlegame", "r1bq1rk1/pp2bppp/2n1pn2/2pp4/3P1B2/2PBPN2/PP1N1PPP/R2Q1RK1 w - - 0 9"),
    ("endgame", "8/5pk1/6p1/8/3P4/5K2/6P1/8 w - - 0 40"),
]


class Uci:
    def __init__(self, path: str, threads: int, hash_mb: int, options: list[str] | None = None):
        self.label = path
        self.proc = subprocess.Popen(
            [path], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL, text=True, bufsize=1,
        )
        self.lines: list[str] = []
        self.lock = threading.Lock()
        self.reader = threading.Thread(target=self._pump, daemon=True)
        self.reader.start()
        self.send("uci")
        self.wait("uciok")
        self.send(f"setoption name Threads value {threads}")
        self.send(f"setoption name Hash value {hash_mb}")
        for opt in options or []:
            name, _, value = opt.partition("=")
            self.send(f"setoption name {name} value {value}".rstrip())
        self.send("isready")
        self.wait("readyok")
        # Warm up (CPU frequency, caches, lazy tables) before measuring.
        self.run_search(POSITIONS[0][1], "go movetime 300")

    def _pump(self):
        assert self.proc.stdout is not None
        for line in self.proc.stdout:
            with self.lock:
                self.lines.append(line.strip())

    def send(self, cmd: str):
        assert self.proc.stdin is not None
        self.proc.stdin.write(cmd + "\n")
        self.proc.stdin.flush()

    def wait(self, prefix: str, timeout: float = 60.0) -> str:
        end = time.time() + timeout
        while time.time() < end:
            with self.lock:
                for i, line in enumerate(self.lines):
                    if line.startswith(prefix):
                        return self.lines.pop(i)
            time.sleep(0.002)
        raise TimeoutError(f"{self.label}: no {prefix!r}")

    def take_info(self) -> list[str]:
        with self.lock:
            out = [l for l in self.lines if l.startswith("info ")]
            self.lines = [l for l in self.lines if not l.startswith("info ")]
            return out

    def clear(self):
        with self.lock:
            self.lines.clear()

    def run_search(self, fen: str, limit_cmd: str, fixed_nodes: int | None = None) -> dict:
        self.clear()
        self.send("ucinewgame")
        self.send("isready")
        self.wait("readyok")
        self.send("position fen " + fen)
        t0 = time.perf_counter()
        self.send(limit_cmd)
        self.wait("bestmove")
        wall_ms = (time.perf_counter() - t0) * 1000.0
        infos = [parse_info(l) for l in self.take_info()]
        best = {}
        for info in infos:
            if "nodes" in info and "nps" in info:
                best = info
        if fixed_nodes:
            # The final (partial) iteration may not be printed, so use the
            # wall clock for the whole node-limited search.
            nodes = fixed_nodes
            ms = wall_ms
            nps = int(nodes * 1000.0 / ms) if ms > 0 else 0
        else:
            nodes = int(best.get("nodes", 0))
            engine_ms = int(best.get("time", 0))
            # Prefer the engine's own elapsed time; fall back to wall clock.
            ms = engine_ms if engine_ms > 0 else wall_ms
            nps = int(nodes * 1000.0 / ms) if ms > 0 and nodes else 0
        return {
            "depth": int(best.get("depth", 0)),
            "nodes": nodes,
            "nps": nps,
            "ms": int(ms),
            "wall_ms": int(wall_ms),
        }

    def quit(self):
        try:
            self.send("quit")
            self.proc.wait(timeout=5)
        except Exception:
            self.proc.kill()


def parse_info(line: str) -> dict:
    toks = line.split()
    out: dict[str, object] = {}
    for i, t in enumerate(toks):
        if t in ("depth", "nodes", "nps", "time") and i + 1 < len(toks):
            try:
                out[t] = int(toks[i + 1])
            except ValueError:
                pass
    return out


def summarise(rows: list[dict]) -> dict:
    nps = [r["nps"] for r in rows if r["nps"]]
    return {
        "nps_median": int(statistics.median(nps)) if nps else 0,
        "nps_mean": int(sum(nps) / len(nps)) if nps else 0,
        "depth_sum": sum(r["depth"] for r in rows),
        "rows": rows,
    }


def main() -> int:
    p = argparse.ArgumentParser(description=__doc__,
                                formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--engine", required=True)
    p.add_argument("--compare", default=None)
    g = p.add_mutually_exclusive_group()
    g.add_argument("--nodes", type=int, default=None, help="fixed node budget (recommended)")
    g.add_argument("--movetime", type=int, default=None, help="fixed ms per position")
    p.add_argument("--threads", type=int, default=1)
    p.add_argument("--hash", type=int, default=64)
    p.add_argument("--set", action="append", default=[],
                   help="UCI option as name=value (repeatable), applied to every engine")
    p.add_argument("--json", default=None)
    args = p.parse_args()
    if args.nodes is None and args.movetime is None:
        args.movetime = 1000
    limit_cmd = f"go nodes {args.nodes}" if args.nodes else f"go movetime {args.movetime}"

    engines: dict[str, Uci] = {
        "A": Uci(args.engine, args.threads, args.hash, args.set),
    }
    if args.compare:
        engines["B"] = Uci(args.compare, args.threads, args.hash, args.set)

    rows: dict[str, list[dict]] = {k: [] for k in engines}
    try:
        for idx, (name, fen) in enumerate(POSITIONS):
            order = ["A", "B"] if idx % 2 == 0 else ["B", "A"]
            for label in order:
                if label not in engines:
                    continue
                r = engines[label].run_search(fen, limit_cmd, args.nodes)
                r["position"] = name
                rows[label].append(r)
    finally:
        for e in engines.values():
            e.quit()

    results = {}
    for label, path in (("A", args.engine), ("B", args.compare)):
        if label not in rows:
            continue
        s = summarise(rows[label])
        s["engine"] = path
        results[label] = s
        print(f"\n{label}: {path}   ({limit_cmd})")
        print(f"  {'position':<12} {'depth':>5} {'nodes':>10} {'nps':>10} {'ms':>6}")
        for r in s["rows"]:
            print(f"  {r['position']:<12} {r['depth']:>5} {r['nodes']:>10} {r['nps']:>10} {r['ms']:>6}")
        print(f"  median nps={s['nps_median']:,}  depth sum={s['depth_sum']}")

    if "B" in results:
        a, b = results["A"], results["B"]
        if b["nps_median"]:
            print(f"\nNPS ratio A/B = {a['nps_median'] / b['nps_median']:.2f}x")
        if args.nodes:
            print(f"median time A={statistics.median([r['ms'] for r in a['rows']]):.0f} ms, "
                  f"B={statistics.median([r['ms'] for r in b['rows']]):.0f} ms "
                  f"for the same {args.nodes:,} nodes")
    if args.json:
        import pathlib
        pathlib.Path(args.json).parent.mkdir(parents=True, exist_ok=True)
        pathlib.Path(args.json).write_text(json.dumps(results, indent=2))
        print(f"wrote {args.json}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
