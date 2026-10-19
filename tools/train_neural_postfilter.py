#!/usr/bin/env python3
"""Train the Phase 1 quantized neural MDCT postfilter & denoiser with QAT and export Rust weights."""

import math
import time
from pathlib import Path
import numpy as np
import torch
import torch.nn as nn
import torch.nn.functional as F

GLOBAL_DIM = 64
GLOBAL_H1 = 64
GLOBAL_H2 = 64
NUM_SUBBANDS = 16
BIN_HIDDEN = 16
SUBBAND_STRIDE = 1 + BIN_HIDDEN
SUBBAND_OUT = NUM_SUBBANDS * SUBBAND_STRIDE
NUM_HEADS = 8
LOCAL_DIM = 16
BIN_OUT = 8

NL_SIGN_TABLE = (-1.0, 1.0, -1.0, 1.0, 1.0, -1.0, 1.0)


def fast_sig(x: torch.Tensor) -> torch.Tensor:
    return x / (1.0 + torch.abs(x))


def build_hf_fft_matrices(n_coeffs: int, coded_bins: int, device: torch.device, overlap: int = 120):
    padding = (n_coeffs - overlap) // 2
    window = np.zeros(2 * n_coeffs, dtype=np.float64)
    window[padding + overlap : n_coeffs + padding] = 1.0
    idx = np.arange(overlap, dtype=np.float64)
    angle = (np.pi / 2.0) * (idx + 0.5) / overlap
    val = np.sin((np.pi / 2.0) * (np.sin(angle) ** 2))
    window[padding + idx.astype(int)] = val
    window[2 * n_coeffs - 1 - padding - idx.astype(int)] = val

    n = np.arange(2 * n_coeffs, dtype=np.float64)[:, None]
    k = np.arange(coded_bins, dtype=np.float64)[None, :]
    basis = window[:, None] * np.cos((np.pi / n_coeffs) * (n + 0.5 + n_coeffs / 2.0) * (k + 0.5))

    hann = np.hanning(1024)[:, None]
    freq = np.fft.rfftfreq(1024, 1.0 / 48000.0)
    hf_bands = (freq >= 4400.0) & (freq <= 8000.0)

    if n_coeffs == 960:
        w_a = basis[448 : 1472, :] * hann
        w_b = np.zeros((1024, coded_bins), dtype=np.float64)
        w_b[:572, :] = basis[928 : 1500, :] * hann[:572]
        w_c = np.zeros((1024, coded_bins), dtype=np.float64)
        w_c[452:, :] = basis[420 : 992, :] * hann[452:]
        windows = [w_a, w_b, w_c]
    else:
        w_a = np.zeros((1024, coded_bins), dtype=np.float64)
        w_a[32 : 32 + 960, :] = basis * hann[32 : 32 + 960]
        windows = [w_a]

    mats = []
    for w in windows:
        fc = np.fft.rfft(w, axis=0)[hf_bands, :].T
        mats.append(
            (
                torch.from_numpy(fc.real.astype(np.float32)).to(device),
                torch.from_numpy(fc.imag.astype(np.float32)).to(device),
            )
        )
    return mats


class QATLinear(nn.Module):
    """Per-output-channel symmetric int8 Quantization-Aware Linear layer."""

    def __init__(self, in_features: int, out_features: int, init_scale: float = 1.0):
        super().__init__()
        self.in_features = in_features
        self.out_features = out_features
        self.weight = nn.Parameter(torch.empty(out_features, in_features))
        self.bias = nn.Parameter(torch.zeros(out_features))
        nn.init.kaiming_uniform_(self.weight, a=math.sqrt(5))
        with torch.no_grad():
            self.weight.mul_(init_scale)

    def quantize_weight(self):
        max_abs = self.weight.abs().amax(dim=1, keepdim=True).clamp(min=1e-6)
        scale = max_abs / 127.0
        q = torch.clamp(torch.round(self.weight / scale), -127.0, 127.0)
        w_ste = (q - self.weight / scale).detach() + (self.weight / scale)
        return w_ste * scale, q.detach().to(torch.int8), scale.detach().squeeze(1)

    def forward(self, x: torch.Tensor) -> torch.Tensor:
        w_deq, _, _ = self.quantize_weight()
        return F.linear(x, w_deq, self.bias)


