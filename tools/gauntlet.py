#!/usr/bin/env python3
"""Ferrum gauntlet: UCI self-play harness (stdlib only).

Plays engine-vs-engine games by speaking UCI to two binaries, adjudicating
mates/stalemates (via `bestmove (none)` + info score), draws by repetition
(3-fold), 50-move rule, and insufficient material (parsed from FEN).
Reports scores and relative Elo (logistic) with 95% CI.

Usage (from repo root, Windows paths work too):
  python tools/gauntlet.py --white target/release/ferrum-gamma.exe \
      --black target/release/ferrum-alpha.exe --games 20 --movetime 50

Elo estimates are *relative to the anchor* used in this run: for the
calibration in ELO.md the anchor is Stockfish (`UCI_LimitStrength` +
`UCI_Elo`), driven through a Ferrum oracle for `d`/FEN. The Stockfish
binary itself is not vendored in this repo (see .gitignore).
"""
from __future__ import annotations

import argparse
import json
import math
import queue
import re
import subprocess
import sys
import threading
import time
from dataclasses import dataclass, field


MOVE_RE = re.compile(r"^[a-h][1-8][a-h][1-8][qrbn]?$")


def parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser(description="Ferrum UCI gauntlet")
    p.add_argument("--white", required=True, help="UCI binary for the A side (White in odd games)")
    p.add_argument("--black", required=True, help="UCI binary for the B side")
    p.add_argument("--games", type=int, default=10, help="total games (half each colour)")
    p.add_argument("--movetime", type=int, default=50, help="ms per move (go movetime)")
    p.add_argument("--limit", type=int, default=300, help="max ply before adjudicated draw")
    p.add_argument("--hash-w", type=int, default=None, help="Hash MB for --white (None=engine default)")
    p.add_argument("--hash-b", type=int, default=None, help="Hash MB for --black")
    p.add_argument("--threads-w", type=int, default=None, help="Threads for --white")
    p.add_argument("--threads-b", type=int, default=None, help="Threads for --black")
    p.add_argument("--elo-w", type=int, default=None, help="If set, enable UCI_LimitStrength for --white (Stockfish)")
    p.add_argument("--elo-b", type=int, default=None, help="If set, enable UCI_LimitStrength for --black (Stockfish)")
    p.add_argument("--oracle", default=None, help="Ferrum binary used only as FEN oracle (needed when a player lacks `d`, e.g. Stockfish)")
    p.add_argument("--out", default="results/gauntlet.json", help="JSON results path")
    p.add_argument("--timeout", type=float, default=30.0, help="seconds to wait per bestmove")
    return p.parse_args()


class UCIEngine:
    def __init__(self, path: str, label: str, hash_mb: int | None, threads: int | None, elo: int | None = None):
        self.path = path
        self.label = label
        self.proc = subprocess.Popen(
            [path],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
            bufsize=1,
        )
        self.q: queue.Queue[str] = queue.Queue()
        self._reader = threading.Thread(target=self._pump, daemon=True)
        self._reader.start()
        self.send("uci")
        self.wait_for("uciok")
        self.send("isready")
        self.wait_for("readyok")
        if hash_mb is not None:
            self.send(f"setoption name Hash value {hash_mb}")
        if threads is not None:
            self.send(f"setoption name Threads value {threads}")
        if elo is not None:
            self.send("setoption name UCI_LimitStrength value true")
            self.send(f"setoption name UCI_Elo value {elo}")
        self.send("isready")
        self.wait_for("readyok")

    def _pump(self) -> None:
        assert self.proc.stdout is not None
        for line in self.proc.stdout:
            self.q.put(line.strip())

    def send(self, cmd: str) -> None:
        assert self.proc.stdin is not None
        self.proc.stdin.write(cmd + "\n")
        self.proc.stdin.flush()

    def wait_for(self, prefix: str, timeout: float = 30.0) -> str:
        deadline = time.time() + timeout
        while time.time() < deadline:
            try:
                line = self.q.get(timeout=0.5)
            except queue.Empty:
                continue
            if line.startswith(prefix):
                return line
        raise TimeoutError(f"{self.label}: timeout waiting for {prefix!r}")

    def get_bestmove(self, timeout: float) -> tuple[str | None, list[str]]:
        """Send `go movetime ...` and collect info/bestmove. Returns (move, infos)."""
        infos: list[str] = []
        deadline = time.time() + timeout
        while time.time() < deadline:
            try:
                line = self.q.get(timeout=0.5)
            except queue.Empty:
                continue
            if line.startswith("info "):
                infos.append(line)
            elif line.startswith("bestmove"):
                parts = line.split()
                mv = parts[1] if len(parts) > 1 else None
                return mv, infos
        # Timed out: try to stop and give up.
        try:
            self.send("stop")
        except Exception:
            pass
        raise TimeoutError(f"{self.label}: timeout waiting for bestmove")

    def get_fen(self) -> str:
        """Ask the engine for `d` (FEN). The FEN line has no prefix; we read
        until a line that looks like a FEN (contains '/' and a side field)."""
        self.send("d")
        deadline = time.time() + 10
        while time.time() < deadline:
            line = self.q.get(timeout=1)
            toks = line.split()
            if len(toks) >= 4 and "/" in toks[0] and toks[1] in ("w", "b"):
                return line
        raise TimeoutError(f"{self.label}: timeout waiting for FEN (d)")

    def quit(self) -> None:
        try:
            self.send("quit")
        except Exception:
            pass
        try:
            self.proc.wait(timeout=5)
        except Exception:
            self.proc.kill()


