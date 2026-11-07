#!/usr/bin/env python3
"""Evaluate a battery of positions with both engines (UCI `eval`) and diff them.

Includes opening traps (material-halving motifs where tactical vision decides),
quiet positions, sharp midgames, and endgames.
"""
import subprocess, sys

ENGINES = ["target/release/ferrum-gamma.exe", "target/release/ferrum-alpha.exe"]

POSITIONS = [
    ("scholar-setup (Qh5/Bc4 danger)", "r1bqkbnr/pppp1ppp/2n5/4p2Q/2B1P3/8/PPPP1PPP/RNB1K1NR w KQkq - 0 1"),
    ("Fried-liver/trap motif",          "r1bqkb1r/ppppnppp/2n2p2/3Np3/3P4/2N1P3/PPP2PPP/R1BQKB1R w KQkq - 0 1"),
    ("early-gambit trap (Rxa1 trap)",   "rnb1kbnr/ppp1pppp/8/3pP2q/3P4/8/PPP2PPP/RNBQKBNR w KQkq - 0 1"),
    ("center, white solid",             "rnbqkbnr/pppp1ppp/8/4p3/4P3/5N2/PPPP1PPP/RNBQKB1R b KQkq - 1 2"),
    ("isolated-queen-pawn sharp",       "r1bq1rk1/pp3ppp/2n2n2/2bp4/3pP3/2P2N2/P2B1PPP/RNBQ1RK1 w - - 0 15"),
    ("hanging rook bait",               "r2qk2r/ppp2ppp/2np1n2/1B2p3/P3P3/2N2Q1P/1PP2P1P/R3R1K1 w kq - 0 18"),
    ("advanced passer (avoid grab)",    "8/1P6/8/8/8/8/6k1/K7 w - - 0 1"),
    ("king safety crisis",              "r1bq1rk1/ppp2ppp/2n5/3Pp3/1P1P4/2P1Q3/P3BPPP/R3R1K1 b - - 0 16"),
    ("equal endgame",                   "8/5k2/8/8/1P6/2K5/8/8 w - - 0 1"),
    ("trap-y pin + fork",               "r1bqk2r/ppp2ppp/2np1n2/2Bpp3/2B5/3P1N2/PPP2PPP/RNBQR1K1 w kq - 0 9"),
]

def fen_eval(exe, fen):
    p = subprocess.Popen([exe], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
    p.stdin.write("isready\n"); p.stdin.flush()
    p.stdin.write(f"position fen {fen}\n"); p.stdin.flush()
    p.stdin.write("eval\n"); p.stdin.flush()
    p.stdin.write("d\n"); p.stdin.flush()
    result = {}
    for _ in range(20):
        line = p.stdout.readline().rstrip()
        if line.startswith("eval "):
            result["eval"] = int(line.split()[1])
        elif " / " in line and " " in line and line.count("/") >= 7 and (" w " in line or " b " in line) and "eval" not in result:
            result["fen"] = line
        if "eval" in result:
            break
    p.stdin.write("quit\n"); p.stdin.flush()
    try:
        p.wait(timeout=3)
    except Exception:
        p.kill()
    return result

print(f"{'position':34s} {'Gamma':>6} {'Alpha':>6} {'G-A':>6}")
for name, fen in POSITIONS:
    g = fen_eval(ENGINES[0], fen).get("eval")
    a = fen_eval(ENGINES[1], fen).get("eval")
    diff = g - a if g is not None and a is not None else "?"
    print(f"{name:34s} {g!s:>6} {a!s:>6} {diff:>6}")
