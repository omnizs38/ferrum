#!/usr/bin/env python3
"""Texel/distillation tuner for ferrum's Gamma evaluation tables.

Features (per position): MG/EG PST counts (768+768), material counts (12),
and positional scalar indicators (14). The model reproduces the historical
tapered formula used by this tuner; texel_fit.py nudges the weights toward
minimizing squared error vs Stockfish labels (Huber).

Reads results/texel_corpus.jsonl (labels from tools/label_corpus.py) and
writes results/texel_features.npz for tools/texel_fit.py.
"""
import json, sys, time
import numpy as np
sys.path.insert(0, "tools")
from ferrum_eval import (PASSED_BONUS, pst_index, parse_fen, color_bitboard,
                         passed_pawn_mask, popcount, KNIGHT_MASK, ray_attacks,
                         FILE_A, FILE_H)

PIECE_NAMES = ["P", "N", "B", "R", "Q", "K"]

def positional_features(pieces_all, color):
    """The 14 positional indicators used (both as MG bonus and EG bonus/2)."""
    occ = 0
    for c in 'wb':
        for p in pieces_all[c]:
            occ |= p
    own = color_bitboard(pieces_all[color], color)
    feats = [0.0] * 14
    # mobility: N*4, B*3, R*2, Q*1 (capture weights as scalars ×1000 units)
    n_attacks = 0
    b = pieces_all[color][1]
    while b:
        lsb = b & -b; sq = lsb.bit_length() - 1; b ^= lsb
        n_attacks += popcount(KNIGHT_MASK[sq] & ~own)
    feats[0] = n_attacks          # knight mobility squares
    bish = pieces_all[color][2]
    b_att = 0
    while bish:
        lsb = bish & -bish; sq = lsb.bit_length() - 1; bish ^= lsb
        b_att += popcount(ray_attacks(sq, occ, [4,6,5,7]) & ~own)
    feats[1] = b_att
    rook = pieces_all[color][3]
    r_att = 0
    while rook:
        lsb = rook & -rook; sq = lsb.bit_length() - 1; rook ^= lsb
        r_att += popcount(ray_attacks(sq, occ, [0,1,2,3]) & ~own)
    feats[2] = r_att
    queen = pieces_all[color][4]
    q_att = 0
    while queen:
        lsb = queen & -queen; sq = lsb.bit_length() - 1; queen ^= lsb
        q_att += popcount((ray_attacks(sq, occ, [0,1,2,3]) | ray_attacks(sq, occ, [4,6,5,7])) & ~own)
    feats[3] = q_att
    # pawn structure pieces: doubled-same_file count, isolated count, blocked count, connected count
    pawns = pieces_all[color][0]
    enemy_pawns = pieces_all['w' if color == 'b' else 'b'][0]
    if color == 'w':
        defended = ((pawns & ~FILE_A) << 7) | ((pawns & ~FILE_H) << 9)
        blocked = pawns & (pawns << 8)
    else:
        defended = ((pawns & ~FILE_H) >> 7) | ((pawns & ~FILE_A) >> 9)
        blocked = pawns & (pawns >> 8)
    feats[4] = popcount(pawns & defended)   # connected
    feats[5] = popcount(blocked)            # blocked
    same_file_extra = 0
    file_count = [0] * 8
    file_mask = [0] * 8
    bb = pawns
    while bb:
        lsb = bb & -bb; sq = lsb.bit_length() - 1; bb ^= lsb
        f = sq % 8
        file_count[f] += 1
        file_mask[f] |= lsb
    for f in range(8):
        if file_count[f] > 1:
            same_file_extra += file_count[f] - 1
    feats[6] = same_file_extra
    isolated = 0
    for f in range(8):
        if file_count[f] == 0:
            continue
        if not ((f > 0 and file_mask[f-1]) or (f < 7 and file_mask[f+1])):
            isolated += file_count[f]
    feats[7] = isolated
    passed = 0
    bb = pawns
    passed_bonus_sum = 0
    while bb:
        lsb = bb & -bb; sq = lsb.bit_length() - 1; bb ^= lsb
        if enemy_pawns & passed_pawn_mask(sq, color) == 0:
            rank = sq // 8
            rank_from_own = rank if color == 'w' else 7 - rank
            passed_bonus_sum += PASSED_BONUS[rank_from_own]
            passed += 1
    feats[8] = passed_bonus_sum   # sum of passed bonuses (pre-scaled)
    # rook files: open / semi-open
    pawns_w = pieces_all['w'][0]
    pawns_b = pieces_all['b'][0]
    any_pawn = pawns_w | pawns_b
    own_pawns = pawns_w if color == 'w' else pawns_b
    open_f = semi_f = 0
    rooks = pieces_all[color][3]
    while rooks:
        lsb = rooks & -rooks; sq = lsb.bit_length() - 1; rooks ^= lsb
        f = sq % 8
        mask = FILE_A << f
        if any_pawn & mask == 0: open_f += 1
        elif own_pawns & mask == 0: semi_f += 1
    feats[9] = open_f
    feats[10] = semi_f
    # king safety: full shields count (3 == grand total) — record count of shield pawns and missing count
    king_bb = pieces_all[color][5]
    if king_bb:
        ksq = (king_bb & -king_bb).bit_length() - 1
        kf, kr = ksq % 8, ksq // 8
        d = 1 if color == 'w' else -1
        shield = 0
        for df in (-1, 0, 1):
            f = kf + df; r = kr + d
            if 0 <= f < 8 and 0 <= r < 8:
                if pawns >> (r*8+f) & 1: shield += 1
        feats[11] = shield
        feats[12] = 3 - min(shield, 3)
    # bishop pair
    feats[13] = 1 if popcount(pieces_all[color][2]) >= 2 else 0
    return feats

