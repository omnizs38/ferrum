#!/usr/bin/env python3
"""Ask an engine for `go depth N` bestmove on a FEN (waits for bestmove)."""
import subprocess, sys

exe = sys.argv[1]
fen = sys.argv[2]
depth = sys.argv[3] if len(sys.argv) > 3 else "6"

p = subprocess.Popen([exe], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
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
send("position fen " + fen)
send(f"go depth {depth}")
infos = []
while True:
    line = p.stdout.readline().rstrip()
    if line.startswith("info depth"):
        infos.append(line)
    elif line.startswith("bestmove"):
        print("\n".join(infos[-2:] + [line]))
        break
p.kill()