class QATMultiHeadLinear(nn.Module):
    """Batched 8-head per-channel symmetric int8 Quantization-Aware Linear layer."""

    def __init__(self, num_heads: int, in_features: int, out_features: int, init_scale: float = 1.0):
        super().__init__()
        self.num_heads = num_heads
        self.in_features = in_features
        self.out_features = out_features
        self.weight = nn.Parameter(torch.empty(num_heads, out_features, in_features))
        self.bias = nn.Parameter(torch.zeros(num_heads, out_features))
        for h in range(num_heads):
            nn.init.kaiming_uniform_(self.weight[h], a=math.sqrt(5))
        with torch.no_grad():
            self.weight.mul_(init_scale)

    def quantize_weight(self):
        max_abs = self.weight.abs().amax(dim=2, keepdim=True).clamp(min=1e-6)
        scale = max_abs / 127.0
        q = torch.clamp(torch.round(self.weight / scale), -127.0, 127.0)
        w_ste = (q - self.weight / scale).detach() + (self.weight / scale)
        return w_ste * scale, q.detach().to(torch.int8), scale.detach().squeeze(2)


class NeuralPostfilter(nn.Module):
    def __init__(self):
        super().__init__()
        self.fc1 = QATLinear(GLOBAL_DIM, GLOBAL_H1, init_scale=0.8)
        self.fc2 = QATLinear(GLOBAL_H1, GLOBAL_H2, init_scale=0.5)
        self.fc3 = QATLinear(GLOBAL_H2, SUBBAND_OUT, init_scale=0.02)
        self.head_l1 = QATMultiHeadLinear(NUM_HEADS, LOCAL_DIM, BIN_HIDDEN, init_scale=0.6)
        self.head_l2 = QATMultiHeadLinear(NUM_HEADS, BIN_HIDDEN, BIN_OUT, init_scale=0.02)
        with torch.no_grad():
            self.head_l2.bias[:, 7].fill_(0.22)

    def forward_bucket(self, batch: dict[str, torch.Tensor]) -> torch.Tensor:
        """
        Exact differentiable match of `crate::neural::refine_spectrum` in `src/neural.rs`.
        """
        global_feat = batch["global_feat"]  # (B, 64)
        local_feat = batch["local_feat"]  # (B, K, 16)
        raw_taps = batch["raw_taps"]  # (B, K, 7)
        nl_basis = batch["nl_basis"]  # (B, K)
        active_mask = batch["active_mask"]  # (B, K) bool
        is_harm = batch["is_harm"]  # (B,) int64 in {0, 1}
        edges = batch["edges"]  # [5] python list of ints
        decoded = batch["decoded"]  # (B, K)
        group_sigma = batch["group_sigma"]  # (B, 4)
        samples = batch["samples"]  # int (480 or 960)

        B, K, _ = local_feat.shape
        sub_bins = K // NUM_SUBBANDS
        k_8000 = samples // 3

        # Scale 1: Frame & Subband Context MLP
        h1 = F.relu(self.fc1(global_feat))
        h2 = F.relu(self.fc2(h1)) + h1
        subband_out = self.fc3(h2).view(B, NUM_SUBBANDS, SUBBAND_STRIDE)

        g_logit = subband_out[:, :, 0]  # (B, 16)
        subband_gains = torch.clamp(1.0 + 0.95 * fast_sig(g_logit), 0.05, 1.6)  # (B, 16)
        subband_cond = subband_out[:, :, 1:]  # (B, 16, 16)

        cond_per_bin = subband_cond.repeat_interleave(sub_bins, dim=1)  # (B, K, 16)
        gain_per_bin = subband_gains.repeat_interleave(sub_bins, dim=1)  # (B, K)

        hw1_deq, _, _ = self.head_l1.quantize_weight()  # (8, 16, 16)
        hb1 = self.head_l1.bias  # (8, 16)
        hw2_deq, _, _ = self.head_l2.quantize_weight()  # (8, 8, 16)
        hb2 = self.head_l2.bias  # (8, 8)

        out_bins = torch.empty((B, K, BIN_OUT), device=local_feat.device, dtype=local_feat.dtype)

        for g in range(4):
            start, end = edges[g], edges[g + 1]
            head_idx = is_harm * 4 + g  # (B,)
            w1_g = hw1_deq[head_idx]  # (B, 16, 16)
            b1_g = hb1[head_idx].unsqueeze(1)  # (B, 1, 16)
            w2_g = hw2_deq[head_idx]  # (B, 8, 16)
            b2_g = hb2[head_idx].unsqueeze(1)  # (B, 1, 8)

            lf_g = local_feat[:, start:end, :]  # (B, Kg, 16)
            cond_g = cond_per_bin[:, start:end, :]  # (B, Kg, 16)

            pre1 = torch.bmm(lf_g, w1_g.transpose(1, 2)) + b1_g + cond_g
            h_bin = F.relu(pre1)
            out_g = torch.bmm(h_bin, w2_g.transpose(1, 2)) + b2_g  # (B, Kg, 8)
            out_bins[:, start:end, :] = out_g

        sig_out = fast_sig(out_bins)
        delta_g = 0.8 * sig_out[:, :, 0]
        w_m3 = 0.35 * sig_out[:, :, 1]
        w_m2 = 0.35 * sig_out[:, :, 2]
        w_m1 = 0.35 * sig_out[:, :, 3]
        w_p1 = 0.35 * sig_out[:, :, 4]
        w_p2 = 0.35 * sig_out[:, :, 5]
        w_p3 = 0.35 * sig_out[:, :, 6]
        w_nl = 0.5 * sig_out[:, :, 7]  # nl_basis is already gated to [k_4400, k_8000)

        filtered = (
            (1.0 + delta_g) * raw_taps[:, :, 3]
            + w_m3 * raw_taps[:, :, 0]
            + w_m2 * raw_taps[:, :, 1]
            + w_m1 * raw_taps[:, :, 2]
            + w_p1 * raw_taps[:, :, 4]
            + w_p2 * raw_taps[:, :, 5]
            + w_p3 * raw_taps[:, :, 6]
            + w_nl * nl_basis
        )
        refined = torch.where(active_mask, gain_per_bin * filtered, torch.zeros_like(filtered))

        # Relative-energy-aware coarse-band RMS floor up to 8000 Hz matching src/neural.rs
        max_sigma = group_sigma.amax(dim=1, keepdim=True).clamp(min=1e-6)
        slices = [refined[:, edges[0] : edges[1]]]
        for g in range(1, 4):
            start, end = edges[g], edges[g + 1]
            if start < k_8000:
                end_8k = min(end, k_8000)
                dim = float(max(end_8k - start, 1))
                orig_sub = decoded[:, start:end_8k]
                ref_sub = refined[:, start:end_8k]
                orig_sigma = torch.sqrt((orig_sub * orig_sub).sum(dim=1, keepdim=True) / dim + 1e-12)
                ref_sigma = torch.sqrt((ref_sub * ref_sub).sum(dim=1, keepdim=True) / dim + 1e-12)
                rel = torch.clamp(orig_sigma / max_sigma, 0.0, 1.0)
                base_ratio = 1.05 if g == 3 else 1.00
                target_sigma = orig_sigma * (base_ratio - 0.26 * torch.sqrt(rel + 1e-12))
                scale = torch.where(
                    (ref_sigma < target_sigma) & (orig_sigma > 1e-6),
                    target_sigma / ref_sigma.clamp(min=1e-12),
                    torch.ones_like(ref_sigma),
                )
                slices.append(ref_sub * scale)
                if end_8k < end:
                    slices.append(refined[:, end_8k:end])
            else:
                slices.append(refined[:, start:end])

        return torch.cat(slices, dim=1)


