import importlib.util
from pathlib import Path
import sys
import unittest
from unittest.mock import patch
from types import SimpleNamespace

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))


@unittest.skipUnless(importlib.util.find_spec("mlx_audio"), "requires the MLX helper environment")
class DraftTests(unittest.TestCase):
    def test_proposals_rescore_original_weights_and_break_ties_by_token_id(self):
        import mlx.core as mx
        from nemo_mtp import propose

        scores = mx.arange(32, dtype=mx.float32)[None]
        # The shortlist ranks 31 first. Original weights tie 20 and 21 in first place.
        weight = mx.array([int(i in (20, 21)) for i in range(32)])[:, None]
        proposal = propose(mx.ones((1, 1)), lambda _: scores, SimpleNamespace(weight=weight))
        self.assertEqual(proposal.item(), 20)

    def test_draft_tail_preserves_attention_history_and_the_last_output(self):
        import mlx.core as mx
        from mlx_audio.lm.models.cache import KVCache
        from mlx_audio.stt.models.nemotron_transcribe.language_model import MultiTokenPrediction
        from nemo_mtp import draft_tail

        width = 32
        args = SimpleNamespace(hidden_size=width, num_attention_heads=2, num_key_value_heads=1,
                               head_dim=16, attention_bias=False, layer_norm_epsilon=1e-5,
                               num_experts_per_tok=1, n_routed_experts=2, n_shared_experts=None,
                               moe_intermediate_size=32, moe_latent_size=None, n_group=1,
                               topk_group=1, norm_topk_prob=False, routed_scaling_factor=1.0)
        mtp = MultiTokenPrediction(args)
        full_cache, tail_cache = [KVCache()], [KVCache()]
        for length in (7, 3, 1):
            inputs = mx.arange(length * width, dtype=mx.float32).reshape(1, length, width)
            hidden = mx.sin(inputs)
            full = mtp(inputs, hidden, full_cache)
            tail = draft_tail(mtp, inputs, hidden, tail_cache)
            self.assertEqual(tail.shape, (1, 1, width))
            self.assertTrue(mx.allclose(tail, full[:, -1:], atol=1e-5).item())
            self.assertEqual(tail_cache[0].offset, full_cache[0].offset)
            for actual, expected in zip(tail_cache[0].state, full_cache[0].state):
                self.assertTrue(mx.array_equal(actual, expected).item())

    def test_verifier_alone_emits_tokens_after_wrong_drafts_and_at_limit(self):
        import mlx.core as mx
        from nemo_mtp import verified_tokens

        class Backbone:
            layers = ()
            fa_idx = ssm_idx = 0

            def __init__(self):
                self.verification_batches = []

            def embeddings(self, ids):
                return ids[..., None]

            def __call__(self, *, input_embeddings, cache):
                return input_embeddings

            def norm_f(self, hidden):
                self.verification_batches.append(hidden.flatten().tolist())
                return hidden

        class LM:
            backbone = Backbone()

            def make_cache(self):
                return [SimpleNamespace(state=mx.array([0]))]

            def mtp(self, inputs, hidden, cache):
                cache[0].update_and_fetch(inputs[:, None], inputs[:, None])
                return inputs

            def lm_head(self, hidden):
                return ((hidden + 1) == mx.arange(8)).astype(mx.float32)

            def finish_verification(self, cache, keep, rejected):
                pass

        model = SimpleNamespace(llm=LM(), config=SimpleNamespace(eos_token_id=[4]))
        self.enterContext(patch("nemo_mtp.draft_tail", lambda draft, x, h, cache: draft(x, h, cache)[:, -1:]))
        with patch("nemo_mtp.propose", return_value=mx.array(7)):
            for limit, expected in [(0, []), (1, [1]), (3, [1, 2, 3]), (12, [1, 2, 3, 4])]:
                with self.subTest(limit=limit):
                    self.assertEqual(list(verified_tokens(model, mx.zeros((2, 1)), None, 8, limit)),
                                     expected)

        # An early draft EOS can be rejected. Later speculative positions must
        # stay out of the target batch even though the GPU computed them.
        model.llm.backbone.verification_batches.clear()
        with patch("nemo_mtp.propose", return_value=mx.array(4)):
            tokens = list(verified_tokens(model, mx.zeros((2, 1)), None, 8, 12))
        self.assertEqual(tokens, [1, 2, 3, 4])
        self.assertEqual(model.llm.backbone.verification_batches, [[1, 4], [2, 4], [3, 4]])
