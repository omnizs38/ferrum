#!/usr/bin/env python3
"""Build a (FEN, side-to-move) corpus from all gauntlet games. Replays each
game with ferrum's own UCI `d` command for FENs. Dedupes by position key."""
import json, glob, subprocess

def ferrum_positions():
    p = subprocess.Popen(["target/release/ferrum-gamma.exe"], stdin=subprocess.PIPE,
                         stdout=subprocess.PIPE, text=True)
    def send(cmd):
        p.stdin.write(cmd + "\n"); p.stdin.flush()
    def wait_line(pred, timeout=10):
        import time
        start = time.time()
        while time.time() - start < timeout:
            line = p.stdout.readline()
            if not line:
                continue
            line = line.rstrip()
            if pred(line):
                return line
        return None
    send("uci"); wait_line(lambda l: l == "uciok")
    send("isready"); wait_line(lambda l: l == "readyok")
    positions = {}  # key -> fen
    games = 0
    for f in sorted(glob.glob("results/*.json")):
        d = json.load(open(f))
        for r in d.get("results", []):
            games += 1
            moves = r["moves"]
            outcome = r["result"]
            if outcome not in ("1-0", "0-1", "1/2-1/2"):
                continue
            for i in range(1, len(moves) + 1):
                mv = " ".join(moves[:i])
                send(f"position startpos moves {mv}")
                send("d")
                fen = wait_line(lambda l: ("/" in l and (" w " in l or " b " in l)))
                if fen is None:
                    continue
                # position key = placement + side + castling + ep (ignore clocks)
                key = " ".join(fen.split()[:4])
                if key not in positions:
                    positions[key] = fen
    p.kill()
    print(f"games={games} unique_positions={len(positions)}")
    return positions

if __name__ == "__main__":
    positions = ferrum_positions()
    with open("results/texel_corpus.txt", "w") as fh:
        for key, fen in positions.items():
            fh.write(fen + "\n")
    print("wrote results/texel_corpus.txt")
