"""Greedy Nemotron ASR with a smaller vocabulary projection for MTP proposals.

Cache handling follows mlx-audio's nemotron_transcribe/speculative.py at
7a0986280fe295f4cf40f64164bb23abed416a22.

Copyright (c) 2024 Prince Canuma

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
"""
# Target layer traversal adapted from mlx-lm's nemotron_h.py, Copyright © 2025 Apple Inc. (MIT).
import mlx.core as mx
from mlx import nn
from mlx_audio.lm.generate import StreamingDetokenizer, generation_stream
from mlx_audio.lm.models.base import create_attention_mask, create_ssm_mask
from mlx_audio.lm.models.cache import KVCache
from mlx_audio.stt.models.nemotron_transcribe.language_model import MambaCache
from mlx_audio.stt.models.nemotron_transcribe.nemotron_transcribe import resolve_prompt
from mlx_audio.stt.utils import wired_limit


def prepare_draft_head(model):
    head = model.llm.lm_head
    if model.llm.mtp is None or type(head) is not nn.Linear or "bias" in head:
        return None
    if head.weight.dtype != mx.bfloat16 or head.weight.shape[0] < 16 or head.weight.shape[1] % 64:
        return None
    draft = nn.QuantizedLinear.from_linear(head, group_size=64, bits=4)
    mx.eval(draft.parameters())
    return draft


def propose(hidden, draft, head):
    scores = draft(hidden)[0]
    # Ascending vocabulary order preserves argmax's first-index tie break.
    ids = mx.sort(mx.argpartition(-scores, kth=15)[:16])
    return ids[mx.argmax(hidden @ head.weight[ids].T)]


def target_hidden(backbone, hidden, cache, checkpoint=None):
    attention_mask = create_attention_mask(hidden, cache[backbone.fa_idx])
    recurrent_mask = create_ssm_mask(hidden, cache[backbone.ssm_idx])
    states = iter(cache)
    for index, layer in enumerate(backbone.layers, 1):
        state = next(states) if layer.block_type in ("M", "*") else None
        mask = attention_mask if layer.block_type == "*" else recurrent_mask
        hidden = layer(hidden, mask=mask, cache=state)
        if index % 12 == 0:
            if checkpoint is None:
                mx.async_eval(hidden)
            else:
                mx.eval(hidden)
                checkpoint()
    return backbone.norm_f(hidden)


def draft_tail(mtp, embeddings, hidden, cache):
    """Update the draft attention cache for every position, then score only the tail."""
    attention, experts = mtp.layers
    fused = attention.eh_proj(mx.concatenate(
        [attention.enorm(embeddings), attention.hnorm(hidden)], axis=-1
    ))
    attended = attention(fused, mask=create_attention_mask(fused, cache[0]), cache=cache[0])
    return experts.final_layernorm(experts(attended[:, -1:]))


PREVIEW_TOKENS = 64
PREFILL_CHUNK = 512


def verified_preview(lm, embeddings, hint):
    cache, draft_cache = lm.make_cache(), [KVCache()]
    for layer in cache:
        if isinstance(layer, MambaCache):
            layer.recording = True
    inputs = mx.concatenate([embeddings[None], lm.backbone.embeddings(mx.array([hint]))], axis=1)
    hidden = lm.backbone(input_embeddings=inputs, cache=cache)
    predictions = mx.argmax(lm.lm_head(hidden[:, -(len(hint) + 1):])[0], axis=-1).tolist()
    accepted = next((i for i, t in enumerate(hint) if t != predictions[i]), len(hint))
    keep = len(embeddings) + accepted
    lm.finish_verification(cache, keep, len(hint) - accepted)
    current = predictions[accepted]
    shifted = mx.concatenate([inputs[:, 1:keep], lm.backbone.embeddings(mx.array([[current]]))], axis=1)
    tail = draft_tail(lm.mtp, shifted, hidden[:, :keep], draft_cache)
    mx.eval(tail, [layer.state for layer in cache], draft_cache[0].state)
    return (cache, draft_cache, tail, current), hint[:accepted]