def prepare_gpu_bucket(npz_data, cb: int, samples_filter: int, device: torch.device, max_frames: int = 45000):
    prefix = f"b{cb}_"
    if prefix + "decoded" not in npz_data:
        return None
    s_all = npz_data[prefix + "samples"]
    idx_all = np.where(s_all == samples_filter)[0]
    if len(idx_all) == 0:
        return None
    if len(idx_all) > max_frames:
        rng = np.random.default_rng(42 + cb + samples_filter)
        idx = np.sort(rng.choice(idx_all, size=max_frames, replace=False))
    else:
        idx = idx_all

    edges = [int(x) for x in npz_data[prefix + "edges"][idx[0]]]
    decoded = torch.from_numpy(npz_data[prefix + "decoded"][idx]).to(device, dtype=torch.float32)
    target_in = torch.from_numpy(npz_data[prefix + "target_in"][idx]).to(device, dtype=torch.float32)
    target_clean = torch.from_numpy(npz_data[prefix + "target_clean"][idx]).to(device, dtype=torch.float32)
    global_feat = torch.from_numpy(npz_data[prefix + "global_feat"][idx]).to(device, dtype=torch.float32)
    group_sigma = torch.from_numpy(npz_data[prefix + "group_sigma"][idx]).to(device, dtype=torch.float32)
    codes = torch.from_numpy(npz_data[prefix + "codes"][idx]).to(device, dtype=torch.int32)
    is_harm = torch.from_numpy(npz_data[prefix + "is_harm"][idx]).to(device, dtype=torch.int64)
    cond_id = torch.from_numpy(npz_data[prefix + "cond_id"][idx]).to(device, dtype=torch.int64)

    B, K = decoded.shape
    sub_bins = K // NUM_SUBBANDS
    k_4400 = (samples_filter * 11) // 60
    k_8000 = samples_filter // 3

    norm_x = torch.zeros_like(decoded)
    group_valid = torch.zeros((B, K), device=device, dtype=torch.bool)
    sigma_per_bin = torch.zeros_like(decoded)

    for g in range(4):
        s, e = edges[g], edges[g + 1]
        valid_g = (codes[:, g] > 0) & (group_sigma[:, g] > 1e-6)
        sig_g = group_sigma[:, g].unsqueeze(1)
        inv_sig = torch.where(valid_g.unsqueeze(1), 1.0 / sig_g.clamp(min=1e-12), torch.zeros_like(sig_g))
        nx = torch.clamp(decoded[:, s:e] * inv_sig, -8.0, 8.0)
        norm_x[:, s:e] = nx
        group_valid[:, s:e] = valid_g.unsqueeze(1).expand(-1, e - s)
        sigma_per_bin[:, s:e] = sig_g.expand(-1, e - s)

    padded_norm = F.pad(norm_x, (3, 3), mode="constant", value=0.0)
    taps = padded_norm.unfold(1, 7, 1)  # (B, K, 7)

    padded_raw = F.pad(decoded, (3, 3), mode="constant", value=0.0)
    raw_taps = padded_raw.unfold(1, 7, 1)  # (B, K, 7)

    energy_sum = (taps * taps).sum(dim=-1)  # (B, K)
    active_mask = group_valid & (energy_sum > 1e-12)

    # Exact priority order [3, 2, 4, 1, 5, 0, 6] for s_ref and antisymmetric quadrature s_nl
    best_abs = torch.full((B, K), -1.0, device=device, dtype=torch.float32)
    s_ref = torch.ones((B, K), device=device, dtype=torch.float32)
    s_nl = torch.ones((B, K), device=device, dtype=torch.float32)
    for d in (3, 2, 4, 1, 5, 0, 6):
        td = taps[:, :, d]
        a = td.abs()
        better = a > best_abs
        best_abs = torch.where(better, a, best_abs)
        s_d = torch.where(td >= 0.0, torch.ones_like(td), -torch.ones_like(td))
        s_ref = torch.where(better, s_d, s_ref)
        s_nl = torch.where(better, s_d * NL_SIGN_TABLE[d], s_nl)

    local_feat = torch.zeros((B, K, LOCAL_DIM), device=device, dtype=torch.float32)
    local_feat[:, :, 0:7] = s_ref.unsqueeze(-1) * taps
    local_feat[:, :, 7:14] = taps.abs()
    local_feat[:, :, 14] = (taps[:, :, 3].abs() > 1e-5).to(torch.float32)
    pos = (torch.arange(K, device=device, dtype=torch.float32) % sub_bins) / float(sub_bins)
    local_feat[:, :, 15] = pos.unsqueeze(0).expand(B, K)

    e_loc = torch.clamp(torch.sqrt(energy_sum * (1.0 / 7.0)), max=4.0)
    nl_basis = s_nl * sigma_per_bin * e_loc
    nl_gate = torch.zeros((1, K), device=device, dtype=torch.float32)
    nl_gate[:, k_4400 : min(k_8000, K)] = 1.0
    nl_basis = nl_basis * nl_gate

    hf_mats = build_hf_fft_matrices(samples_filter, K, device)

    return {
        "samples": samples_filter,
        "coded_bins": K,
        "edges": edges,
        "global_feat": global_feat,
        "local_feat": local_feat,
        "raw_taps": raw_taps,
        "nl_basis": nl_basis,
        "active_mask": active_mask,
        "is_harm": is_harm,
        "cond_id": cond_id,
        "group_sigma": group_sigma,
        "decoded": decoded,
        "target_in": target_in,
        "target_clean": target_clean,
        "hf_mats": hf_mats,
    }


