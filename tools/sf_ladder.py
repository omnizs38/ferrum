#!/usr/bin/env python3
"""Run the Stockfish ladder: one gauntlet per ``UCI_Elo`` anchor.

Ferrum plays Stockfish 19 with Stockfish's own ``UCI_LimitStrength`` +
``UCI_Elo`` handicap (the anchor used throughout ELO.md). This wrapper runs
one fixed-``movetime`` match per anchor, prints a compact table, and keeps
each raw JSON under ``results/``.

Example::

    python tools/sf_ladder.py --engine target/release/ferrum-final.exe \
        --elos 1800,2000,2200 --games 40 --movetime 300 --threads 4
"""

import argparse
import json
import os
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)


def run_match(args, elo):
    out = os.path.join(
        args.results,
        f"ladder_{os.path.basename(args.engine).replace('.exe', '')}_{elo}_{args.movetime}ms_{args.games}g.json",
    )
    cmd = [
        sys.executable,
        os.path.join(HERE, "gauntlet.py"),
        "--white", args.engine,
        "--black", args.stockfish,
        "--games", str(args.games),
        "--movetime", str(args.movetime),
        "--threads-w", str(args.threads),
        "--elo-b", str(elo),
        "--oracle", args.engine,
        "--out", out,
    ]
    print("$ " + " ".join(cmd), flush=True)
    proc = subprocess.run(cmd, cwd=ROOT, text=True, capture_output=True)
    tail = "\n".join(proc.stdout.strip().splitlines()[-4:])
    print(tail, flush=True)
    if proc.returncode != 0:
        print(proc.stderr[-2000:], flush=True)
        return None
    with open(os.path.join(ROOT, out), "r", encoding="utf-8") as fh:
        return json.load(fh)["summary"]


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--engine", required=True, help="Ferrum binary (A side)")
    p.add_argument("--stockfish", default="stockfish.exe")
    p.add_argument("--elos", default="1800,2000,2200", help="comma-separated UCI_Elo anchors")
    p.add_argument("--games", type=int, default=40)
    p.add_argument("--movetime", type=int, default=300)
    p.add_argument("--threads", type=int, default=4)
    p.add_argument("--results", default="results")
    args = p.parse_args()

    rows = []
    for elo in [int(x) for x in args.elos.split(",") if x.strip()]:
        summary = run_match(args, elo)
        if summary:
            rows.append((elo, summary))

    print("\n=== Stockfish ladder ===")
    print(f"{'SF UCI_Elo':>10}  {'games':>5}  {'A W/D/L':>10}  {'score':>6}  {'Elo(A-B)':>9}  95% CI")
    for elo, s in rows:
        ci = f"[{s['elo_lo']:.0f}, {s['elo_hi']:.0f}]"
        print(
            f"{elo:>10}  {s['total']:>5}  "
            f"{s['a_wins']}/{s['draws']}/{s['b_wins']:<4}  {s['p_A']:.3f}  "
            f"{s['elo_A_minus_B']:>+9.0f}  {ci}"
        )


if __name__ == "__main__":
    main()
