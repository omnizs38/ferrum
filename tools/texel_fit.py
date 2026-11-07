#!/usr/bin/env python3
"""Fit the texel features (from tools/texel.py) to Stockfish score labels.

Loss: Huber(s - y) + lambda * ||W - W0||^2, where W0 is the starting
(hand-written) evaluation and s is the tapered score from the side to move.

Writes results/texel_fit.json.
"""
import json, sys
import numpy as np

D = np.load("results/texel_features.npz")
phase = D["phase"].astype(np.float64) / 256.0
phase = np.minimum(phase, 1.0)
mg_diff = D["mg_diff"].astype(np.float64)
eg_diff = D["eg_diff"].astype(np.float64)
mat_diff = D["mat_diff"].astype(np.float64)
emat_diff = D["emat_diff"].astype(np.float64)
pos_diff = D["pos_diff"].astype(np.float64)
sign = D["sign"].astype(np.float64)
y = D["y"].astype(np.float64)

# Starting weights: hand-written tables (MG_VALUE, EG_VALUE, PST_MG, PST_EG,
# positional constants as used by ferrum).
MG_VALUE = np.array([82, 337, 365, 477, 1025, 0], dtype=np.float64)
EG_VALUE = np.array([94, 281, 297, 512, 936, 0], dtype=np.float64)
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
W0_p = np.array(PST_MG + PST_EG, dtype=np.float64).flatten()  # 384 mg then 384 eg
# index mapping: for color-specific sq, pst_index was already applied by the
# feature builder, so mg_diff[ k*64 + pst_index ] uses the same PST_MG table.
W0_mat = MG_VALUE.astype(np.float64); W0_emat = EG_VALUE.astype(np.float64)
# positional: [N,B,R,Q mobility, connected, blocked, doubled, isolated, passed-sum, open, semi, shield, missing, bpair]
W0_pos = np.array([4, 3, 2, 1, 12, 8, 12, 10, 1.0, 15, 8, 15, 10, 30], dtype=np.float64)

W = {
    "mat": W0_mat.copy(),
    "emat": W0_emat.copy(),
    "pmg": W0_p[:384].copy(),
    "peg": W0_p[384:].copy(),
    "pos": W0_pos.copy(),
}

def forward(W):
    mg = mat_diff @ W["mat"] + mg_diff @ W["pmg"] + pos_diff @ W["pos"]
    eg = emat_diff @ W["emat"] + eg_diff @ W["peg"] + 0.5 * (pos_diff @ W["pos"])
    s = (mg * phase + eg * (1.0 - phase)) * sign
    return s

def huber_grad(r, delta=150.0):
    g = r.copy()
    big = np.abs(r) > delta
    g[big] = delta * np.sign(r[big])
    return g