def prefill(lm, embeddings, checkpoint):
    cache, draft_cache = lm.make_cache(), [KVCache()]
    for start in range(0, len(embeddings), PREFILL_CHUNK):
        end = min(start + PREFILL_CHUNK, len(embeddings))
        inputs = embeddings[None, start:end]
        if checkpoint is None:
            hidden = lm.backbone(input_embeddings=inputs, cache=cache)
        else:
            checkpoint()
            hidden = target_hidden(lm.backbone, inputs, cache, checkpoint)
        shifted = embeddings[None, start + 1:end + 1]
        if end == len(embeddings):
            current = mx.argmax(lm.lm_head(hidden[:, -1]), axis=-1).item()
            shifted = mx.concatenate([shifted, lm.backbone.embeddings(mx.array([[current]]))], axis=1)
        tail = draft_tail(lm.mtp, shifted, hidden, draft_cache)
        mx.eval(tail, [layer.state for layer in cache], draft_cache[0].state)
    return cache, draft_cache, tail, current


def verified_tokens(model, embeddings, draft, depth, max_tokens, hint=(), checkpoint=None):
    if max_tokens == 0:
        return
    lm = model.llm
    eos = model.config.eos_token_id
    k = min(len(hint), PREVIEW_TOKENS, max_tokens - 1)
    hint = list(hint[:k])
    state = None
    with mx.stream(generation_stream):
        if hint and len(embeddings) + k <= PREFILL_CHUNK:
            state, hint = verified_preview(lm, embeddings, hint)
        if state is None:
            hint = []
            state = prefill(lm, embeddings, checkpoint)
    cache, draft_cache, draft_hidden, current = state
    del state
    yield from hint
    yield current
    remaining = max_tokens - len(hint) - 1
    while remaining and current not in eos:
        with mx.stream(generation_stream):
            if checkpoint is not None:
                checkpoint()
            offset = draft_cache[0].offset
            count = min(depth, remaining - 1)
            draft_tokens = []
            for index in range(count):
                token = propose(draft_hidden[:, -1], draft, lm.lm_head)
                draft_tokens.append(token)
                if index + 1 < count:
                    embedding = lm.backbone.embeddings(token.reshape(1, 1))
                    draft_hidden = lm.mtp(embedding, draft_hidden[:, -1:], draft_cache)
            proposals = mx.stack(draft_tokens).tolist() if draft_tokens else []
            # Keep the original verification batch shape when a draft hits EOS.
            for i, token in enumerate(proposals):
                if token in eos:
                    proposals = proposals[:i + 1]
                    break

            for layer in cache:
                if isinstance(layer, MambaCache):
                    layer.recording = True
            inputs = lm.backbone.embeddings(mx.array([[current, *proposals]]))
            hidden = target_hidden(lm.backbone, inputs, cache, checkpoint)
            predictions = mx.argmax(lm.lm_head(hidden)[0], axis=-1).tolist()
            accepted = 0
            for proposal, prediction in zip(proposals, predictions):
                if proposal != prediction:
                    break
                accepted += 1
            output = predictions[:accepted + 1]
            lm.finish_verification(cache, len(output), len(proposals) - accepted)
            draft_cache[0].trim(draft_cache[0].offset - offset)
            draft_hidden = draft_tail(lm.mtp, lm.backbone.embeddings(mx.array([output])),
                                      hidden[:, :len(output)], draft_cache)
            mx.eval(draft_hidden, [layer.state for layer in cache], draft_cache[0].state)
        for current in output:
            yield current
            if current in eos:
                return
        remaining -= len(output)


def audio_tokens(model, audio, draft, depth, max_tokens, hint=(), checkpoint=None):
    prompt = resolve_prompt("asr", None, None, None, model.config.audio_locator_tag)
    with wired_limit(model):
        _, embeddings = model._prepare_inputs(audio, prompt, None, False)
        yield from verified_tokens(model, embeddings, draft, depth, max_tokens, hint, checkpoint)


def transcribe(model, audio, draft, depth, max_tokens, hint=()):
    decoder = StreamingDetokenizer(model._tokenizer, skip_special_tokens=False)
    for token in audio_tokens(model, audio, draft, depth, max_tokens, hint):
        if token in model.config.eos_token_id:
            break
        decoder.add_token(token)
    decoder.finalize()
    return decoder.text.strip()
