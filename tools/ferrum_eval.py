#!/usr/bin/env python3
"""Historical NumPy replica of ferrum's Gamma evaluation (src/eval.rs).

Used by the texel tuner. It predates the current trainable style terms and
is not kept in lock-step with the engine; treat it as approximate."""

MG_VALUE = [82, 337, 365, 477, 1025, 0]
EG_VALUE = [94, 281, 297, 512, 936, 0]
PST_MG = [
    [0,0,0,0,0,0,0,0, 50,50,50,50,50,50,50,50, 10,10,20,30,30,20,10,10, 5,5,10,25,25,10,5,5, 0,0,0,20,20,0,0,0, 5,-5,-10,0,0,-10,-5,5, 5,10,10,-20,-20,10,10,5, 0,0,0,0,0,0,0,0],
    [-50,-40,-30,-30,-30,-30,-40,-50, -40,-20,0,0,0,0,-20,-40, -30,0,10,15,15,10,0,-30, -30,5,15,20,20,15,5,-30, -30,0,15,20,20,15,0,-30, -30,5,10,15,15,10,5,-30, -40,-20,0,5,5,0,-20,-40, -50,-40,-30,-30,-30,-30,-40,-50],
    [-20,-10,-10,-10,-10,-10,-10,-20, -10,0,0,0,0,0,0,-10, -10,0,5,10,10,5,0,-10, -10,5,5,10,10,5,5,-10, -10,0,10,10,10,10,0,-10, -10,10,10,10,10,10,10,-10, -10,5,0,0,0,0,5,-10, -20,-10,-10,-10,-10,-10,-10,-20],
    [0,0,0,0,0,0,0,0, 5,10,10,10,10,10,10,5, -5,0,0,0,0,0,0,-5, -5,0,0,0,0,0,0,-5, -5,0,0,0,0,0,0,-5, -5,0,0,0,0,0,0,-5, -5,0,0,0,0,0,0,-5, 0,0,0,5,5,0,0,0],
    [-20,-10,-10,-5,-5,-10,-10,-20, -10,0,0,0,0,0,0,-10, -10,0,5,5,5,5,0,-10, -5,0,5,5,5,5,0,-5, 0,0,5,5,5,5,0,-5, -10,5,5,5,5,5,0,-10, -10,0,5,0,0,0,0,-10, -20,-10,-10,-5,-5,-10,-10,-20],
    [-30,-40,-40,-50,-50,-40,-40,-30, -30,-40,-40,-50,-50,-40,-40,-30, -30,-40,-40,-50,-50,-40,-40,-30, -30,-40,-40,-50,-50,-40,-40,-30, -20,-30,-30,-40,-40,-30,-30,-20, -10,-20,-20,-20,-20,-20,-20,-10, 20,20,0,0,0,0,20,20, 20,30,10,0,0,10,30,20],
]
PST_EG = [
    [0,0,0,0,0,0,0,0, 50,50,50,50,50,50,50,50, 30,30,30,35,35,30,30,30, 20,20,20,25,25,20,20,20, 10,10,10,15,15,10,10,10, 5,5,5,10,10,5,5,5, 0,0,0,0,0,0,0,0, 0,0,0,0,0,0,0,0],
    [-30,-20,-10,-10,-10,-10,-20,-30, -20,-10,0,0,0,0,-10,-20, -10,0,10,15,15,10,0,-10, -10,5,15,20,20,15,5,-10, -10,0,15,20,20,15,0,-10, -10,5,10,15,15,10,5,-10, -20,-10,0,5,5,0,-10,-20, -30,-20,-10,-10,-10,-10,-20,-30],
    [-10,-10,-10,-10,-10,-10,-10,-10, -10,0,0,0,0,0,0,-10, -10,0,5,10,10,5,0,-10, -10,5,5,10,10,5,5,-10, -10,0,10,10,10,10,0,-10, -10,10,10,10,10,10,10,-10, -10,5,0,0,0,0,5,-10, -10,0,0,0,0,0,0,-10],
    [0,0,0,0,0,0,0,0, 5,10,10,10,10,10,10,5, -5,0,0,0,0,0,0,-5, -5,0,0,0,0,0,0,-5, -5,0,0,0,0,0,0,-5, -5,0,0,0,0,0,0,-5, -5,0,0,0,0,0,0,-5, 0,0,0,0,0,0,0,0],
    [-10,-5,-5,-5,-5,-5,-5,-10, -5,0,0,0,0,0,0,-5, -5,0,5,5,5,5,0,-5, 0,0,5,5,5,5,0,-5, 0,0,5,5,5,5,0,-5, -5,0,5,5,5,5,0,-5, -5,0,5,0,0,0,0,-5, -10,-5,-5,-5,-5,-5,-5,-10],
    [-50,-40,-30,-20,-20,-30,-40,-50, -30,-20,-10,0,0,-10,-20,-30, -30,-10,20,30,30,20,-10,-30, -30,-10,30,40,40,30,-10,-30, -30,-10,30,40,40,30,-10,-30, -30,-10,20,30,30,20,-10,-30, -30,-30,0,0,0,0,-30,-30, -50,-30,-30,-30,-30,-30,-30,-50],
]
PASSED_BONUS = [0, 0, 5, 10, 20, 35, 60, 0]
FILE_A = 0x0101010101010101
FILE_H = 0x8080808080808080

