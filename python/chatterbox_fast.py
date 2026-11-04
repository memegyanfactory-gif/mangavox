#!/usr/bin/env python3
"""
Fast Chatterbox sampling for the `chatterbox_tts.py serve` worker (engine venv, CUDA only).

Chatterbox's own `T3.inference` speaks one sentence at a time and spends most of its time
in Python: every speech token runs HuggingFace's Llama with `output_attentions=True` (which
forces the slow eager attention) and all hidden states, then syncs with the CPU to check for
the stop token. This module runs the same model and the same sampling maths, but:

  * many sentences at once, each with its own CFG pair, emotion, temperature and seed;
  * a static KV cache and one CUDA graph per decode step (no Python per layer, no sync);
  * per-sentence random streams (exponential-race sampling == multinomial), so a sentence
    sounds the same whatever batch it was made in.

The maths follows chatterbox 0.1.7 `T3.inference`: CFG logits `cond + w * (cond - uncond)`,
repetition penalty over the tokens so far (start token included), temperature, min-p, and a
prefix of [conditioning, text, start, start]. Anything it does not cover (top_p < 1) goes
back to `model.generate`.
"""
from __future__ import annotations

import math
import os
import shutil
import subprocess
import time

import torch

try:
    import gpu_power
except ImportError:
    gpu_power = None
import torch.nn.functional as F

STEPS_PER_BLOCK = 32  # noise is drawn, and "all done?" checked, once per block of steps


def _round_up(n: int, m: int) -> int:
    return (n + m - 1) // m * m


def _env_float(name, default, lo, hi):
    try:
        return min(hi, max(lo, float(os.environ.get(name, default))))
    except ValueError:
        return default


