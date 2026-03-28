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

import logging
from dataclasses import dataclass
from typing import Any, NamedTuple, Optional, Tuple

import haiku as hk
import jax
import jax.numpy as jnp

from grok import TransformerConfig, Transformer
from recsys_model import (
    HashConfig,
    RecsysBatch,
    RecsysEmbeddings,
    block_history_reduce,
    block_user_reduce,
)

logger = logging.getLogger(__name__)

EPS = 1e-12
INF = 1e12


class RetrievalOutput(NamedTuple):
    """Output of the retrieval model."""

    user_representation: jax.Array
    top_k_indices: jax.Array
    top_k_scores: jax.Array


@dataclass
class CandidateTower(hk.Module):
    """Candidate tower that projects post+author embeddings to a shared embedding space.

    This tower takes the concatenated embeddings of a post and its author,
    and projects them to a normalized representation suitable for similarity search.
    """

    emb_size: int
    name: Optional[str] = None

    def __call__(self, post_author_embedding: jax.Array) -> jax.Array:
        """Project post+author embeddings to normalized representation.

        Args:
            post_author_embedding: Concatenated post and author embeddings
                Shape: [B, C, num_hashes, D] or [B, num_hashes, D]

        Returns:
            Normalized candidate representation
                Shape: [B, C, D] or [B, D]
        """
        if len(post_author_embedding.shape) == 4:
            B, C, _, _ = post_author_embedding.shape
            post_author_embedding = jnp.reshape(post_author_embedding, (B, C, -1))
        else:
            B, _, _ = post_author_embedding.shape
            post_author_embedding = jnp.reshape(post_author_embedding, (B, -1))

        embed_init = hk.initializers.VarianceScaling(1.0, mode="fan_out")

        proj_1 = hk.get_parameter(
            "candidate_tower_projection_1",
            [post_author_embedding.shape[-1], self.emb_size * 2],
            dtype=jnp.float32,
            init=embed_init,
        )

        proj_2 = hk.get_parameter(
            "candidate_tower_projection_2",
            [self.emb_size * 2, self.emb_size],
            dtype=jnp.float32,
            init=embed_init,
        )

        hidden = jnp.dot(post_author_embedding.astype(proj_1.dtype), proj_1)
        hidden = jax.nn.silu(hidden)
        candidate_embeddings = jnp.dot(hidden.astype(proj_2.dtype), proj_2)

        candidate_norm_sq = jnp.sum(candidate_embeddings**2, axis=-1, keepdims=True)
        candidate_norm = jnp.sqrt(jnp.maximum(candidate_norm_sq, EPS))
        candidate_representation = candidate_embeddings / candidate_norm

        return candidate_representation.astype(post_author_embedding.dtype)


@dataclass
class PhoenixRetrievalModelConfig:
    """Configuration for the Phoenix Retrieval Model.

    This model uses the same transformer architecture as the Phoenix ranker
    for encoding user representations.
    """

    model: TransformerConfig
    emb_size: int
    history_seq_len: int = 128
    candidate_seq_len: int = 32

    name: Optional[str] = None
    fprop_dtype: Any = jnp.bfloat16

    hash_config: HashConfig = None  # type: ignore

    product_surface_vocab_size: int = 16

    _initialized: bool = False

    def __post_init__(self):
        if self.hash_config is None:
            self.hash_config = HashConfig()

    def initialize(self):
        self._initialized = True
        return self

    def make(self):
        if not self._initialized:
            logger.warning(f"PhoenixRetrievalModel {self.name} is not initialized. Initializing.")
            self.initialize()

        return PhoenixRetrievalModel(
            model=self.model.make(),
            config=self,
            fprop_dtype=self.fprop_dtype,
        )