def compute_loss(
    refined: torch.Tensor,
    batch: dict[str, torch.Tensor],
) -> torch.Tensor:
    target_in = batch["target_in"]
    target_clean = batch["target_clean"]
    decoded = batch["decoded"]
    edges = batch["edges"]
    is_harm = batch["is_harm"]
    hf_mats = batch["hf_mats"]
    K = refined.shape[1]

    act = (decoded * decoded).sum(dim=1) > 1e-4
    if not act.any():
        return ((refined - decoded) ** 2).mean()

    # Blend target: 94% encoder input (for waveform SNR & LSD) + 6% clean reference (for STOI/ESTOI)
    target = 0.94 * target_in + 0.06 * target_clean

    # 1. Corpus-aggregate & frame-level normalized MDCT MSE (balanced across pulse and harmonic)
    frame_mse = ((refined - target) ** 2).sum(dim=1)
    base_mse = ((decoded - target) ** 2).sum(dim=1).clamp(min=1e-5)

    p_mask = act & (is_harm == 0)
    h_mask = act & (is_harm == 1)
    if p_mask.any() and h_mask.any():
        loss_agg_snr = 0.75 * (frame_mse[p_mask].sum() / base_mse[p_mask].sum()) + 0.25 * (
            frame_mse[h_mask].sum() / base_mse[h_mask].sum()
        )
        loss_frame_snr = 0.75 * (frame_mse[p_mask] / base_mse[p_mask]).mean() + 0.25 * (
            frame_mse[h_mask] / base_mse[h_mask]
        ).mean()
    else:
        loss_agg_snr = frame_mse[act].sum() / base_mse[act].sum()
        loss_frame_snr = (frame_mse[act] / base_mse[act]).mean()

    # 2. Coarse-band normalized MDCT MSE on g=0, 1, 2 (0..4000 Hz where STOI/ESTOI and 98% of energy live)
    loss_band_snr = 0.0
    for g in range(3):
        s, e = edges[g], edges[g + 1]
        b_mse = ((refined[:, s:e] - target[:, s:e]) ** 2).sum(dim=1)
        b_base = ((decoded[:, s:e] - target[:, s:e]) ** 2).sum(dim=1).clamp(min=1e-5)
        loss_band_snr = loss_band_snr + (1.0 / 3.0) * (b_mse[act] / b_base[act]).mean()

    # 3. Subband & fine 5-bin Log-Spectral Distance (improves STOI/ESTOI envelope correlation)
    sub_bins = K // NUM_SUBBANDS
    ref_sub = refined.view(-1, NUM_SUBBANDS, sub_bins)
    tgt_sub = target.view(-1, NUM_SUBBANDS, sub_bins)
    e_ref = torch.log10((ref_sub * ref_sub).mean(dim=-1) + 1e-7)
    e_tgt = torch.log10((tgt_sub * tgt_sub).mean(dim=-1) + 1e-7)
    loss_sub_lsd = F.smooth_l1_loss(e_ref[act], e_tgt[act], beta=0.05)

    fine_groups = K // 5
    ref_fine = refined[:, : fine_groups * 5].view(-1, fine_groups, 5)
    tgt_fine = target[:, : fine_groups * 5].view(-1, fine_groups, 5)
    ef_ref = torch.log10((ref_fine * ref_fine).mean(dim=-1) + 1e-7)
    ef_tgt = torch.log10((tgt_fine * tgt_fine).mean(dim=-1) + 1e-7)
    loss_fine_lsd = F.smooth_l1_loss(ef_ref[act], ef_tgt[act], beta=0.05)

    # 4. High-frequency (4400..8000 Hz) multi-window Hann-FFT Log-Spectral Distance
    loss_hf_lsd = 0.0
    for re_m, im_m in hf_mats:
        ta = torch.sqrt((target_in @ re_m) ** 2 + (target_in @ im_m) ** 2 + 1e-12)
        da = torch.sqrt((decoded @ re_m) ** 2 + (decoded @ im_m) ** 2 + 1e-12)
        ra = torch.sqrt((refined @ re_m) ** 2 + (refined @ im_m) ** 2 + 1e-12)
        fl = torch.clamp(ta.amax(dim=1, keepdim=True) * 0.005, min=1e-6)
        bl = torch.sqrt(torch.mean((20.0 * torch.log10(torch.maximum(da, fl) / torch.maximum(ta, fl))) ** 2, dim=1) + 1e-6)
        rl = torch.sqrt(torch.mean((20.0 * torch.log10(torch.maximum(ra, fl) / torch.maximum(ta, fl))) ** 2, dim=1) + 1e-6)
        loss_hf_lsd = loss_hf_lsd + (rl[act] - bl[act]).mean() / len(hf_mats)

    return (
        0.45 * loss_agg_snr
        + 0.35 * loss_frame_snr
        + 0.20 * loss_band_snr
        + 0.08 * loss_sub_lsd
        + 0.05 * loss_fine_lsd
        + 0.035 * loss_hf_lsd
    )


