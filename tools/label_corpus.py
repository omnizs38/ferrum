#!/usr/bin/env python3
"""Label the texel corpus with Stockfish scores (depth 10, UCI_Elo 3500 cap).
Reads results/texel_corpus.txt, writes binary q3500 engine-vs-engine game scores
(results/texel_corpus.jsonl, one JSON object per line, resumable)."""
import subprocess, json, os, re, sys

CORPUS = "results/texel_corpus.txt"
OUT = "results/texel_corpus.jsonl"
DEPTH = 6

def label():
    positions = [l.rstrip() for l in open(CORPUS).read().splitlines() if l.strip()]
    done = set()
    if os.path.exists(OUT):
        with open(OUT) as fh:
            for line in fh:
                try:
                    done.add(json.loads(line)["fen"])
                except Exception:
                    pass
    todo = [f for f in positions if f not in done]
    print(f"total={len(positions)} done={len(done)} todo={len(todo)}")
    if not todo:
        return
    p = subprocess.Popen(["stockfish.exe"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
    def send(c):
        p.stdin.write(c + "\n"); p.stdin.flush()
    def wait(prefix, timeout=30):
        import time
        start = time.time()
        while time.time() - start < timeout:
            line = p.stdout.readline()
            if not line:
                continue
            if line.startswith(prefix):
                return line
        return None
    send("uci"); wait("uciok")
    send("setoption name UCI_LimitStrength value true")
    send("setoption name UCI_Elo value 3500")
    send("isready"); wait("readyok")
    n = 0
    with open(OUT, "a") as out:
        for i, fen in enumerate(todo):
            send("position fen " + fen)
            send(f"go depth {DEPTH}")
            info = ""
            while True:
                start = __import__("time").time()
                line = p.stdout.readline().rstrip()
                if line.startswith("info depth"):
                    info = line
                elif line.startswith("bestmove"):
                    break
                if __import__("time").time() - start > 30:
                    break
            m = re.search(r"score (cp|mate) ([+-]?\d+)", info)
            score = None
            if m:
                kind, val = m.group(1), int(m.group(2))
                if kind == "cp":
                    score = max(-2000, min(2000, val))
                else:
                    score = 2000 if val > 0 else (-2000 if val < 0 else 0)
            out.write(json.dumps({"fen": fen, "score": score}) + "\n")
            out.flush()
            n += 1
            if n % 5000 == 0:
                print(f"labeled {n}/{len(todo)}", flush=True)
    p.kill()
    print("done", n)

if __name__ == "__main__":
    label()