def build_row(fen):
    pieces_all, side = parse_fen(fen)
    phase = 0
    for c in 'wb':
        for k in range(6):
            n = popcount(pieces_all[c][k])
            phase += n * (0,1,1,2,4,0)[k]
    phase = min(256, (phase * 256) // 24)

    mg_w = np.zeros(384); eg_w = np.zeros(384)
    mg_b = np.zeros(384); eg_b = np.zeros(384)
    mat_w = np.zeros(6); mat_b = np.zeros(6); e_mat_w = np.zeros(6); e_mat_b = np.zeros(6)
    for c, mgv, egv in (('w', mg_w, eg_w), ('b', mg_b, eg_b)):
        for k in range(6):
            bb = pieces_all[c][k]
            while bb:
                lsb = bb & -bb; sq = lsb.bit_length() - 1; bb ^= lsb
                idx = k * 64 + pst_index(sq, c)
                mgv[idx] += 1
                egv[idx] += 1
                if c == 'w': mat_w[k] += 1; e_mat_w[k] += 1
                else: mat_b[k] += 1; e_mat_b[k] += 1
    pos_w = np.array(positional_features(pieces_all, 'w'), dtype=np.float64)
    pos_b = np.array(positional_features(pieces_all, 'b'), dtype=np.float64)
    sign = 1.0 if side == 'w' else -1.0
    row = {
        'phase': phase,
        'mg_diff': mg_w - mg_b,   # features, signed
        'eg_diff': eg_w - eg_b,
        'mat_diff': mat_w - mat_b,      # MG material
        'emat_diff': e_mat_w - e_mat_b, # EG material
        'pos_diff': pos_w - pos_b,
        'sign': sign,
    }
    return row

def load_labels(path):
    labels = {}
    with open(path) as fh:
        for line in fh:
            try:
                d = json.loads(line)
            except Exception:
                continue
            if d.get('fen') is not None and d.get('score') is not None:
                labels[d['fen']] = d['score']
    return labels

def main():
    print("loading labels...", flush=True)
    labels = load_labels("results/texel_corpus.jsonl")
    print("labels:", len(labels), flush=True)
    fens = sorted(labels.keys())
    rows = []
    t0 = time.time()
    for i, fen in enumerate(fens):
        rows.append(build_row(fen))
        if i and i % 10000 == 0:
            print(f"features {i}/{len(fens)} ({time.time()-t0:.1f}s)", flush=True)
    y = np.array([labels[f] for f in fens], dtype=np.float64)
    print("built", len(rows), "rows in", round(time.time()-t0, 1), "s", flush=True)
    np.savez("results/texel_features.npz",
             phase=np.array([r['phase'] for r in rows], dtype=np.float64),
             mg_diff=np.array([r['mg_diff'] for r in rows]),
             eg_diff=np.array([r['eg_diff'] for r in rows]),
             mat_diff=np.array([r['mat_diff'] for r in rows]),
             emat_diff=np.array([r['emat_diff'] for r in rows]),
             pos_diff=np.array([r['pos_diff'] for r in rows]),
             sign=np.array([r['sign'] for r in rows]),
             y=y)
    print("saved results/texel_features.npz", flush=True)

if __name__ == "__main__":
    main()