@torch.no_grad()
def evaluate_buckets(model: NeuralPostfilter, val_buckets: dict[tuple[int, int], dict[str, torch.Tensor]]):
    model.eval()
    total_db_gains = []
    report = []
    for (samples, cb), b in sorted(val_buckets.items()):
        refined = model.forward_bucket(b)
        target_in = b["target_in"]
        decoded = b["decoded"]
        act = (decoded * decoded).sum(dim=1) > 1e-4

        err_base = ((decoded[act] - target_in[act]) ** 2).sum(dim=1).clamp(min=1e-7)
        err_ref = ((refined[act] - target_in[act]) ** 2).sum(dim=1).clamp(min=1e-7)
        frame_gain_db = 10.0 * torch.log10(err_base / err_ref)
        mean_gain_db = frame_gain_db.mean().item()

        tot_base = err_base.sum().item()
        tot_ref = err_ref.sum().item()
        agg_gain_db = 10.0 * math.log10(tot_base / tot_ref)

        # Check HF LSD delta
        re_m, im_m = b["hf_mats"][0]
        ta = torch.sqrt((target_in[act] @ re_m) ** 2 + (target_in[act] @ im_m) ** 2 + 1e-12)
        da = torch.sqrt((decoded[act] @ re_m) ** 2 + (decoded[act] @ im_m) ** 2 + 1e-12)
        ra = torch.sqrt((refined[act] @ re_m) ** 2 + (refined[act] @ im_m) ** 2 + 1e-12)
        fl = torch.clamp(ta.amax(dim=1, keepdim=True) * 0.005, min=1e-6)
        bl = torch.sqrt(torch.mean((20.0 * torch.log10(torch.maximum(da, fl) / torch.maximum(ta, fl))) ** 2, dim=1) + 1e-6)
        rl = torch.sqrt(torch.mean((20.0 * torch.log10(torch.maximum(ra, fl) / torch.maximum(ta, fl))) ** 2, dim=1) + 1e-6)
        d_hf_lsd = (rl - bl).mean().item()

        total_db_gains.append(agg_gain_db - 0.12 * d_hf_lsd)
        report.append(f"({samples},{cb}): agg={agg_gain_db:+.3f}dB mean={mean_gain_db:+.3f}dB hfLSD={d_hf_lsd:+.2f}dB")
    return float(np.mean(total_db_gains)), " | ".join(report)


