#!/usr/bin/env python3
"""Ferrum training driver.

Runs the in-process paired-game trainer (`examples/train.rs`) repeatedly to
train the two variants towards their playstyle objectives:

  * Ferrum Gamma  -> trapping: hunt loose material, keep enemy pieces short
                     of squares, press the enemy king.
  * Ferrum Alpha  -> defensive: shelter the king, avoid leaving own pieces
                     hanging, keep the structure compact.

Two things are optimised, in order:

  1. **Strength** (SPSA): a simultaneous-perturbation stochastic
     approximation over every `eval::EvalParams` weight, using paired games
     as the objective. A candidate is only kept if it beats the incumbent
     over paired openings.
  2. **Style** (targeted search): with strength frozen, the style weights are
     moved towards the variant's objective and kept only if the style metric
     improves without a strength regression (`score >= --style-floor`).

All results are written as JSON under `training/` (git-ignored) and the
chosen specs are printed so they can be baked into `EvalParams::DEFAULT` or
passed to the engine via `setoption name EvalParams value ...`.

Usage (from the repo root):
  python tools/train.py --variant gamma --mode spsa --iters 24 --games 200
  python tools/train.py --variant gamma --mode style
  python tools/train.py --variant gamma --mode validate --games 600

The script builds `examples/train` for the requested variant itself
(`--no-default-features` for Alpha), so the binary is always the right one.
"""
from __future__ import annotations

import argparse
import json
import math
import pathlib
import random
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
EXE = ROOT / "target" / "release" / "examples" / "train.exe"
TRAIN_DIR = ROOT / "training"

# name: (default, min, max, perturbation, rate)
GAMMA_PARAMS: dict[str, tuple[int, int, int, int, float]] = {
    "mob_knight": (4, 0, 10, 1, 0.8),
    "mob_bishop": (3, 0, 10, 1, 0.8),
    "mob_rook": (2, 0, 6, 1, 0.6),
    "mob_queen": (1, 0, 4, 1, 0.4),
    "bishop_pair": (40, 0, 60, 4, 2.0),
    "passed_scale": (100, 50, 200, 10, 5.0),
    "doubled": (12, 0, 30, 2, 1.5),
    "isolated": (10, 0, 30, 2, 1.5),
    "connected": (12, 0, 30, 2, 1.5),
    "blocked": (8, 0, 20, 2, 1.0),
    "rook_open": (15, 0, 40, 3, 2.0),
    "rook_semi": (8, 0, 30, 2, 1.5),
    "shield_pawn": (15, 0, 30, 2, 1.5),
    "shield_cap": (45, 0, 80, 5, 3.0),
    "shield_missing": (10, 0, 30, 2, 1.5),
    "pressure_scale": (16, 0, 30, 2, 1.5),
    "trap_bonus": (8, 0, 20, 2, 1.0),
    "king_pressure": (3, 0, 10, 1, 0.6),
    "tempo": (15, 0, 30, 3, 2.0),
}

ALPHA_PARAMS: dict[str, tuple[int, int, int, int, float]] = {
    "alpha_shield": (8, 0, 30, 2, 1.5),
    "alpha_castle": (20, 0, 40, 3, 2.0),
    "alpha_king_danger": (0, 0, 15, 1, 0.8),
    "alpha_own_loose": (24, 0, 40, 3, 2.0),
    "alpha_pawn": (100, 80, 120, 3, 2.0),
    "alpha_knight": (320, 280, 360, 6, 4.0),
    "alpha_bishop": (330, 290, 370, 6, 4.0),
    "alpha_rook": (500, 450, 550, 8, 5.0),
    "alpha_queen": (900, 850, 1000, 12, 8.0),
    "tempo": (15, 0, 30, 3, 2.0),
}


def params_for(variant: str) -> dict[str, tuple[int, int, int, int, float]]:
    return GAMMA_PARAMS if variant == "gamma" else ALPHA_PARAMS


def build(variant: str) -> None:
    cmd = ["cargo", "build", "--release", "--example", "train"]
    if variant == "alpha":
        cmd.insert(2, "--no-default-features")
    print("build:", " ".join(cmd), flush=True)
    subprocess.run(cmd, cwd=ROOT, check=True)


def spec_of(theta: dict[str, float], keys: list[str] | None = None) -> str:
    keys = keys or list(theta)
    return ",".join(f"{k}={int(round(theta[k]))}" for k in keys)


def spec_dict(spec: str) -> dict[str, int]:
    if not spec or spec == "default":
        return {}
    out = {}
    for part in spec.split(","):
        k, v = part.split("=")
        out[k.strip()] = int(v)
    return out


