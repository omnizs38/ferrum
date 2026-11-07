#!/usr/bin/env python3
"""Tabulate every gauntlet JSON in results/ for the session report."""
import json, glob, os

rows = []
for f in sorted(glob.glob("results/*.json")):
    if "texel" in f:
        continue
    try:
        d = json.load(open(f))
    except Exception:
        continue
    s = d.get("summary", {})
    if not s:
        continue
    a = d.get("args", {})
    name = os.path.basename(f).replace(".json", "")
    est = s.get("elo_A_minus_B", 0)
    try:
        est_i = int(est)
    except (OverflowError, TypeError):
        est_i = -99999  # inf (0 wins)
    rows.append((
        name,
        a.get("games", "?"),
        a.get("movetime", "?"),
        a.get("elo_b", "-"),
        f"{s.get('a_wins')}-{s.get('b_wins')}-{s.get('draws')}",
        est_i,
    ))
rows.sort(key=lambda r: r[0])
for r in rows:
    print("%-52s g=%-3s mt=%-4sms anchor=%-5s score=%-8s est=%+d" % r)