def format_i8_2d(name: str, doc: str, arr: np.ndarray, dim_r: str, dim_c: str) -> str:
    lines = [f"/// {doc}", f"pub(crate) static {name}: [[i8; {dim_c}]; {dim_r}] = ["]
    for row in arr:
        vals = ", ".join(str(int(v)) for v in row)
        lines.append(f"    [{vals}],")
    lines.append("];")
    return "\n".join(lines)


def format_f32_1d(name: str, doc: str, arr: np.ndarray, dim: str) -> str:
    vals = ", ".join(f"{float(v):.8e}" for v in arr)
    return f"/// {doc}\npub(crate) static {name}: [f32; {dim}] = [{vals}];"


def format_i8_3d(name: str, doc: str, arr: np.ndarray, d0: str, d1: str, d2: str) -> str:
    lines = [f"/// {doc}", f"pub(crate) static {name}: [[[i8; {d2}]; {d1}]; {d0}] = ["]
    for mat in arr:
        lines.append("    [")
        for row in mat:
            vals = ", ".join(str(int(v)) for v in row)
            lines.append(f"        [{vals}],")
        lines.append("    ],")
    lines.append("];")
    return "\n".join(lines)


def format_f32_2d(name: str, doc: str, arr: np.ndarray, d0: str, d1: str) -> str:
    lines = [f"/// {doc}", f"pub(crate) static {name}: [[f32; {d1}]; {d0}] = ["]
    for row in arr:
        vals = ", ".join(f"{float(v):.8e}" for v in row)
        lines.append(f"    [{vals}],")
    lines.append("];")
    return "\n".join(lines)


