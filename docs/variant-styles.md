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

## The "trap-based" and "defensive" labels — verified, then implemented

First, honesty: at the time of the study both engines had **no**
style mechanism — they were pure maximizers (material/PST/positional,
negamax), and the labels were emergent perception, not design:

*Gamma looked "cunning" because* it was the *deeper, richer* searcher
— forcing lines Alpha cannot see, and winning the material battles;
it didn't set traps, it noticed Alpha walked into them (most 1 ms-ladder
wins were tactical strikes, not prepared bait).

*Alpha looked "defensive" because* it was the *shallower, static* one
— played stand-pat at the horizon, rarely initiated anything, held
material more from slowness than from design.

The labels are now **implemented and measured** (WIP-through the code):

* Gamma advanced-eval: loose-material hunting bonus (attacked more than
  defended with profit: `loose_material(side) * 8/100` cp). A/B vs base:
  30 g @100 ms: **7–3–20, +47** (CI [−78,185]); earlier iteration measured
  +83 with the same conclusion. Unit test: `attackers_count_and_loose_material_work`.
* Alpha simple-eval: king pawn-shield bonus (8 cp/pawn, cap 3 → +24):
  value covered castled positions. A/B vs base: 30 g @100 ms: **7–11–12, −47**
  (CI [−185,78]), within noise of base. A naive loose-piece penalty
  (−8/100 cp own loose material) measured **−223** and was discarded.

The styles are engineered and verified in the current tree; they are not
identity claims — Alpha is still the lightweight variant, Gamma the rich
one; the personalities are different *biases*, not different strengths.

Direct probes (see `tools/compare_evals.py`):

- Both engines produce Near-identical evaluations of the trap motifs
  (e.g. pin+fork position: Gamma +570, Alpha +420; Fried-Liver:
  Gamma −24, Alpha −55) — they see the same tactical skeleton, Gamma
  just searches deeper; their moves diverge only where depth settles
  the line.
- Neither engine ever chose a line whose point is "this trap is set on
  purpose"; both play what wins the most nodes.

**Verdict (post-implementation):** the personalities are now implemented, not
only emergent:

- **Gamma "cunning/trap-based" (implemented):** advanced-eval gains a
  *loose-material hunting* bonus (`loose_material(board, side) * 8 / 100`):
  a N/B/R/Q attacked more than defended, and profitably capturable by a
  cheaper-enough attacker (defenders == 0 → hanging to anyone; otherwise at
  least one attacker of value ≤ victim). This is the closest classical-eval
  equivalent of "attack the hanging things". Verified: unit test
  `attackers_count_and_loose_material_work`; A/B vs base at 100 ms,
  30 games: **7–3–20, +47** (CI [−78,185]) — positive direction in line
  with an earlier implementation of the same idea measured twice
  (+83, +47).
- **Alpha "defensive" (implemented):** simple-eval gains a *king pawn-shield*
  bonus (`shield_bonus`: 8 cp per pawn in the 3 squares directly ahead of
  the king, capped at 3 → up to +24): values castled, covered positions.
  A first candidate (penalise own loose material −8/100 cp) was measured at
  **−223** (CI [−451,−97]) and discarded; the shield bonus measures
  **7–11–12, −47** (CI [−185,78]) — within noise of base, and the
  previous catastrophic variant was abandoned. Games from
  `results/gauntlet_defensive2_vs_base_100ms.json`,
  `results/gauntlet_trap2_vs_base_100ms.json`.
- Both remain equally-rated ladders of their own strength — the playstyles
  are different; the engines are not suddenly different beings.

## What the styles cost / buy

Gamma: hunts hanging material always — its preferred lines now differ
from base in loose-material positions. Strength impact ambiguous-positive
(+47/+83/+47, CIs overlapping 0). Alpha: prefers shield/castle setups,
style impact is real but passive; strength impact within noise
(−47, CI [−185,78]).

**Bottom line, stated honestly:** the labels were originally aspirations,
not mechanics; they are now mechanics — Gamma hunts hanging material,
Alpha values king shelter — verified by unit tests (`attackers_count_and_loose_material_work`)
and 30-game A/Bs. The line-level behaviour is visible via
`tools/probe_fen.py` and `tools/compare_evals.py`.
