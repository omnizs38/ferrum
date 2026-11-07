#!/usr/bin/env python3
"""Verify gauntlet mate claims using Ferrum itself: perft 1 == 0 and search score is mate.

Usage: python tools/verify_mates.py [results JSON path] [ferrum exe]
Defaults: results/gauntlet_gamma_alpha.json and target/release/ferrum-gamma.exe.
"""
import json, queue, subprocess, threading, time, sys

def run_one(exe, moves):
    p = subprocess.Popen([exe], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, bufsize=1)
    q = queue.Queue()
    def pump():
        for line in p.stdout:
            q.put(line.strip())
    threading.Thread(target=pump, daemon=True).start()
    def send(s):
        p.stdin.write(s+"\n"); p.stdin.flush()
    def wait(prefix, timeout=10):
        deadline=time.time()+timeout
        while time.time()<deadline:
            try: line=q.get(timeout=0.5)
            except queue.Empty: continue
            if line.startswith(prefix): return line
        return None
    send("uci"); wait("uciok"); send("isready"); wait("readyok")
    send("position startpos moves "+" ".join(moves))
    send("d")
    fen=None
    deadline=time.time()+10
    while time.time()<deadline:
        try: line=q.get(timeout=1)
        except queue.Empty: continue
        t=line.split()
        if len(t)>=4 and "/" in t[0] and t[1] in ("w","b"):
            fen=line; break
    send("perft 1")
    perft=None
    deadline=time.time()+10
    while time.time()<deadline:
        try: line=q.get(timeout=1)
        except queue.Empty: continue
        if line.startswith("perft 1"): perft=line; break
    send("go depth 2")
    infos=[]
    deadline=time.time()+15
    best=None
    while time.time()<deadline:
        try: line=q.get(timeout=1)
        except queue.Empty: continue
        if line.startswith("info "): infos.append(line)
        if line.startswith("bestmove"): best=line; break
    send("quit")
    try: p.wait(timeout=5)
    except Exception: p.kill()
    return fen, perft, infos[-1] if infos else "", best

data_path = sys.argv[1] if len(sys.argv) > 1 else "results/gauntlet_gamma_alpha.json"
exe = sys.argv[2] if len(sys.argv) > 2 else r"target\release\ferrum-gamma.exe"
data=json.load(open(data_path))
for i,r in enumerate(data["results"]):
    fen, perft, info, best = run_one(exe, r["moves"])
    ok = (perft and perft.endswith(" 0")) and best and "best" in best and ("(none)" in best) and ("mate" in info)
    print(f"game{i} perft1={perft} info={info!r} best={best!r} FEN={fen!r} => {'MATE-OK' if ok else 'CHECK-FAIL'}")
    if not ok:
        print("  moves tail:", r["moves"][-5:])
