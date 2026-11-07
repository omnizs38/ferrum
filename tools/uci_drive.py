#!/usr/bin/env python3
"""Drive the engine like a real UCI GUI: wait for bestmove before the next command."""
import subprocess, sys, time

engine = sys.argv[1]
lines = sys.stdin.read().splitlines()
p = subprocess.Popen([engine], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                      stderr=subprocess.DEVNULL, text=True, bufsize=1)
out = []

def read_until(prefix, timeout=30.0):
    deadline = time.time() + timeout
    buf = []
    while time.time() < deadline:
        line = p.stdout.readline()
        if not line:
            break
        line = line.rstrip()
        buf.append(line)
        if line.startswith(prefix):
            return buf
    raise TimeoutError(f"timeout waiting for {prefix!r}; got {buf}")

for raw in lines:
    raw = raw.strip()
    if not raw:
        continue
    p.stdin.write(raw + "\n")
    p.stdin.flush()
    if raw.startswith("go"):
        out += read_until("bestmove", 60.0)
    elif raw == "uci":
        out += read_until("uciok")
    elif raw == "isready":
        out += read_until("readyok")
    elif raw == "ucinewgame":
        pass
    else:
        # debug commands: read a short burst
        time.sleep(0.2)
        out.append(f"[sent {raw!r}]")


p.stdin.write("quit\n")
p.stdin.flush()
try:
    p.wait(timeout=5)
except subprocess.TimeoutExpired:
    p.kill()
print("\n".join(out))