def export_rust_weights(model: NeuralPostfilter, out_path: Path):
    _, w1_q, s1 = model.fc1.quantize_weight()
    b1 = model.fc1.bias.detach()
    _, w2_q, s2 = model.fc2.quantize_weight()
    b2 = model.fc2.bias.detach()
    _, w3_q, s3 = model.fc3.quantize_weight()
    b3 = model.fc3.bias.detach()

    _, hw1_q, hs1 = model.head_l1.quantize_weight()
    hb1 = model.head_l1.bias.detach()
    _, hw2_q, hs2 = model.head_l2.quantize_weight()
    hb2 = model.head_l2.bias.detach()

    parts = [
        "// SPDX-License-Identifier: MIT",
        "//! Quantized `i8` weights and per-channel scales for the neural MDCT postfilter.",
        "#![allow(clippy::excessive_precision)]",
        "",
        "/// Input feature count for the frame-level subband envelope network.",
        "pub(crate) const GLOBAL_DIM: usize = 64;",
        "/// Hidden dimension of the first frame-level linear layer.",
        "pub(crate) const GLOBAL_H1: usize = 64;",
        "/// Hidden dimension of the second frame-level linear layer.",
        "pub(crate) const GLOBAL_H2: usize = 64;",
        "/// Number of fine spectral subbands across the coded MDCT spectrum.",
        "pub(crate) const NUM_SUBBANDS: usize = 16;",
        "/// Hidden dimension of the per-bin local convolutional head.",
        "pub(crate) const BIN_HIDDEN: usize = 16;",
        "/// Output dimension per subband from the frame-level network (`1` gate + `BIN_HIDDEN` conditioning).",
        "pub(crate) const SUBBAND_STRIDE: usize = 1 + BIN_HIDDEN;",
        "/// Total output dimension of the frame-level network (`NUM_SUBBANDS * SUBBAND_STRIDE`).",
        "pub(crate) const SUBBAND_OUT: usize = NUM_SUBBANDS * SUBBAND_STRIDE;",
        "/// Number of specialized local spectral heads (`2` modes * `4` coarse bands).",
        "pub(crate) const NUM_HEADS: usize = 8;",
        "/// Input feature count for the per-bin local spectral head.",
        "pub(crate) const LOCAL_DIM: usize = 16;",
        "/// Output dimension of the per-bin local spectral head.",
        "pub(crate) const BIN_OUT: usize = 8;",
        "",
        format_i8_2d("W1", "Layer 1 quantized weights (`GLOBAL_H1 x GLOBAL_DIM`).", w1_q.cpu().numpy(), "GLOBAL_H1", "GLOBAL_DIM"),
        format_f32_1d("S1", "Layer 1 per-channel dequantization scales.", s1.cpu().numpy(), "GLOBAL_H1"),
        format_f32_1d("B1", "Layer 1 biases.", b1.cpu().numpy(), "GLOBAL_H1"),
        "",
        format_i8_2d("W2", "Layer 2 quantized weights (`GLOBAL_H2 x GLOBAL_H1`).", w2_q.cpu().numpy(), "GLOBAL_H2", "GLOBAL_H1"),
        format_f32_1d("S2", "Layer 2 per-channel dequantization scales.", s2.cpu().numpy(), "GLOBAL_H2"),
        format_f32_1d("B2", "Layer 2 biases.", b2.cpu().numpy(), "GLOBAL_H2"),
        "",
        format_i8_2d("W3", "Layer 3 quantized weights (`SUBBAND_OUT x GLOBAL_H2`).", w3_q.cpu().numpy(), "SUBBAND_OUT", "GLOBAL_H2"),
        format_f32_1d("S3", "Layer 3 per-channel dequantization scales.", s3.cpu().numpy(), "SUBBAND_OUT"),
        format_f32_1d("B3", "Layer 3 biases.", b3.cpu().numpy(), "SUBBAND_OUT"),
        "",
        format_i8_3d("HEAD_W1", "Local head layer 1 quantized weights (`NUM_HEADS x BIN_HIDDEN x LOCAL_DIM`).", hw1_q.cpu().numpy(), "NUM_HEADS", "BIN_HIDDEN", "LOCAL_DIM"),
        format_f32_2d("HEAD_S1", "Local head layer 1 per-channel dequantization scales.", hs1.cpu().numpy(), "NUM_HEADS", "BIN_HIDDEN"),
        format_f32_2d("HEAD_B1", "Local head layer 1 biases.", hb1.cpu().numpy(), "NUM_HEADS", "BIN_HIDDEN"),
        "",
        format_i8_3d("HEAD_W2", "Local head layer 2 quantized weights (`NUM_HEADS x BIN_OUT x BIN_HIDDEN`).", hw2_q.cpu().numpy(), "NUM_HEADS", "BIN_OUT", "BIN_HIDDEN"),
        format_f32_2d("HEAD_S2", "Local head layer 2 per-channel dequantization scales.", hs2.cpu().numpy(), "NUM_HEADS", "BIN_OUT"),
        format_f32_2d("HEAD_B2", "Local head layer 2 biases.", hb2.cpu().numpy(), "NUM_HEADS", "BIN_OUT"),
        "",
    ]
    out_path.write_text("\n".join(parts))
    print(f"Exported quantized Rust weights to {out_path} ({out_path.stat().st_size / 1024:.1f} KB)")