@dataclass
class PhoenixRetrievalModel(hk.Module):
    """A two-tower retrieval model using the Phoenix transformer for user encoding.

    This model implements the two-tower architecture for efficient retrieval:
    - User Tower: Encodes user features + history using the Phoenix transformer
    - Candidate Tower: Projects candidate embeddings to a shared space

    The user and candidate representations are L2-normalized, enabling efficient
    approximate nearest neighbor (ANN) search using dot product similarity.
    """

    model: Transformer
    config: PhoenixRetrievalModelConfig
    fprop_dtype: Any = jnp.bfloat16
    name: Optional[str] = None

    def _get_action_embeddings(
        self,
        actions: jax.Array,
    ) -> jax.Array:
        """Convert multi-hot action vectors to embeddings."""
        config = self.config
        _, _, num_actions = actions.shape
        D = config.emb_size

        embed_init = hk.initializers.VarianceScaling(1.0, mode="fan_out")
        action_projection = hk.get_parameter(
            "action_projection",
            [num_actions, D],
            dtype=jnp.float32,
            init=embed_init,
        )

        actions_signed = (2 * actions - 1).astype(jnp.float32)
        action_emb = jnp.dot(actions_signed.astype(action_projection.dtype), action_projection)

        valid_mask = jnp.any(actions, axis=-1, keepdims=True)
        action_emb = action_emb * valid_mask

        return action_emb.astype(self.fprop_dtype)

    def _single_hot_to_embeddings(
        self,
        input: jax.Array,
        vocab_size: int,
        emb_size: int,
        name: str,
    ) -> jax.Array:
        """Convert single-hot indices to embeddings via lookup table."""
        embed_init = hk.initializers.VarianceScaling(1.0, mode="fan_out")
        embedding_table = hk.get_parameter(
            name,
            [vocab_size, emb_size],
            dtype=jnp.float32,
            init=embed_init,
        )

        input_one_hot = jax.nn.one_hot(input, vocab_size)
        output = jnp.dot(input_one_hot, embedding_table)
        return output.astype(self.fprop_dtype)

    def build_user_representation(
        self,
        batch: RecsysBatch,
        recsys_embeddings: RecsysEmbeddings,
    ) -> Tuple[jax.Array, jax.Array]:
        """Build user representation from user features and history.

        Uses the Phoenix transformer to encode user + history embeddings
        into a single user representation vector.

        Args:
            batch: RecsysBatch containing hashes, actions, product surfaces
            recsys_embeddings: RecsysEmbeddings containing pre-looked-up embeddings

        Returns:
            user_representation: L2-normalized user embedding [B, D]
            user_norm: Pre-normalization L2 norm [B, 1]
        """
        config = self.config
        hash_config = config.hash_config

        history_product_surface_embeddings = self._single_hot_to_embeddings(
            batch.history_product_surface,  # type: ignore
            config.product_surface_vocab_size,
            config.emb_size,
            "product_surface_embedding_table",
        )

        history_actions_embeddings = self._get_action_embeddings(batch.history_actions)  # type: ignore

        user_embeddings, user_padding_mask = block_user_reduce(
            batch.user_hashes,  # type: ignore
            recsys_embeddings.user_embeddings,  # type: ignore
            hash_config.num_user_hashes,
            config.emb_size,
            1.0,
        )

        history_embeddings, history_padding_mask = block_history_reduce(
            batch.history_post_hashes,  # type: ignore
            recsys_embeddings.history_post_embeddings,  # type: ignore
            recsys_embeddings.history_author_embeddings,  # type: ignore
            history_product_surface_embeddings,
            history_actions_embeddings,
            hash_config.num_item_hashes,
            hash_config.num_author_hashes,
            1.0,
        )

        embeddings = jnp.concatenate([user_embeddings, history_embeddings], axis=1)
        padding_mask = jnp.concatenate([user_padding_mask, history_padding_mask], axis=1)

        model_output = self.model(
            embeddings.astype(self.fprop_dtype),
            padding_mask,
            candidate_start_offset=None,
        )

        user_outputs = model_output.embeddings

        mask_float = padding_mask.astype(jnp.float32)[:, :, None]  # [B, T, 1]
        user_embeddings_masked = user_outputs * mask_float
        user_embedding_sum = jnp.sum(user_embeddings_masked, axis=1)  # [B, D]
        mask_sum = jnp.sum(mask_float, axis=1)  # [B, 1]
        user_representation = user_embedding_sum / jnp.maximum(mask_sum, 1.0)

        user_norm_sq = jnp.sum(user_representation**2, axis=-1, keepdims=True)
        user_norm = jnp.sqrt(jnp.maximum(user_norm_sq, EPS))
        user_representation = user_representation / user_norm

        return user_representation, user_norm

    def build_candidate_representation(
        self,
        batch: RecsysBatch,
        recsys_embeddings: RecsysEmbeddings,
    ) -> Tuple[jax.Array, jax.Array]:
        """Build candidate (item) representations.

        Projects post + author embeddings to a shared embedding space
        using the candidate tower MLP.

        Args:
            batch: RecsysBatch containing candidate hashes
            recsys_embeddings: RecsysEmbeddings containing pre-looked-up embeddings

        Returns:
            candidate_representation: L2-normalized candidate embeddings [B, C, D]
            candidate_padding_mask: Valid candidate mask [B, C]
        """
        config = self.config

        candidate_post_embeddings = recsys_embeddings.candidate_post_embeddings
        candidate_author_embeddings = recsys_embeddings.candidate_author_embeddings

        post_author_embedding = jnp.concatenate(
            [candidate_post_embeddings, candidate_author_embeddings], axis=2
        )

        candidate_tower = CandidateTower(
            emb_size=config.emb_size,
        )
        candidate_representation = candidate_tower(post_author_embedding)

        candidate_padding_mask = (batch.candidate_post_hashes[:, :, 0] != 0).astype(jnp.bool_)  # type: ignore

        return candidate_representation, candidate_padding_mask

    def __call__(
        self,
        batch: RecsysBatch,
        recsys_embeddings: RecsysEmbeddings,
        corpus_embeddings: jax.Array,
        top_k: int,
        corpus_mask: Optional[jax.Array] = None,
    ) -> RetrievalOutput:
        """Retrieve top-k candidates from corpus for each user.

        Args:
            batch: RecsysBatch containing hashes, actions, product surfaces
            recsys_embeddings: RecsysEmbeddings containing pre-looked-up embeddings
            corpus_embeddings: [N, D] normalized corpus candidate embeddings
            top_k: Number of candidates to retrieve
            corpus_mask: [N] optional mask for valid corpus entries

        Returns:
            RetrievalOutput containing user representation and top-k results
        """
        user_representation, _ = self.build_user_representation(batch, recsys_embeddings)

        top_k_indices, top_k_scores = self._retrieve_top_k(
            user_representation, corpus_embeddings, top_k, corpus_mask
        )

        return RetrievalOutput(
            user_representation=user_representation,
            top_k_indices=top_k_indices,
            top_k_scores=top_k_scores,
        )

    def _retrieve_top_k(
        self,
        user_representation: jax.Array,
        corpus_embeddings: jax.Array,
        top_k: int,
        corpus_mask: Optional[jax.Array] = None,
    ) -> Tuple[jax.Array, jax.Array]:
        """Retrieve top-k candidates from a corpus for each user.

        Args:
            user_representation: [B, D] normalized user embeddings
            corpus_embeddings: [N, D] normalized corpus candidate embeddings
            top_k: Number of candidates to retrieve
            corpus_mask: [N] optional mask for valid corpus entries

        Returns:
            top_k_indices: [B, K] indices of top-k candidates
            top_k_scores: [B, K] similarity scores of top-k candidates
        """
        scores = jnp.matmul(user_representation, corpus_embeddings.T)

        if corpus_mask is not None:
            scores = jnp.where(corpus_mask[None, :], scores, -INF)

        top_k_scores, top_k_indices = jax.lax.top_k(scores, top_k)

        return top_k_indices, top_k_scores
