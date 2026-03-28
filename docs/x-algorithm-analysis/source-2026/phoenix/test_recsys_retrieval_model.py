# Copyright 2026 X.AI Corp.
#
# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at
#
#     http://www.apache.org/licenses/LICENSE-2.0
#
# Unless required by applicable law or agreed to in writing, software
# distributed under the License is distributed on an "AS IS" BASIS,
# WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
# See the License for the specific language governing permissions and
# limitations under the License.

"""Tests for the Phoenix Retrieval Model."""

import unittest

import haiku as hk
import jax
import jax.numpy as jnp
import numpy as np

from grok import TransformerConfig
from recsys_model import HashConfig
from recsys_retrieval_model import (
    CandidateTower,
    PhoenixRetrievalModelConfig,
)
from runners import (
    RecsysRetrievalInferenceRunner,
    RetrievalModelRunner,
    create_example_batch,
    create_example_corpus,
)


class TestCandidateTower(unittest.TestCase):
    """Tests for the CandidateTower module."""

    def test_candidate_tower_output_shape(self):
        """Test that candidate tower produces correct output shape."""
        emb_size = 64
        batch_size = 4
        num_candidates = 8
        num_hashes = 4

        def forward(x):
            tower = CandidateTower(emb_size=emb_size)
            return tower(x)

        forward_fn = hk.without_apply_rng(hk.transform(forward))

        rng = jax.random.PRNGKey(0)
        x = jax.random.normal(rng, (batch_size, num_candidates, num_hashes, emb_size))

        params = forward_fn.init(rng, x)
        output = forward_fn.apply(params, x)

        self.assertEqual(output.shape, (batch_size, num_candidates, emb_size))

    def test_candidate_tower_normalized(self):
        """Test that candidate tower output is L2 normalized."""
        emb_size = 64
        batch_size = 4
        num_candidates = 8
        num_hashes = 4

        def forward(x):
            tower = CandidateTower(emb_size=emb_size)
            return tower(x)

        forward_fn = hk.without_apply_rng(hk.transform(forward))

        rng = jax.random.PRNGKey(0)
        x = jax.random.normal(rng, (batch_size, num_candidates, num_hashes, emb_size))

        params = forward_fn.init(rng, x)
        output = forward_fn.apply(params, x)

        norms = jnp.sqrt(jnp.sum(output**2, axis=-1))
        np.testing.assert_array_almost_equal(norms, jnp.ones_like(norms), decimal=5)

    def test_candidate_tower_mean_pooling(self):
        """Test candidate tower with mean pooling (no linear projection)."""
        emb_size = 64
        batch_size = 4
        num_candidates = 8
        num_hashes = 4

        def forward(x):
            tower = CandidateTower(emb_size=emb_size)
            return tower(x)

        forward_fn = hk.without_apply_rng(hk.transform(forward))

        rng = jax.random.PRNGKey(0)
        x = jax.random.normal(rng, (batch_size, num_candidates, num_hashes, emb_size))

        params = forward_fn.init(rng, x)
        output = forward_fn.apply(params, x)

        self.assertEqual(output.shape, (batch_size, num_candidates, emb_size))

        norms = jnp.sqrt(jnp.sum(output**2, axis=-1))
        np.testing.assert_array_almost_equal(norms, jnp.ones_like(norms), decimal=5)