def main():
    torch.manual_seed(20261019)
    np.random.seed(20261019)
    if torch.cuda.is_available():
        device = torch.device("cuda")
    elif torch.backends.mps.is_available():
        device = torch.device("mps")
    else:
        device = torch.device("cpu")
    print(f"Using device: {device}")

    train_path = Path("/private/tmp/snacka-data/train_dataset.npz")
    val_path = Path("/private/tmp/snacka-data/val_dataset.npz")
    if not train_path.exists():
        train_path = Path("train_dataset.npz")
        val_path = Path("val_dataset.npz")

    train_npz = np.load(train_path)
    val_npz = np.load(val_path)

    bucket_specs = [(480, 160), (480, 240), (480, 480), (960, 320), (960, 480), (960, 960)]
    train_buckets = {}
    val_buckets = {}
    for samples, cb in bucket_specs:
        tb = prepare_gpu_bucket(train_npz, cb, samples, device, max_frames=36000)
        if tb is not None:
            train_buckets[(samples, cb)] = tb
            print(f"Loaded train bucket ({samples}, {cb}): {tb['decoded'].shape[0]} frames")
        vb = prepare_gpu_bucket(val_npz, cb, samples, device, max_frames=12000)
        if vb is not None:
            val_buckets[(samples, cb)] = vb
            print(f"Loaded val bucket ({samples}, {cb}): {vb['decoded'].shape[0]} frames")

    model = NeuralPostfilter().to(device)
    init_score, init_rep = evaluate_buckets(model, val_buckets)
    print(f"[Epoch 000] Val composite score: {init_score:+.4f} | {init_rep}", flush=True)

    optimizer = torch.optim.AdamW(model.parameters(), lr=3.5e-3, weight_decay=1e-4)
    epochs = 60
    batch_size = 1024
    scheduler = torch.optim.lr_scheduler.CosineAnnealingLR(optimizer, T_max=epochs, eta_min=5e-5)

    best_score = -1e9
    best_state = None
    start_time = time.time()

    for epoch in range(1, epochs + 1):
        model.train()
        epoch_loss = 0.0
        steps = 0

        bucket_iters = {}
        for key, b in train_buckets.items():
            N = b["decoded"].shape[0]
            perm = torch.randperm(N, device=device)
            bucket_iters[key] = [perm[i : i + batch_size] for i in range(0, N, batch_size)]

        max_len = max(len(v) for v in bucket_iters.values())
        for step_idx in range(max_len):
            optimizer.zero_grad(set_to_none=True)
            step_loss = 0.0
            active_b = 0
            for key, chunks in bucket_iters.items():
                if step_idx >= len(chunks):
                    continue
                idx = chunks[step_idx]
                tb = train_buckets[key]
                sub_batch = {
                    "samples": tb["samples"],
                    "coded_bins": tb["coded_bins"],
                    "edges": tb["edges"],
                    "global_feat": tb["global_feat"][idx],
                    "local_feat": tb["local_feat"][idx],
                    "raw_taps": tb["raw_taps"][idx],
                    "nl_basis": tb["nl_basis"][idx],
                    "active_mask": tb["active_mask"][idx],
                    "is_harm": tb["is_harm"][idx],
                    "cond_id": tb["cond_id"][idx],
                    "group_sigma": tb["group_sigma"][idx],
                    "decoded": tb["decoded"][idx],
                    "target_in": tb["target_in"][idx],
                    "target_clean": tb["target_clean"][idx],
                    "hf_mats": tb["hf_mats"],
                }
                refined = model.forward_bucket(sub_batch)
                loss = compute_loss(refined, sub_batch)
                step_loss = step_loss + loss
                active_b += 1

            step_loss = step_loss / max(active_b, 1)
            step_loss.backward()
            torch.nn.utils.clip_grad_norm_(model.parameters(), 1.0)
            optimizer.step()
            epoch_loss += step_loss.item()
            steps += 1

        scheduler.step()

        if epoch % 5 == 0 or epoch == 1 or epoch == epochs:
            val_score, val_rep = evaluate_buckets(model, val_buckets)
            elapsed = time.time() - start_time
            print(
                f"[Epoch {epoch:03d}/{epochs:03d}] ({elapsed:.1f}s) loss={epoch_loss/max(steps,1):.4f} | Val: {val_score:+.4f} | {val_rep}",
                flush=True,
            )
            if val_score > best_score:
                best_score = val_score
                best_state = {k: v.detach().cpu().clone() for k, v in model.state_dict().items()}

    if best_state is not None:
        model.load_state_dict({k: v.to(device) for k, v in best_state.items()})
    final_score, final_rep = evaluate_buckets(model, val_buckets)
    print(f"\nBest Val Composite Score: {final_score:+.4f} | {final_rep}", flush=True)
    out_file = Path("src/neural_weights.rs") if Path("src").is_dir() else Path("neural_weights.rs")
    export_rust_weights(model, out_file)


if __name__ == "__main__":
    main()