def fen_position_key(fen: str) -> str:
    toks = fen.split()
    return " ".join(toks[:4]) if len(toks) >= 4 else fen


def is_insufficient_material(fen: str) -> bool:
    placement = fen.split()[0]
    # Strip digits; remaining chars are pieces.
    pieces = [c for c in placement if c.isalpha()]
    lowered = [c.lower() for c in pieces]
    if "p" in lowered or "r" in lowered or "q" in lowered:
        return False
    # Count minors.
    minors = [c for c in lowered if c in ("n", "b")]
    if len(minors) <= 1:
        return True
    # K+B vs K+B same-colour bishops -> draw.
    if len(minors) == 2 and minors == ["b", "b"]:
        # Need bishop squares: re-parse placement.
        bishops = []
        for rank_idx, rank_str in enumerate(placement.split("/")):
            file_idx = 0
            for ch in rank_str:
                if ch.isdigit():
                    file_idx += int(ch)
                else:
                    if ch in ("B", "b"):
                        # rank of FEN row 0 is rank 8 (8-rank_idx).
                        bishops.append((file_idx, 8 - rank_idx))
                    file_idx += 1
        if len(bishops) == 2:
            (f1, r1), (f2, r2) = bishops
            return ((f1 + r1) % 2) == ((f2 + r2) % 2)
    return False


@dataclass
class GameResult:
    game: int
    white_label: str
    black_label: str
    result: str  # "1-0" | "0-1" | "1/2-1/2"
    reason: str
    plies: int
    moves: list[str] = field(default_factory=list)


def play_game(white: UCIEngine, black: UCIEngine, moves: list[str], movetime_ms: int, limit: int, timeout: float, oracle: UCIEngine | None = None) -> GameResult:
    """One game starting from `moves` (UCI strings). Returns the result."""
    # Drive both engines with the same position. `oracle` (a Ferrum binary)
    # provides FENs when a player lacks `d` (Stockfish ignores it).
    hist: dict[str, int] = {}
    fen_src = oracle if oracle is not None else white

    # Seed repetition with the starting position.
    for eng in (white, black):
        eng.send("position startpos moves " + " ".join(moves) if moves else "position startpos")
    if oracle is not None:
        oracle.send("position startpos moves " + " ".join(moves) if moves else "position startpos")
    fen0 = fen_src.get_fen()
    hist[fen0.split()[0] + " " + fen0.split()[1]] = 1

    side_is_white = (len(moves) % 2) == 0
    winners = {True: white, False: black}
    stm = winners[side_is_white]

    for ply in range(limit):
        stm.send(f"go movetime {movetime_ms}")
        try:
            mv, infos = stm.get_bestmove(timeout)
        except TimeoutError:
            other = black if side_is_white else white
            return GameResult(len(moves), white.label, black.label, "1-0" if other is white else "0-1",
                              f"{stm.label} timeout (forfeit)", ply, list(moves))
        if mv is None or mv == "(none)" or not MOVE_RE.match(mv):
            # No legal moves? Check last info score for mate vs stalemate.
            last = infos[-1] if infos else ""
            if "score mate" in last:
                reason = f"{stm.label} checkmated"
                return GameResult(len(moves), white.label, black.label, "0-1" if side_is_white else "1-0", reason, ply, list(moves))
            if mv == "(none)":
                # No best move and no mate score -> stalemate/draw adjudication.
                return GameResult(len(moves), white.label, black.label, "1/2-1/2", "stalemate (no legal moves)", ply, list(moves))
            return GameResult(len(moves), white.label, black.label, "1-0" if side_is_white else "0-1",
                              f"{stm.label} illegal move {mv!r} (forfeit)", ply, list(moves))

        moves.append(mv)
        # Push the move to both engines.
        pos = "position startpos moves " + " ".join(moves)
        white.send(pos)
        black.send(pos)
        if oracle is not None:
            oracle.send(pos)

        # Fetch FEN from the oracle (or the mover if no oracle).
        src = fen_src
        try:
            fen = src.get_fen()
        except TimeoutError:
            return GameResult(len(moves), white.label, black.label, "1-0" if side_is_white else "0-1",
                              f"{src.label} no FEN after move (forfeit)", ply + 1, list(moves))
        toks = fen.split()
        halfmove = int(toks[4]) if len(toks) > 4 else 0
        key = fen_position_key(fen)
        hist[key] = hist.get(key, 0) + 1
        if hist[key] >= 3:
            return GameResult(len(moves), white.label, black.label, "1/2-1/2", "threefold repetition", ply + 1, list(moves))
        if halfmove >= 100:
            return GameResult(len(moves), white.label, black.label, "1/2-1/2", "fifty-move rule", ply + 1, list(moves))
        if is_insufficient_material(fen):
            return GameResult(len(moves), white.label, black.label, "1/2-1/2", "insufficient material", ply + 1, list(moves))

        side_is_white = not side_is_white
        stm = winners[side_is_white]

    return GameResult(len(moves), white.label, black.label, "1/2-1/2", "move limit adjudicated draw", limit, list(moves))