def pst_index(sq, color):
    r, f = divmod(sq, 8)
    return (7 - r) * 8 + f if color == 'w' else r * 8 + f

KNIGHT_MASK = [0] * 64
STEP = [[255] * 8 for _ in range(64)]
DIRS = [(0,1),(0,-1),(1,0),(-1,0),(1,1),(-1,1),(1,-1),(-1,-1)]
for sq in range(64):
    f, r = sq % 8, sq // 8
    for df, dr in [(1,2),(2,1),(2,-1),(1,-2),(-1,-2),(-2,-1),(-2,1),(-1,2)]:
        nf, nr = f+df, r+dr
        if 0 <= nf < 8 and 0 <= nr < 8: KNIGHT_MASK[sq] |= 1 << (nr*8+nf)
    for d, (df, dr) in enumerate(DIRS):
        nf, nr = f+df, r+dr
        if 0 <= nf < 8 and 0 <= nr < 8: STEP[sq][d] = nr*8+nf

def popcount(bb): return bin(bb).count('1')

def ray_attacks(sq, occ, dirs):
    out = 0
    for d in dirs:
        cur = sq
        while True:
            cur = STEP[cur][d]
            if cur == 255: break
            out |= 1 << cur
            if occ >> cur & 1: break
    return out

def color_bitboard(pieces, color):
    bb = 0
    for p in pieces:
        bb |= p
    return bb

def passed_pawn_mask(sq, color):
    f, r = sq % 8, sq // 8
    mask = 0
    for df in (-1, 0, 1):
        nf = f + df
        if not (0 <= nf < 8): continue
        if color == 'w':
            for nr in range(r + 1, 8): mask |= 1 << (nr*8 + nf)
        else:
            for nr in range(0, r): mask |= 1 << (nr*8 + nf)
    return mask

def parse_fen(fen):
    parts = fen.split()
    placement, side = parts[0], parts[1]
    pieces = {'w': [0]*6, 'b': [0]*6}
    ranks = placement.split('/')
    for ri, row in enumerate(ranks):
        sq = (7 - ri) * 8
        for ch in row:
            if ch.isdigit():
                sq += int(ch)
            else:
                color = 'w' if ch.isupper() else 'b'
                kind = {'p':0,'n':1,'b':2,'r':3,'q':4,'k':5}[ch.lower()]
                pieces[color][kind] |= 1 << sq
                sq += 1
    return pieces, side

def pawn_structure(pawns_bb, enemy_pawns_bb, color):
    pawns = pawns_bb
    enemy = enemy_pawns_bb
    penalty = 0
    file_count = [0]*8
    file_mask = [0]*8
    bb = pawns
    while bb:
        lsb = bb & -bb
        sq = lsb.bit_length() - 1
        bb ^= lsb
        f = sq % 8
        file_count[f] += 1
        file_mask[f] |= lsb
    if color == 'w':
        defended = ((pawns & ~FILE_A) << 7) | ((pawns & ~FILE_H) << 9)
        blocked = pawns & (pawns << 8)
    else:
        defended = ((pawns & ~FILE_H) >> 7) | ((pawns & ~FILE_A) >> 9)
        blocked = pawns & (pawns >> 8)
    penalty -= popcount(pawns & defended) * 12
    penalty += popcount(blocked) * 8
    bb = pawns
    while bb:
        lsb = bb & -bb
        sq = lsb.bit_length() - 1
        bb ^= lsb
        f = sq % 8
        rank = sq // 8
        same_file = max(0, file_count[f] - 1)
        penalty += 12 * same_file
        has_nb = (f > 0 and file_mask[f-1]) or (f < 7 and file_mask[f+1])
        if not has_nb: penalty += 10
        rank_from_own = rank if color == 'w' else 7 - rank
        if enemy & passed_pawn_mask(sq, color) == 0:
            penalty -= PASSED_BONUS[rank_from_own]
    return -penalty