def fit(lam, epochs=800, lr=0.1, seed=42, beta1=0.9, beta2=0.999, eps=1e-8, mask=None):
    # Adam optimizer: per-weight normalized step, immune to feature scale.
    if mask is None:
        mask = np.ones(len(y), dtype=bool)
    W = {
        "mat": W0_mat.copy(),
        "emat": W0_emat.copy(),
        "pmg": W0_p[:384].copy(),
        "peg": W0_p[384:].copy(),
        "pos": W0_pos.copy(),
    }
    m = {k: np.zeros_like(v) for k, v in W.items()}
    v = {k: np.zeros_like(v) for k, v in W.items()}
    # start WD anchors toward W0
    anchors = {
        "mat": W0_mat,
        "emat": W0_emat,
        "pmg": W0_p[:384],
        "peg": W0_p[384:],
        "pos": W0_pos,
    }
    N = len(y)
    idxs = np.where(mask)[0]
    sub = {
        "phase": phase[idxs], "sign": sign[idxs],
        "mg_diff": mg_diff[idxs], "eg_diff": eg_diff[idxs],
        "mat_diff": mat_diff[idxs], "emat_diff": emat_diff[idxs],
        "pos_diff": pos_diff[idxs], "y": y[idxs],
    }
    phase_s, sign_s = sub["phase"], sub["sign"]
    mg_s, eg_s = sub["mg_diff"], sub["eg_diff"]
    mat_s, emat_s = sub["mat_diff"], sub["emat_diff"]
    pos_s, y_s = sub["pos_diff"], sub["y"]
    for epoch in range(epochs):
        mg = mat_s @ W["mat"] + mg_s @ W["pmg"] + pos_s @ W["pos"]
        eg = emat_s @ W["emat"] + eg_s @ W["peg"] + 0.5 * (pos_s @ W["pos"])
        s = (mg * phase_s + eg * (1.0 - phase_s)) * sign_s
        r = s - y_s
        g = huber_grad(r) / N
        dg_mg = g * phase_s * sign_s
        dg_eg = g * (1.0 - phase_s) * sign_s
        grads = {
            "mat": mat_s.T @ dg_mg,
            "emat": emat_s.T @ dg_eg,
            "pmg": mg_s.T @ dg_mg,
            "peg": eg_s.T @ dg_eg,
            "pos": pos_s.T @ (dg_mg + 0.5 * dg_eg),
        }
        for k in W:
            grads[k] = grads[k] + 2 * lam * (W[k] - anchors[k])
        t = epoch + 1
        for k in W:
            m[k] = beta1 * m[k] + (1 - beta1) * grads[k]
            v[k] = beta2 * v[k] + (1 - beta2) * grads[k] ** 2
            mhat = m[k] / (1 - beta1 ** t)
            vhat = v[k] / (1 - beta2 ** t)
            W[k] -= lr * mhat / (np.sqrt(vhat) + eps)
        if epoch % 50 == 0 or epoch == epochs - 1:
            mg = mat_s @ W["mat"] + mg_s @ W["pmg"] + pos_s @ W["pos"]
            eg = emat_s @ W["emat"] + eg_s @ W["peg"] + 0.5 * (pos_s @ W["pos"])
            s = (mg * phase_s + eg * (1.0 - phase_s)) * sign_s
            rmse = float(np.sqrt(np.mean((s - y_s) ** 2)))
            print(f"epoch {epoch}: rmse {rmse:.1f}")
    return W

def holdout_report(W, name, mask=None):
    if mask is None:
        mask = np.ones(len(y), dtype=bool)
    mg = mat_diff[mask] @ W["mat"] + mg_diff[mask] @ W["pmg"] + pos_diff[mask] @ W["pos"]
    eg = emat_diff[mask] @ W["emat"] + eg_diff[mask] @ W["peg"] + 0.5 * (pos_diff[mask] @ W["pos"])
    s = (mg * phase[mask] + eg * (1.0 - phase[mask])) * sign[mask]
    rmse = float(np.sqrt(np.mean((s - y[mask]) ** 2)))
    mae = float(np.mean(np.abs(s - y[mask])))
    print(f"{name}: rmse={rmse:.1f} mae={mae:.1f}")

if __name__ == "__main__":
    holdout_report(W, "start (hand-written)")
    rng = np.random.default_rng(7)
    perm = rng.permutation(len(y))
    cut = int(0.9 * len(y))
    train_mask = np.zeros(len(y), dtype=bool)
    hold_mask = np.zeros(len(y), dtype=bool)
    train_mask[perm[:cut]] = True
    hold_mask[perm[cut:]] = True
    for lam in [1e-3]:
        print("lambda =", lam, "(train 90%, holdout 10%)")
        Wf = fit(lam, epochs=800, lr=0.1, mask=train_mask)
        holdout_report(Wf, "train", train_mask)
        holdout_report(Wf, "holdout", hold_mask)
        # dump tables for eval.rs
        mg_tab = Wf["pmg"].reshape(6, 64).astype(int).tolist()
        eg_tab = Wf["peg"].reshape(6, 64).astype(int).tolist()
        out = {
            "mg_value": Wf["mat"].astype(int).tolist(),
            "eg_value": Wf["emat"].astype(int).tolist(),
            "pst_mg": mg_tab,
            "pst_eg": eg_tab,
            "positional": Wf["pos"].astype(float).tolist(),
        }
        with open("results/texel_fit.json", "w") as fh:
            json.dump(out, fh)
        print("wrote results/texel_fit.json")