def run_match(
    a: str,
    b: str,
    games: int,
    nodes: int,
    seed: int,
    parallel: int,
    tag: str,
    out: pathlib.Path,
) -> dict:
    TRAIN_DIR.mkdir(exist_ok=True)
    cmd = [
        str(EXE),
        "--a", a,
        "--b", b,
        "--games", str(games),
        "--nodes", str(nodes),
        "--seed", str(seed),
        "--parallel", str(parallel),
        "--json", str(out),
        "--tag", tag,
    ]
    res = subprocess.run(cmd, cwd=ROOT, text=True, capture_output=True)
    if res.returncode != 0:
        sys.stderr.write(res.stdout)
        sys.stderr.write(res.stderr)
        raise SystemExit(f"trainer failed ({res.returncode})")
    return json.loads(out.read_text())


def spsa(args: argparse.Namespace) -> dict:
    build(args.variant)
    table = params_for(args.variant)
    rng = random.Random(args.seed)
    theta = {k: float(v[0]) for k, v in table.items()}
    keys = list(table)
    history = []
    for it in range(args.iters):
        decay = (it + 1.0) ** -0.35
        # Perturb every coordinate; c decays so late iterations are fine-tuning.
        delta = {k: rng.choice((-1.0, 1.0)) for k in keys}
        plus, minus = {}, {}
        for k in keys:
            default, lo, hi, c, _rate = table[k]
            step = max(1, int(round(c * decay)))
            plus[k] = min(hi, max(lo, theta[k] + step * delta[k]))
            minus[k] = min(hi, max(lo, theta[k] - step * delta[k]))
        seed = args.seed + 1000 * (it + 1)
        out = TRAIN_DIR / f"tmp_{args.variant}_spsa_{it:02d}.json"
        res = run_match(
            spec_of(plus), spec_of(minus), args.games, args.nodes, seed,
            args.parallel, f"spsa-{args.variant}-{it}", out,
        )
        p = res["p_a"]
        elo = res["elo"]
        old = {k: theta[k] for k in keys}
        for k in keys:
            _default, lo, hi, _c, rate = table[k]
            theta[k] = min(hi, max(lo, theta[k] + 2.5 * rate * decay * (p - 0.5) * delta[k]))
        history.append({
            "iter": it,
            "p_plus": p,
            "elo_plus_minus": elo,
            "plus": spec_of(plus),
            "minus": spec_of(minus),
            "theta_after": spec_of(theta),
        })
        print(
            f"[spsa {args.variant} {it + 1}/{args.iters}] p(plus)={p:.3f} "
            f"elo={elo:+.0f} theta={spec_of(theta)}",
            flush=True,
        )
        out.unlink(missing_ok=True)
    result = {
        "variant": args.variant,
        "mode": "spsa",
        "games": args.games,
        "nodes": args.nodes,
        "seed": args.seed,
        "iters": args.iters,
        "theta": {k: int(round(v)) for k, v in theta.items()},
        "spec": spec_of(theta),
        "history": history,
    }
    path = TRAIN_DIR / f"{args.variant}_spsa.json"
    path.write_text(json.dumps(result, indent=2))
    print(f"wrote {path}")
    return result


STYLE_SPACE = {
    "gamma": {
        "pressure_scale": [16, 24, 32],
        "trap_bonus": [6, 10, 14],
        "king_pressure": [2, 4, 6],
    },
    "alpha": {
        "alpha_shield": [12, 20],
        "alpha_castle": [15, 30],
        "alpha_king_danger": [3, 6],
        "alpha_own_loose": [16, 24, 32],
    },
}


def style_index(variant: str, res: dict) -> float:
    """Higher is more style-conformant.

    Gamma (trapping): how much more loose material we create for the enemy
    than we concede, plus trapped pieces and king-ring pressure.
    Alpha (defensive): the inverse — we want low own loose material, few own
    trapped pieces, and few enemy attacks on our king.
    """
    a = res["style_a"]
    if variant == "gamma":
        return (
            a["enemy_loose"]
            - a["own_loose"]
            + 10.0 * a["enemy_trapped"]
            + 5.0 * a["king_ring_for"]
        )
    return -(a["own_loose"] + 20.0 * a["own_trapped"] + 8.0 * a["king_ring_against"])