def rook_files(pieces_all, color):
    pawns_w = pieces_all['w'][0]
    pawns_b = pieces_all['b'][0]
    any_pawn = pawns_w | pawns_b
    bonus = 0
    own_pawns = pawns_w if color == 'w' else pawns_b
    rooks = pieces_all[color][3]
    while rooks:
        lsb = rooks & -rooks
        sq = lsb.bit_length() - 1
        rooks ^= lsb
        f = sq % 8
        mask = FILE_A << f
        if any_pawn & mask == 0: bonus += 15
        elif own_pawns & mask == 0: bonus += 8
    return bonus

def king_safety(pieces_all, color):
    king_bb = pieces_all[color][5]
    if king_bb == 0: return 0
    ksq = (king_bb & -king_bb).bit_length() - 1
    kfile = ksq % 8
    krank = ksq // 8
    pawns = pieces_all[color][0]
    d = 1 if color == 'w' else -1
    shield = 0
    for df in (-1, 0, 1):
        f = kfile + df
        r = krank + d
        if 0 <= f < 8 and 0 <= r < 8:
            if pawns >> (r*8+f) & 1:
                shield += 1
    return min(shield * 15, 45) - (3 - min(shield, 3)) * 10

def mobility(pieces_all, color):
    occ = 0
    for c in 'wb':
        for p in pieces_all[c]:
            occ |= p
    own = color_bitboard(pieces_all[color], color)
    mob = 0
    bb = pieces_all[color][1]  # knights *4
    while bb:
        lsb = bb & -bb; sq = lsb.bit_length() - 1; bb ^= lsb
        mob += popcount(KNIGHT_MASK[sq] & ~own) * 4
    bb = pieces_all[color][2]  # bishops *3
    while bb:
        lsb = bb & -bb; sq = lsb.bit_length() - 1; bb ^= lsb
        mob += popcount(ray_attacks(sq, occ, [4,6,5,7]) & ~own) * 3
    bb = pieces_all[color][3]  # rooks *2
    while bb:
        lsb = bb & -bb; sq = lsb.bit_length() - 1; bb ^= lsb
        mob += popcount(ray_attacks(sq, occ, [0,1,2,3]) & ~own) * 2
    bb = pieces_all[color][4]  # queens *1
    while bb:
        lsb = bb & -bb; sq = lsb.bit_length() - 1; bb ^= lsb
        mob += popcount((ray_attacks(sq, occ, [0,1,2,3]) | ray_attacks(sq, occ, [4,6,5,7])) & ~own)
    return mob

def taper_material(pieces_all, color):
    mg = eg = units = 0
    for k in range(6):
        bb = pieces_all[color][k]
        while bb:
            lsb = bb & -bb; sq = lsb.bit_length() - 1; bb ^= lsb
            mg += MG_VALUE[k] + PST_MG[k][pst_index(sq, color)]
            eg += EG_VALUE[k] + PST_EG[k][pst_index(sq, color)]
            units += {1:1, 2:1, 3:2, 4:4}.get(k, 0)
    phase = min(256, (units * 256) // 24)
    return mg, eg, phase

def tdiv(a, b):
    # Rust integer division truncates toward zero; do it exactly in Python.
    assert b > 0
    q = abs(a) // b
    return q if a >= 0 else -q

def taper(mg, eg, phase):
    return tdiv(mg * phase + eg * (256 - phase), 256)

def evaluate_color(pieces_all, color):
    mg, eg, phase = taper_material(pieces_all, color)
    bonus = 0
    bonus += mobility(pieces_all, color)
    bonus += pawn_structure(pieces_all[color][0], pieces_all['w' if color == 'b' else 'b'][0], color)
    bonus += rook_files(pieces_all, color)
    bonus += king_safety(pieces_all, color)
    if popcount(pieces_all[color][2]) >= 2:
        bonus += 30
    mg += bonus
    eg += bonus // 2
    return taper(mg, eg, phase)

def eval_stm(fen):
    pieces_all, side = parse_fen(fen)
    w = evaluate_color(pieces_all, 'w')
    b = evaluate_color(pieces_all, 'b')
    score = w - b
    return score if side == 'w' else -score

if __name__ == "__main__":
    import sys
    for fen in sys.argv[1:]:
        print(eval_stm(fen))
