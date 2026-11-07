# Ferrum Gamma & Ferrum Alpha — anatomy, and the (missing) playstyles

Written from direct code inspection and measured probes (not from the
aspirational labels). Everything here is checkable: `src/`, `results/`,
`ELO.md`.

## The two binaries, line by line

| Subsystem | **Ferrum Gamma** (default, `cargo build --release`) | **Ferrum Alpha** (`--no-default-features`) |
|---|---|---|
| Cargo features | `gamma = threads + qsearch + advanced-eval + large-tt` (64 MB TT) | none (8 MB TT) |
| Move generation | `src/board.rs` — bitboards (`src/bitboard.rs`) + occupied-ray sliders via occupancy-indexed tables (`src/sliders.rs`) | identical |
| Legality | pseudo-legal + king-safety filter, `is_in_check` via attack tests | identical |
| Search | negamax + PVS, iterative deepening, aspiration windows (±50, re-centred), null-move pruning (`R = 2 + depth/4`), late-move reduction, qsearch (stand-pat, delta+SEE pruning), check extension capped at 12 | negamax + PVS, iterative deepening, static eval at horizon; **no qsearch, no null-move, no LMR, no aspiration** |
| Evaluation | tapered MG/EG material + PST (maintained incrementally in `Board::mg_scores/eg_scores`), plus mobility, pawn structure (passed/doubled/isolated/connected/blocked), bishop pair, rook open/semi files, king pawn-shield | material + a **single** PST (the shared `PST`, no EG taper, no positional terms) |
| Threads / SMP | Lazy SMP (main + helpers, detached; helps at 200–500 ms, hurts at 1 ms, games split asymmetrically) | single-thread |
| TT | single-entry table, single-lock `probe_both`, age-based replacement | same structure, 8 MB |
| Depth 8 (bench) | ~40439 nodes | much shallower at fixed time |

## The "trap-based" and "defensive" labels — verified against reality

**There is no trap-play or defensive-play mechanism anywhere in the code.**
Both engines are pure maximizers: material/PST/positional value, negamax
alpha-beta, tactics. Nothing in the eval rewards baiting, tension
retention, provocation, or holding compensation; nothing rewards passive
solidity, damage limitation, or drawing at-all-costs play either.

Why the labels *feel* true — evidence, not impression:

*Gamma looks "cunning" because* it is the *deeper, richer* searcher: at
equal time it sees forcing lines Alpha cannot see; it simply wins the
material battles that Alpha doesn't calculate. It does not set traps;
it notices that Alpha walked into one — most wins from the 1 ms ladder
(see `results/`) come from tactical strikes in the middlegame, never
from a prepared bait.
Gamma's eval has king-safety and passed-pawn amplification — the things
that punish tactic-poor opponents' sloppy structure — but zero "trap"
terms (e.g. no bonus for attacking a loose point, no penalty for a
dangling defender, no bait-first PV bias).

*Alpha looks "defensive" because* it is the *shallower, static* one:
no qsearch, so inside the tree where tactics are sharpest it plays
stand-pat; no mobility/king-safety/pawn terms, so it misjudges dynamic
positions; and a material-dominant eval that rarely initiates anything.
It holds material longer simply because it is bad at winning it quickly,
not because of measured defensive evaluation. Measured vs Gamma: e.g.
15–4–1 (+215) at 50 ms; 12–5–3 final build at 50 ms.

Direct probes (see `tools/compare_evals.py`):

- Both engines produce Near-identical evaluations of the trap motifs
  (e.g. pin+fork position: Gamma +570, Alpha +420; Fried-Liver:
  Gamma −24, Alpha −55) — they see the same tactical skeleton, Gamma
  just searches deeper; their moves diverge only where depth settles
  the line.
- Neither engine ever chose a line whose point is "this trap is set on
  purpose"; both play what wins the most nodes.

**Verdict:** Gamma = *sharp/tactical* in effect, not by design; Alpha =
*static/passive* in effect, not by design. The personality is an
artifact of feature depth, not an implemented style.

## What "conforming to the stated styles" would require (not done)

Gamma "trap-based" for real (candidate knobs):
* Eval terms: attack pressure on loose defenders / overextended pieces,
  exposed loose king-squares, compensation factors for genuine attacks.
* Search: prefer quiet PV move that *creates* threats when material is
  equal (forcing-line bias); killer-move selection favouring attacking
  quiet moves.
Alpha "defensive" for real:
* Eval: solidity/damage-limitation weights (safer king position,
  structural damage penalties amplified), draw-oriented bonus when
  behind; risk-averse move selection (suppress sharp forcing lines in
  won positions, prefer simplifying).
* Search: prefer simplifying captures in good positions; avoid sharps
  unless forcing.

Either can be implemented and A/B-measured exactly like any other change;
they currently exist nowhere in this codebase.

**Bottom line, stated honestly:** anyone tuning against these labels as
if they were implemented mechanics is chasing ghosts. The labels name
*emergent perception*, not *engine design*.