def style(args: argparse.Namespace) -> dict:
    build(args.variant)
    base_path = TRAIN_DIR / f"{args.variant}_spsa.json"
    if base_path.exists() and not args.from_scratch:
        base = json.loads(base_path.read_text())["spec"]
        print(f"style: starting from {base_path.name}")
    else:
        base = "default"
        print("style: starting from baseline defaults")

    space = STYLE_SPACE[args.variant]
    keys = list(space)
    base_theta = spec_dict(base)
    # Defaults for keys not present in the base spec.
    table = params_for(args.variant)
    for k in keys:
        base_theta.setdefault(k, table[k][0])

    out = TRAIN_DIR / f"tmp_{args.variant}_style_base.json"
    ref = run_match(base, base, args.games, args.nodes, args.seed + 7, args.parallel,
                    f"style-ref-{args.variant}", out)
    ref_index = style_index(args.variant, ref)
    print(f"style reference: index={ref_index:.2f} score(self)=0.500")
    trials = []
    for combo in _product(space):
        theta = dict(base_theta)
        theta.update(combo)
        spec = ",".join(f"{k}={int(theta[k])}" for k in base_theta)
        res = run_match(spec, base, args.games, args.nodes, args.seed + 11,
                        args.parallel, "style-trial", out)
        idx = style_index(args.variant, res)
        trials.append({"spec": spec, "score_vs_base": res["p_a"], "elo": res["elo"],
                       "style_index": idx, "index_delta": idx - ref_index})
        print(
            f"  {combo} score={res['p_a']:.3f} elo={res['elo']:+.0f} "
            f"style={idx:+.1f} (delta {idx - ref_index:+.1f})",
            flush=True,
        )
    out.unlink(missing_ok=True)

    eligible = [t for t in trials if t["score_vs_base"] >= args.style_floor]
    eligible.sort(key=lambda t: t["index_delta"], reverse=True)
    if eligible and eligible[0]["index_delta"] > 0:
        chosen = eligible[0]
        print(f"style: chose {chosen['spec']} (delta {chosen['index_delta']:+.1f})")
    else:
        chosen = {"spec": base, "score_vs_base": 0.5, "elo": 0.0,
                  "style_index": ref_index, "index_delta": 0.0}
        print("style: no candidate improved the style index without losing strength; keeping base")

    result = {
        "variant": args.variant,
        "mode": "style",
        "games": args.games,
        "nodes": args.nodes,
        "base_spec": base,
        "base_index": ref_index,
        "chosen": chosen,
        "trials": trials,
    }
    path = TRAIN_DIR / f"{args.variant}_style.json"
    path.write_text(json.dumps(result, indent=2))
    print(f"wrote {path}")
    # Also write the usable final spec.
    (TRAIN_DIR / f"{args.variant}_final.spec").write_text(chosen["spec"] + "\n")
    print(f"final spec: {chosen['spec']}")
    return result


def _product(space: dict[str, list[int]]):
    keys = list(space)
    def rec(i, acc):
        if i == len(keys):
            yield dict(acc)
            return
        for v in space[keys[i]]:
            acc[keys[i]] = v
            yield from rec(i + 1, acc)
    yield from rec(0, {})


def validate(args: argparse.Namespace) -> dict:
    build(args.variant)
    final_path = TRAIN_DIR / f"{args.variant}_final.spec"
    trained = args.spec
    if not trained and final_path.exists():
        trained = final_path.read_text().strip()
    if not trained:
        raise SystemExit("no trained spec: run --mode style first or pass --spec")
    runs = []
    for nodes in args.validate_nodes:
        out = TRAIN_DIR / f"tmp_{args.variant}_validate_{nodes}.json"
        res = run_match(trained, "default", args.games, nodes, args.seed + nodes,
                        args.parallel, f"validate-{args.variant}-{nodes}", out)
        out.unlink(missing_ok=True)
        runs.append({
            "nodes": nodes,
            "p": res["p_a"],
            "elo": res["elo"],
            "elo_lo": res["elo_lo"],
            "elo_hi": res["elo_hi"],
            "style_a": res["style_a"],
            "style_b": res["style_b"],
            "style_index_a": style_index(args.variant, res),
        })
        print(
            f"[validate {args.variant} nodes={nodes}] trained vs default: "
            f"score={res['p_a']:.3f} elo={res['elo']:+.0f} "
            f"CI [{res['elo_lo']:.0f}, {res['elo_hi']:.0f}] "
            f"style={runs[-1]['style_index_a']:+.1f}",
            flush=True,
        )
    result = {"variant": args.variant, "mode": "validate", "spec": trained,
              "games": args.games, "runs": runs}
    path = TRAIN_DIR / f"{args.variant}_validation.json"
    path.write_text(json.dumps(result, indent=2))
    print(f"wrote {path}")
    return result


def main() -> int:
    p = argparse.ArgumentParser(description=__doc__,
                                formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--variant", choices=("gamma", "alpha"), required=True)
    p.add_argument("--mode", choices=("spsa", "style", "validate"), default="spsa")
    p.add_argument("--iters", type=int, default=24)
    p.add_argument("--games", type=int, default=200)
    p.add_argument("--nodes", type=int, default=20000)
    p.add_argument("--seed", type=int, default=1)
    p.add_argument("--parallel", type=int, default=8)
    p.add_argument("--style-floor", type=float, default=0.47,
                   help="minimum score vs base for a style candidate")
    p.add_argument("--from-scratch", action="store_true",
                   help="style mode starts from defaults, not the SPSA result")
    p.add_argument("--spec", default=None, help="validate this spec instead of the saved one")
    p.add_argument("--validate-nodes", type=int, nargs="+", default=[20000, 40000, 80000])
    args = p.parse_args()
    if args.mode == "spsa":
        spsa(args)
    elif args.mode == "style":
        style(args)
    else:
        validate(args)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