class GpuGuard:
    """Keeps voicing from holding the GPU flat out: after each piece of GPU work it rests for
    a share of that time (`MANGAVOX_GPU_DUTY`, default 0.65 = busy at most 65% of the time, less
    when the GPU power governor, gpu_power, needs it to hold the card under its watt target),
    and while the GPU is at `MANGAVOX_GPU_MAX_TEMP` (default 80 C) or hotter it waits until the
    GPU has cooled 8 degrees. The temperature comes from nvidia-smi every few seconds."""

    def __init__(self):
        self.duty = _env_float("MANGAVOX_GPU_DUTY", 0.65, 0.2, 1.0)
        self.gov = gpu_power.governor("voice") if gpu_power else None
        self.max_temp = _env_float("MANGAVOX_GPU_MAX_TEMP", 80.0, 60.0, 90.0)
        self.temp = None
        self.cooled_s = 0.0
        smi = shutil.which("nvidia-smi")
        if not smi and os.name == "nt":
            smi = os.path.join(os.environ.get("SystemRoot", r"C:\Windows"), "System32", "nvidia-smi.exe")
        self._smi = smi if smi and os.path.isfile(smi) else None
        self._last = 0.0

    def read_temp(self):
        if not self._smi:
            return None
        try:
            r = subprocess.run([self._smi, "--query-gpu=temperature.gpu", "--format=csv,noheader,nounits"], capture_output=True,
                               text=True, timeout=5, creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
            self.temp = float((r.stdout or "").split()[0])
        except Exception:
            self.temp = None
        return self.temp

    def pace(self, busy_s: float) -> None:
        if self.gov is not None:
            self.gov.pace(busy_s, max_duty=self.duty)
        elif self.duty < 1.0 and busy_s > 0:
            time.sleep(min(5.0, busy_s * (1.0 - self.duty) / self.duty))
        now = time.time()
        if now - self._last < 3.0:
            return
        self._last = now
        t = self.read_temp()
        if t is None or t < self.max_temp:
            return
        t0 = time.time()
        while time.time() - t0 < 300:
            time.sleep(2.0)
            t = self.read_temp()
            if t is None or t <= self.max_temp - 8:
                break
        self.cooled_s += time.time() - t0


class FastT3:
    """Batched, CUDA-graphed T3 sampling over the model's own weights (nothing is copied)."""

    def __init__(self, model, dtype=None):
        self.model = model
        self.t3 = model.t3
        self.hp = self.t3.hp
        self.tfmr = self.t3.tfmr
        self.device = self.t3.device
        self.dtype = dtype or next(self.tfmr.parameters()).dtype
        if self.dtype != next(self.tfmr.parameters()).dtype:
            self.tfmr.to(self.dtype)
        cfg = self.t3.cfg
        self.layers = list(self.tfmr.layers)
        self.n_heads = cfg.num_attention_heads
        self.n_kv = cfg.num_key_value_heads
        self.head_dim = getattr(cfg, "head_dim", None) or cfg.hidden_size // cfg.num_attention_heads
        self.vocab = self.hp.speech_tokens_dict_size
        self.stop = self.hp.stop_speech_token
        self.start = self.hp.start_speech_token
        self._graphs: dict = {}
        self.guard = GpuGuard()
        self.stats = {"t3_s": 0.0, "captures": 0, "capture_s": 0.0, "steps": 0, "batches": 0}
        from transformers.models.llama.modeling_llama import apply_rotary_pos_emb
        self._rope = apply_rotary_pos_emb

    # ------------------------------------------------------------------ transformer

    def _forward(self, x, positions, kv, slot, mask):
        """x (R, T, C) at RoPE `positions` (R, T); writes keys/values at cache slots
        `slot` .. slot+T-1 (`slot` a 0-d long tensor); `mask` (R, 1, T, L) is True where a
        query may attend. Returns the final-norm hidden states (R, T, C)."""
        R, T, _ = x.shape
        cos, sin = self.tfmr.rotary_emb(x, positions)
        idx = slot + torch.arange(T, device=x.device)
        for li, layer in enumerate(self.layers):
            attn = layer.self_attn
            h = layer.input_layernorm(x)
            q = attn.q_proj(h).view(R, T, self.n_heads, self.head_dim).transpose(1, 2)
            k = attn.k_proj(h).view(R, T, self.n_kv, self.head_dim).transpose(1, 2)
            v = attn.v_proj(h).view(R, T, self.n_kv, self.head_dim).transpose(1, 2)
            q, k = self._rope(q, k, cos, sin)
            kc, vc = kv[li]
            kc.index_copy_(2, idx, k)
            vc.index_copy_(2, idx, v)
            if self.n_kv != self.n_heads:
                rep = self.n_heads // self.n_kv
                kk, vv = kc.repeat_interleave(rep, 1), vc.repeat_interleave(rep, 1)
            else:
                kk, vv = kc, vc
            o = F.scaled_dot_product_attention(q, kk, vv, attn_mask=mask)
            o = o.transpose(1, 2).reshape(R, T, self.n_heads * self.head_dim)
            x = x + attn.o_proj(o)
            x = x + layer.mlp(layer.post_attention_layernorm(x))
        return self.tfmr.norm(x)

    # ------------------------------------------------------------------ prefix

    def _prefix(self, cond, text, exaggeration):
        """[conditioning, text, start, start] embeddings for the (cond, uncond) pair of one
        sentence, as T3.prepare_input_embeds + T3.inference build them. (2, L, C) fp32."""
        from chatterbox.models.t3.modules.cond_enc import T3Cond
        t3 = self.t3
        c = T3Cond(speaker_emb=cond.speaker_emb, cond_prompt_speech_tokens=cond.cond_prompt_speech_tokens,
                   cond_prompt_speech_emb=cond.cond_prompt_speech_emb,
                   emotion_adv=exaggeration * torch.ones(1, 1, 1, device=self.device))
        cond_emb = t3.prepare_conditioning(c)  # (1, Lc, C)
        tokens = text.view(1, -1).to(self.device)
        te = t3.text_emb(tokens)
        te = torch.cat([te, torch.zeros_like(te)])  # CFG: the uncond row has no text
        te = te + t3.text_pos_emb(tokens)
        bos = torch.full((1, 1), self.start, dtype=torch.long, device=self.device)
        be = t3.speech_emb(bos) + t3.speech_pos_emb.get_fixed_embedding(0)
        return torch.cat([cond_emb.expand(2, -1, -1), te, be.expand(2, -1, -1), be.expand(2, -1, -1)], dim=1)

    # ------------------------------------------------------------------ graph

    def _graph(self, n, L):
        """Static buffers and the captured decode step for `n` sentences and cache length `L`
        (the prefix length is a buffer, `pbase`)."""
        # A captured graph a little bigger than needed is reused (padding rows are cheap,
        # capturing is not); only one is kept, their KV caches are big.
        for (gn, gl), g in self._graphs.items():
            if n <= gn and L <= gl <= L + 512:
                return g
        t_cap = time.time()
        self._graphs.clear()
        torch.cuda.empty_cache()
        key = (n, L)
        dev, R, C = self.device, 2 * n, self.t3.dim
        s = {
            "kv": [(torch.zeros(R, self.n_kv, L, self.head_dim, device=dev, dtype=self.dtype),
                    torch.zeros(R, self.n_kv, L, self.head_dim, device=dev, dtype=self.dtype)) for _ in self.layers],
            "mask": torch.zeros(R, 1, 1, L, dtype=torch.bool, device=dev),
            "logits": torch.zeros(R, self.vocab, device=dev),
            "seen": torch.zeros(n, self.vocab, dtype=torch.bool, device=dev),
            "done": torch.zeros(n, dtype=torch.bool, device=dev),
            "cap": torch.zeros(n, dtype=torch.long, device=dev),
            "plen": torch.zeros(R, dtype=torch.long, device=dev),
            "cfg": torch.zeros(n, 1, device=dev),
            "temp": torch.ones(n, 1, device=dev),
            "rep": torch.ones(n, 1, device=dev),
            "minp": torch.zeros(n, 1, device=dev),
            "noise": torch.ones(STEPS_PER_BLOCK, n, self.vocab, device=dev),
            "out": torch.full((n, L), self.stop, dtype=torch.long, device=dev),
            "step": torch.zeros((), dtype=torch.long, device=dev),
            "pbase": torch.zeros((), dtype=torch.long, device=dev),
        }

        def body():
            step = s["step"]
            lg = s["logits"].float()
            cond, uncond = lg[0::2], lg[1::2]
            x = cond + s["cfg"] * (cond - uncond)
            rp = s["rep"]
            x = torch.where(s["seen"], torch.where(x < 0, x * rp, x / rp), x)
            x = x / s["temp"]
            p = torch.softmax(x, dim=-1)
            x = x.masked_fill(p < p.max(dim=-1, keepdim=True).values * s["minp"], -math.inf)
            p = torch.softmax(x, dim=-1)
            noise = s["noise"].index_select(0, (step % STEPS_PER_BLOCK).view(1)).squeeze(0)
            tok = torch.argmax(p / noise, dim=-1)
            tok = torch.where(s["done"], torch.full_like(tok, self.stop), tok)
            s["out"].index_copy_(1, step.view(1), tok.view(n, 1))
            s["seen"].scatter_(1, tok.view(n, 1), True)
            s["done"].logical_or_(tok == self.stop).logical_or_(step + 1 >= s["cap"])
            # Forward the new token (the cond and uncond rows get the same one).
            e = self.t3.speech_emb(tok) + self.t3.speech_pos_emb.emb(step + 1).view(1, -1)
            e = e.repeat_interleave(2, dim=0).view(R, 1, C).to(self.dtype)
            slot = s["pbase"] + step
            s["mask"].index_fill_(3, slot.view(1), True)
            h = self._forward(e, (s["plen"] + step).view(R, 1), s["kv"], slot, s["mask"])
            s["logits"].copy_(self.t3.speech_head(h[:, -1].to(self.t3.speech_head.weight.dtype)).float())
            step.add_(1)

        # Warm up on a side stream, then capture.
        st = torch.cuda.Stream()
        st.wait_stream(torch.cuda.current_stream())
        with torch.cuda.stream(st), torch.inference_mode():
            for _ in range(2):
                s["step"].zero_()
                body()
        torch.cuda.current_stream().wait_stream(st)
        graph = torch.cuda.CUDAGraph()
        with torch.inference_mode(), torch.cuda.graph(graph):
            body()
        g = {"graph": graph, "s": s, "n": n, "L": L}
        self._graphs[key] = g
        torch.cuda.synchronize()
        self.stats["captures"] += 1
        self.stats["capture_s"] += time.time() - t_cap
        return g

    # ------------------------------------------------------------------ generate

    def _cache_bytes(self, n, L):
        """KV cache size of a batch of `n` sentences (2n rows) and length `L`."""
        elt = torch.finfo(self.dtype).bits // 8
        return len(self.layers) * 2 * (2 * n) * self.n_kv * L * self.head_dim * elt

    @staticmethod
    def _cache_len(prefix, cap):
        return _round_up(prefix + cap + STEPS_PER_BLOCK + 1, 256)

    def batches(self, items, max_batch=16, budget=None):
        """Groups items (indices) into batches whose KV cache fits in free GPU memory, short
        sentences together so a batch wastes little on padding."""
        if budget is None:
            torch.cuda.empty_cache()
            free, _ = torch.cuda.mem_get_info()
            held = sum(self._cache_bytes(gn, gl) for gn, gl in self._graphs)
            # Room for S3Gen and activations beside the cache; never the whole card.
            budget = max(256 << 20, int((free + held - (1536 << 20)) * 0.6))
        order = sorted(range(len(items)), key=lambda i: (len(items[i]["text_tokens"]), items[i].get("max_tokens") or 0))
        out, cur = [], []
        for i in order:
            trial = cur + [i]
            n = 1 << max(0, (len(trial) - 1).bit_length())
            P = max(len(items[j]["text_tokens"]) for j in trial) + 40  # + conditioning and start tokens
            cap = max(int(items[j].get("max_tokens") or 1000) for j in trial)
            if cur and (len(trial) > max_batch or self._cache_bytes(n, self._cache_len(P, cap)) > budget):
                out.append(cur)
                cur = [i]
            else:
                cur = trial
        if cur:
            out.append(cur)
        return out

    @torch.inference_mode()
    def speech_tokens(self, cond, items, max_new_tokens=1000):
        """Speech tokens for each item {text_tokens (1-D with start/stop text tokens),
        exaggeration, cfg_weight, temperature, repetition_penalty, min_p, seed, max_tokens?}.
        Returns one 1-D long tensor per item (special tokens dropped)."""
        n_real = len(items)
        n = 1 << max(0, (n_real - 1).bit_length())  # 1, 2, 4, 8, 16: few graph shapes
        prefixes = [self._prefix(cond, it["text_tokens"], float(it["exaggeration"])) for it in items]
        lens = [p.shape[1] for p in prefixes]
        P = max(lens)
        caps = [int(min(max_new_tokens, it.get("max_tokens") or max_new_tokens)) for it in items]
        # Room for the longest sentence plus the rest of its last block of steps.
        L = self._cache_len(P, max(caps))
        t_t3 = time.time()
        g = self._graph(n, L)
        n, L = g["n"], g["L"]
        s, R = g["s"], 2 * n
        dev = self.device

        # Prefill: left-padded prefixes, each row attending to its own tokens only.
        C = self.t3.dim
        x = torch.zeros(R, P, C, device=dev)
        valid = torch.zeros(R, P, dtype=torch.bool, device=dev)
        plen = torch.zeros(R, dtype=torch.long, device=dev)
        for i in range(n):
            j = min(i, n_real - 1)  # padding rows copy the last item and are done from the start
            pre, ln = prefixes[j], lens[j]
            x[2 * i:2 * i + 2, P - ln:] = pre
            valid[2 * i:2 * i + 2, P - ln:] = True
            plen[2 * i:2 * i + 2] = ln
        pos = (torch.arange(P, device=dev).view(1, P) - (P - plen).view(R, 1)).clamp(min=0)
        causal = torch.ones(P, P, dtype=torch.bool, device=dev).tril()
        pmask = (causal.view(1, 1, P, P) & valid.view(R, 1, 1, P))
        # Pad queries attend to themselves only (their output is never used; avoids NaN rows).
        pmask = pmask | torch.eye(P, dtype=torch.bool, device=dev).view(1, 1, P, P)
        pmask = F.pad(pmask, (0, L - P), value=False)  # the cache slots after the prefix
        for kc, vc in s["kv"]:
            kc.zero_()
            vc.zero_()
        h = self._forward(x.to(self.dtype), pos, s["kv"], torch.zeros((), dtype=torch.long, device=dev), pmask)
        s["logits"].copy_(self.t3.speech_head(h[:, -1].to(self.t3.speech_head.weight.dtype)).float())
        s["mask"].zero_()
        s["mask"][:, 0, 0, :P] = valid
        s["plen"].copy_(plen)
        s["step"].zero_()
        s["pbase"].fill_(P)
        s["out"].fill_(self.stop)
        s["seen"].zero_()
        s["seen"][:, self.start] = True

        def col(key, default):
            vals = [float(items[min(i, n_real - 1)].get(key, default)) for i in range(n)]
            return torch.tensor(vals, device=dev).view(n, 1)

        s["cfg"].copy_(col("cfg_weight", 0.5))
        s["temp"].copy_(col("temperature", 0.8))
        s["rep"].copy_(col("repetition_penalty", 1.2))
        s["minp"].copy_(col("min_p", 0.05))
        s["cap"].copy_(torch.tensor([caps[min(i, n_real - 1)] for i in range(n)], device=dev))
        s["done"].copy_(torch.tensor([i >= n_real for i in range(n)], device=dev))
        gens = [torch.Generator(device=dev).manual_seed(int(it["seed"]) & 0xFFFF_FFFF_FFFF) for it in items]

        steps = 0
        limit = max(caps)
        while steps < limit:
            t0 = time.time()
            for i, gen in enumerate(gens):
                s["noise"][:, i].exponential_(generator=gen)
            for _ in range(STEPS_PER_BLOCK):
                g["graph"].replay()
            steps += STEPS_PER_BLOCK
            finished = bool(s["done"][:n_real].all())  # waits for the block
            self.guard.pace(time.time() - t0)
            if finished:
                break

        out = s["out"][:n_real].cpu()
        result = []
        for i in range(n_real):
            row = out[i, :min(steps, out.shape[1])]
            stop = (row == self.stop).nonzero()
            if len(stop):
                row = row[:int(stop[0])]
            result.append(row[row < 6561].to(dev))
        self.stats["t3_s"] += time.time() - t_t3
        self.stats["steps"] += steps
        self.stats["batches"] += 1
        return result


def load(model_dir, device="cuda"):
    """ChatterboxTTS.from_local, about 3x quicker: the modules are built on the GPU (their
    random initial weights are made there instead of on the CPU) and the weights are read
    straight onto it. The weights end up the same."""
    from pathlib import Path
    from safetensors.torch import load_file
    from chatterbox.tts import ChatterboxTTS, Conditionals
    from chatterbox.models.t3 import T3
    from chatterbox.models.s3gen import S3Gen
    from chatterbox.models.voice_encoder import VoiceEncoder
    from chatterbox.models.tokenizers import EnTokenizer
    d = Path(model_dir)
    with torch.device(device):
        ve, t3, s3 = VoiceEncoder(), T3(), S3Gen()
    ve.load_state_dict(load_file(d / "ve.safetensors", device=device))
    sd = load_file(d / "t3_cfg.safetensors", device=device)
    t3.load_state_dict(sd["model"][0] if "model" in sd else sd)
    s3.load_state_dict(load_file(d / "s3gen.safetensors", device=device), strict=False)
    for m in (ve, t3, s3):
        m.to(device).eval()  # buffers made from numpy at build time are still on the CPU
    conds = Conditionals.load(d / "conds.pt").to(device) if (d / "conds.pt").exists() else None
    return ChatterboxTTS(t3, s3, ve, EnTokenizer(str(d / "tokenizer.json")), device, conds=conds)


def supported(params) -> bool:
    """The sampling settings FastT3 reproduces (top_p filtering is not implemented)."""
    return float((params or {}).get("top_p", 1.0)) >= 1.0 and float((params or {}).get("cfg_weight", 0.5)) > 0.0


def speak(model, fast, reqs, vocode_group=8):
    """Speech for many sentences: [{text, params, seed, max_tokens}] -> float32 arrays in the
    same order. T3 in memory-sized batches, S3Gen in groups of `vocode_group`, resting
    between pieces of GPU work as `fast.guard` asks."""
    items = [{"text_tokens": text_tokens(model, r["text"]), **(r.get("params") or {}), "seed": int(r.get("seed", 0)),
              "max_tokens": r.get("max_tokens")} for r in reqs]
    out = [None] * len(items)
    for group in fast.batches(items):
        tokens = fast.speech_tokens(model.conds.t3, [items[i] for i in group])
        for k in range(0, len(group), vocode_group):
            idx = group[k:k + vocode_group]
            t1 = time.time()
            wavs = vocode_batch(model, tokens[k:k + vocode_group], [items[i]["seed"] for i in idx])
            fast.stats["vocode_s"] = fast.stats.get("vocode_s", 0.0) + time.time() - t1
            fast.guard.pace(time.time() - t1)
            for i, w in zip(idx, wavs):
                out[i] = w
    return out


def text_tokens(model, text):
    """Chatterbox's text normalisation and tokens, with the start/stop text tokens."""
    from chatterbox.tts import punc_norm
    t = model.tokenizer.text_to_tokens(punc_norm(text)).view(-1)
    hp = model.t3.hp
    return torch.cat([torch.tensor([hp.start_text_token]), t.cpu(), torch.tensor([hp.stop_text_token])])


@torch.inference_mode()
def vocode_batch(model, tokens, seeds, n_timesteps=10, fp16=True):
    """S3Gen for several sentences: the flow encoder per sentence (it is cheap, and padding
    would leak into its convolutions), the 10 CFM steps for all of them as one batch (the
    estimator is causal, so padding after a sentence never reaches it), HiFT and the watermark
    per sentence. Follows CausalMaskedDiffWithXvec.inference + CausalConditionalCFM.forward,
    with each sentence's noise drawn from its own seed so it does not depend on the batch.
    `fp16` runs the CFM estimator under fp16 autocast. Returns float32 arrays."""
    from chatterbox.models.s3gen.utils.mask import make_pad_mask
    s3, ref = model.s3gen, model.conds.gen
    flow, dev = s3.flow, s3.device
    B = len(tokens)
    embedding = F.normalize(torch.atleast_2d(ref["embedding"].to(dev, s3.dtype)), dim=1)
    embedding = flow.spk_embed_affine_layer(embedding)
    p_tok = torch.atleast_2d(ref["prompt_token"].to(dev))
    p_len = ref["prompt_token_len"].to(dev).view(-1)[:1]
    p_feat = torch.atleast_3d(ref["prompt_feat"].to(dev, s3.dtype))
    mel1 = p_feat.shape[1]

    hs = []
    for t in tokens:
        t = torch.atleast_2d(t.to(dev))
        full = torch.cat([p_tok, t], dim=1)
        full_len = p_len + t.shape[1]
        x = flow.input_embedding(full.long()) * (~make_pad_mask(full_len)).unsqueeze(-1).to(embedding)
        h, h_masks = flow.encoder(x, full_len)
        hs.append(flow.encoder_proj(h)[0])  # (M_i, 80)
    lens = [h.shape[0] for h in hs]
    M = max(lens)
    mu = torch.zeros(B, M, hs[0].shape[1], device=dev, dtype=hs[0].dtype)
    for i, h in enumerate(hs):
        mu[i, :lens[i]] = h
    mu = mu.transpose(1, 2).contiguous()
    conds = torch.zeros(B, flow.output_size, M, device=dev, dtype=mu.dtype)
    conds[:, :, :mel1] = p_feat.transpose(1, 2)
    mask = (~make_pad_mask(torch.tensor(lens, device=dev), M)).unsqueeze(1).to(mu)

    cfm = flow.decoder
    z = torch.zeros_like(mu)
    for i, seed in enumerate(seeds):
        g = torch.Generator(device=dev).manual_seed(int(seed) & 0xFFFF_FFFF_FFFF)
        z[i, :, :lens[i]] = torch.randn(mu.shape[1], lens[i], generator=g, device=dev, dtype=mu.dtype)
    t_span = torch.linspace(0, 1, n_timesteps + 1, device=dev, dtype=mu.dtype)
    if cfm.t_scheduler == "cosine":
        t_span = 1 - torch.cos(t_span * 0.5 * torch.pi)
    with torch.autocast("cuda", dtype=torch.float16, enabled=fp16):
        feat = cfm.solve_euler(z, t_span=t_span, mu=mu, mask=mask, spks=embedding.expand(B, -1), cond=conds)
    feat = feat.float()[:, :, mel1:].to(dtype=s3.dtype)

    out = []
    for i in range(B):
        torch.manual_seed(int(seeds[i]))  # HiFT's excitation noise
        wav, _ = s3.hift_inference(feat[i:i + 1, :, :lens[i] - mel1], None)
        wav[:, :len(s3.trim_fade)] *= s3.trim_fade
        w = wav.squeeze(0).detach().cpu().numpy()
        out.append(model.watermarker.apply_watermark(w, sample_rate=model.sr))
    return out


def vocode(model, tokens, seed):
    """S3Gen + watermark for one sentence's speech tokens, as ChatterboxTTS.generate does.
    The flow-matching noise is seeded per sentence. Returns a float32 numpy array."""
    torch.manual_seed(seed)
    torch.cuda.manual_seed_all(seed)
    with torch.inference_mode():
        wav, _ = model.s3gen.inference(speech_tokens=tokens, ref_dict=model.conds.gen)
        wav = wav.squeeze(0).detach().cpu().numpy()
    return model.watermarker.apply_watermark(wav, sample_rate=model.sr)