class TestPhoenixRetrievalModel(unittest.TestCase):
    """Tests for the full Phoenix Retrieval Model."""

    def setUp(self):
        """Set up test fixtures."""
        self.emb_size = 64
        self.history_seq_len = 16
        self.candidate_seq_len = 8
        self.batch_size = 2
        self.num_actions = 19
        self.corpus_size = 100
        self.top_k = 10

        self.hash_config = HashConfig(
            num_user_hashes=2,
            num_item_hashes=2,
            num_author_hashes=2,
        )

        self.config = PhoenixRetrievalModelConfig(
            emb_size=self.emb_size,
            history_seq_len=self.history_seq_len,
            candidate_seq_len=self.candidate_seq_len,
            hash_config=self.hash_config,
            product_surface_vocab_size=16,
            model=TransformerConfig(
                emb_size=self.emb_size,
                widening_factor=2,
                key_size=32,
                num_q_heads=2,
                num_kv_heads=2,
                num_layers=1,
                attn_output_multiplier=0.125,
            ),
        )

    def _create_test_batch(self) -> tuple:
        """Create test batch and embeddings."""
        return create_example_batch(
            batch_size=self.batch_size,
            emb_size=self.emb_size,
            history_len=self.history_seq_len,
            num_candidates=self.candidate_seq_len,
            num_actions=self.num_actions,
            num_user_hashes=self.hash_config.num_user_hashes,
            num_item_hashes=self.hash_config.num_item_hashes,
            num_author_hashes=self.hash_config.num_author_hashes,
            product_surface_vocab_size=16,
        )

    def _create_test_corpus(self):
        """Create test corpus embeddings."""
        return create_example_corpus(self.corpus_size, self.emb_size)

    def test_model_forward(self):
        """Test model forward pass produces correct output shapes."""

        def forward(batch, embeddings, corpus_embeddings, top_k):
            model = self.config.make()
            return model(batch, embeddings, corpus_embeddings, top_k)

        forward_fn = hk.without_apply_rng(hk.transform(forward))

        batch, embeddings = self._create_test_batch()
        corpus_embeddings, _ = self._create_test_corpus()

        rng = jax.random.PRNGKey(0)
        params = forward_fn.init(rng, batch, embeddings, corpus_embeddings, self.top_k)
        output = forward_fn.apply(params, batch, embeddings, corpus_embeddings, self.top_k)

        self.assertEqual(output.user_representation.shape, (self.batch_size, self.emb_size))
        self.assertEqual(output.top_k_indices.shape, (self.batch_size, self.top_k))
        self.assertEqual(output.top_k_scores.shape, (self.batch_size, self.top_k))

    def test_user_representation_normalized(self):
        """Test that user representations are L2 normalized."""

        def forward(batch, embeddings, corpus_embeddings, top_k):
            model = self.config.make()
            return model(batch, embeddings, corpus_embeddings, top_k)

        forward_fn = hk.without_apply_rng(hk.transform(forward))

        batch, embeddings = self._create_test_batch()
        corpus_embeddings, _ = self._create_test_corpus()

        rng = jax.random.PRNGKey(0)
        params = forward_fn.init(rng, batch, embeddings, corpus_embeddings, self.top_k)
        output = forward_fn.apply(params, batch, embeddings, corpus_embeddings, self.top_k)

        norms = jnp.sqrt(jnp.sum(output.user_representation**2, axis=-1))
        np.testing.assert_array_almost_equal(norms, jnp.ones(self.batch_size), decimal=5)

    def test_candidate_representation_normalized(self):
        """Test that candidate representations from build_candidate_representation are L2 normalized."""

        def forward(batch, embeddings):
            model = self.config.make()
            cand_rep, _ = model.build_candidate_representation(batch, embeddings)
            return cand_rep

        forward_fn = hk.without_apply_rng(hk.transform(forward))

        batch, embeddings = self._create_test_batch()

        rng = jax.random.PRNGKey(0)
        params = forward_fn.init(rng, batch, embeddings)
        cand_rep = forward_fn.apply(params, batch, embeddings)

        norms = jnp.sqrt(jnp.sum(cand_rep**2, axis=-1))
        np.testing.assert_array_almost_equal(
            norms, jnp.ones((self.batch_size, self.candidate_seq_len)), decimal=5
        )

    def test_retrieve_top_k(self):
        """Test top-k retrieval through __call__."""

        def forward(batch, embeddings, corpus_embeddings, top_k):
            model = self.config.make()
            return model(batch, embeddings, corpus_embeddings, top_k)

        forward_fn = hk.without_apply_rng(hk.transform(forward))

        batch, embeddings = self._create_test_batch()
        corpus_embeddings, _ = self._create_test_corpus()

        rng = jax.random.PRNGKey(0)
        params = forward_fn.init(rng, batch, embeddings, corpus_embeddings, self.top_k)
        output = forward_fn.apply(params, batch, embeddings, corpus_embeddings, self.top_k)

        self.assertEqual(output.top_k_indices.shape, (self.batch_size, self.top_k))
        self.assertEqual(output.top_k_scores.shape, (self.batch_size, self.top_k))

        self.assertTrue(jnp.all(output.top_k_indices >= 0))
        self.assertTrue(jnp.all(output.top_k_indices < self.corpus_size))

        for b in range(self.batch_size):
            scores = np.array(output.top_k_scores[b])
            self.assertTrue(np.all(scores[:-1] >= scores[1:]))