def elo_from_score(p: float) -> float:
    p = min(max(p, 1e-6), 1 - 1e-6)
    return -400.0 * math.log10(1.0 / p - 1.0)


def main() -> int:
    args = parse_args()
    if args.games <= 0:
        print("games must be > 0", file=sys.stderr)
        return 2
    if args.games % 2 == 1:
        print("games should be even for colour balance; proceeding anyway", file=sys.stderr)

    print(f"Gauntlet: A={args.white} vs B={args.black} games={args.games} movetime={args.movetime}ms")
    ea = UCIEngine(args.white, "A", args.hash_w, args.threads_w, args.elo_w)
    eb = UCIEngine(args.black, "B", args.hash_b, args.threads_b, args.elo_b)
    oracle = UCIEngine(args.oracle, "Oracle", None, None, None) if args.oracle else None
    try:
        # Reuse the same two processes for all games (ucinewgame per game).
        results: list[GameResult] = []
        for g in range(args.games):
            ea.send("ucinewgame")
            eb.send("ucinewgame")
            if oracle is not None:
                oracle.send("ucinewgame")
            # Alternate colours: even games A=White, odd games B=White.
            if g % 2 == 0:
                res = play_game(ea, eb, [], args.movetime, args.limit, args.timeout, oracle)
                # Normalise to A-score.
                results.append(res)
            else:
                res = play_game(eb, ea, [], args.movetime, args.limit, args.timeout, oracle)
                # Swap colours back for A-centric scoring.
                res.white_label, res.black_label = res.black_label, res.white_label
                if res.result == "1-0":
                    res.result = "0-1"
                elif res.result == "0-1":
                    res.result = "1-0"
                results.append(res)
            print(f"game {g+1}/{args.games}: {res.result} ({res.reason}) plies={res.plies} moves={res.moves[:10]}{'...' if len(res.moves) > 10 else ''}", flush=True)

        # Score from the --white binary's perspective (A).
        a_wins = sum(1 for r in results if r.result == "1-0")
        b_wins = sum(1 for r in results if r.result == "0-1")
        draws = sum(1 for r in results if r.result == "1/2-1/2")
        total = len(results)
        score_a = a_wins + 0.5 * draws
        p = score_a / total if total else 0.5
        elo = elo_from_score(p) if 0 < p < 1 else float("-inf") if p == 0 else float("inf")
        # 95% Wilson-ish interval via normal approx on logit is unstable at
        # extremes; use simple SE on p then map through the Elo curve.
        se = math.sqrt(p * (1 - p) / total) if total else 0
        lo_p = max(1e-6, p - 1.96 * se)
        hi_p = min(1 - 1e-6, p + 1.96 * se)
        elo_lo = elo_from_score(lo_p) if lo_p < 1 else 0
        elo_hi = elo_from_score(hi_p) if hi_p > 0 else 0
        print(f"\nSummary A wins={a_wins} B wins={b_wins} draws={draws} total={total}")
        print(f"A score fraction p={p:.3f}  Elo(A-B)={elo:.0f} (approx 95% CI [{elo_lo:.0f}, {elo_hi:.0f}])")
        print("NOTE: relative gap only. Absolute Elo needs an external rated anchor.")
        out = {
            "args": vars(args),
            "results": [vars(r) for r in results],
            "summary": {"a_wins": a_wins, "b_wins": b_wins, "draws": draws, "total": total, "p_A": p, "elo_A_minus_B": elo, "elo_lo": elo_lo, "elo_hi": elo_hi},
        }
        import pathlib
        pathlib.Path(args.out).parent.mkdir(parents=True, exist_ok=True)
        pathlib.Path(args.out).write_text(json.dumps(out, indent=2))
        print(f"wrote {args.out}")
    finally:
        ea.quit()
        eb.quit()
        if oracle is not None:
            oracle.quit()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