class TestRetrievalInferenceRunner(unittest.TestCase):
    """Tests for the retrieval inference runner."""

    def setUp(self):
        """Set up test fixtures."""
        self.emb_size = 64
        self.history_seq_len = 16
        self.candidate_seq_len = 8
        self.batch_size = 2
        self.num_actions = 19

        self.hash_config = HashConfig(
            num_user_hashes=2,
            num_item_hashes=2,
            num_author_hashes=2,
        )

        self.config = PhoenixRetrievalModelConfig(
            emb_size=self.emb_size,
            history_seq_len=self.history_seq_len,
            candidate_seq_len=self.candidate_seq_len,
            hash_config=self.hash_config,
            product_surface_vocab_size=16,
            model=TransformerConfig(
                emb_size=self.emb_size,
                widening_factor=2,
                key_size=32,
                num_q_heads=2,
                num_kv_heads=2,
                num_layers=1,
                attn_output_multiplier=0.125,
            ),
        )

    def test_runner_initialization(self):
        """Test that runner initializes correctly."""
        runner = RecsysRetrievalInferenceRunner(
            runner=RetrievalModelRunner(
                model=self.config,
                bs_per_device=0.125,
            ),
            name="test_retrieval",
        )

        runner.initialize()

        self.assertIsNotNone(runner.params)

    def test_runner_encode_user(self):
        """Test user encoding through runner."""
        runner = RecsysRetrievalInferenceRunner(
            runner=RetrievalModelRunner(
                model=self.config,
                bs_per_device=0.125,
            ),
            name="test_retrieval",
        )
        runner.initialize()

        batch, embeddings = create_example_batch(
            batch_size=self.batch_size,
            emb_size=self.emb_size,
            history_len=self.history_seq_len,
            num_candidates=self.candidate_seq_len,
            num_actions=self.num_actions,
            num_user_hashes=self.hash_config.num_user_hashes,
            num_item_hashes=self.hash_config.num_item_hashes,
            num_author_hashes=self.hash_config.num_author_hashes,
        )

        user_rep = runner.encode_user(batch, embeddings)

        self.assertEqual(user_rep.shape, (self.batch_size, self.emb_size))

    def test_runner_retrieve(self):
        """Test retrieval through runner."""
        runner = RecsysRetrievalInferenceRunner(
            runner=RetrievalModelRunner(
                model=self.config,
                bs_per_device=0.125,
            ),
            name="test_retrieval",
        )
        runner.initialize()

        batch, embeddings = create_example_batch(
            batch_size=self.batch_size,
            emb_size=self.emb_size,
            history_len=self.history_seq_len,
            num_candidates=self.candidate_seq_len,
            num_actions=self.num_actions,
            num_user_hashes=self.hash_config.num_user_hashes,
            num_item_hashes=self.hash_config.num_item_hashes,
            num_author_hashes=self.hash_config.num_author_hashes,
        )

        corpus_size = 100
        corpus_embeddings, corpus_post_ids = create_example_corpus(corpus_size, self.emb_size)
        runner.set_corpus(corpus_embeddings, corpus_post_ids)

        top_k = 10
        output = runner.retrieve(batch, embeddings, top_k=top_k)

        self.assertEqual(output.user_representation.shape, (self.batch_size, self.emb_size))
        self.assertEqual(output.top_k_indices.shape, (self.batch_size, top_k))
        self.assertEqual(output.top_k_scores.shape, (self.batch_size, top_k))


if __name__ == "__main__":
    unittest.main()
