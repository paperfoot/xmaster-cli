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


import functools
import logging
from abc import ABC, abstractmethod
from dataclasses import dataclass
from typing import Any, List, NamedTuple, Optional, Tuple

import haiku as hk
import jax
import jax.numpy as jnp
import numpy as np

from grok import TrainingState
from recsys_retrieval_model import PhoenixRetrievalModelConfig
from recsys_retrieval_model import RetrievalOutput as ModelRetrievalOutput

from recsys_model import (
    PhoenixModelConfig,
    RecsysBatch,
    RecsysEmbeddings,
    RecsysModelOutput,
)

rank_logger = logging.getLogger("rank")


def create_dummy_batch_from_config(
    hash_config: Any,
    history_len: int,
    num_candidates: int,
    num_actions: int,
    batch_size: int = 1,
) -> RecsysBatch:
    """Create a dummy batch for initialization.

    Args:
        hash_config: HashConfig with num_user_hashes, num_item_hashes, num_author_hashes
        history_len: History sequence length
        num_candidates: Number of candidates
        num_actions: Number of action types
        batch_size: Batch size

    Returns:
        RecsysBatch with zeros
    """
    return RecsysBatch(
        user_hashes=np.zeros((batch_size, hash_config.num_user_hashes), dtype=np.int32),
        history_post_hashes=np.zeros(
            (batch_size, history_len, hash_config.num_item_hashes), dtype=np.int32
        ),
        history_author_hashes=np.zeros(
            (batch_size, history_len, hash_config.num_author_hashes), dtype=np.int32
        ),
        history_actions=np.zeros((batch_size, history_len, num_actions), dtype=np.float32),
        history_product_surface=np.zeros((batch_size, history_len), dtype=np.int32),
        candidate_post_hashes=np.zeros(
            (batch_size, num_candidates, hash_config.num_item_hashes), dtype=np.int32
        ),
        candidate_author_hashes=np.zeros(
            (batch_size, num_candidates, hash_config.num_author_hashes), dtype=np.int32
        ),
        candidate_product_surface=np.zeros((batch_size, num_candidates), dtype=np.int32),
    )


def create_dummy_embeddings_from_config(
    hash_config: Any,
    emb_size: int,
    history_len: int,
    num_candidates: int,
    batch_size: int = 1,
) -> RecsysEmbeddings:
    """Create dummy embeddings for initialization.

    Args:
        hash_config: HashConfig with num_user_hashes, num_item_hashes, num_author_hashes
        emb_size: Embedding dimension
        history_len: History sequence length
        num_candidates: Number of candidates
        batch_size: Batch size

    Returns:
        RecsysEmbeddings with zeros
    """
    return RecsysEmbeddings(
        user_embeddings=np.zeros(
            (batch_size, hash_config.num_user_hashes, emb_size), dtype=np.float32
        ),
        history_post_embeddings=np.zeros(
            (batch_size, history_len, hash_config.num_item_hashes, emb_size), dtype=np.float32
        ),
        candidate_post_embeddings=np.zeros(
            (batch_size, num_candidates, hash_config.num_item_hashes, emb_size),
            dtype=np.float32,
        ),
        history_author_embeddings=np.zeros(
            (batch_size, history_len, hash_config.num_author_hashes, emb_size), dtype=np.float32
        ),
        candidate_author_embeddings=np.zeros(
            (batch_size, num_candidates, hash_config.num_author_hashes, emb_size),
            dtype=np.float32,
        ),
    )


@dataclass
class BaseModelRunner(ABC):
    """Base class for model runners with shared initialization logic."""

    bs_per_device: float = 2.0
    rng_seed: int = 42

    @property
    @abstractmethod
    def model(self) -> Any:
        """Return the model config."""
        pass

    @property
    def _model_name(self) -> str:
        """Return model name for logging."""
        return "model"

    @abstractmethod
    def make_forward_fn(self):
        """Create the forward function. Must be implemented by subclasses."""
        pass

    def initialize(self):
        """Initialize the model runner."""
        self.model.initialize()
        self.model.fprop_dtype = jnp.bfloat16
        num_local_gpus = len(jax.local_devices())

        self.batch_size = max(1, int(self.bs_per_device * num_local_gpus))

        rank_logger.info(f"Initializing {self._model_name}...")
        self.forward = self.make_forward_fn()


@dataclass
class BaseInferenceRunner(ABC):
    """Base class for inference runners with shared dummy data creation."""

    name: str

    @property
    @abstractmethod
    def runner(self) -> BaseModelRunner:
        """Return the underlying model runner."""
        pass

    def _get_num_actions(self) -> int:
        """Get number of actions. Override in subclasses if needed."""
        model_config = self.runner.model
        if hasattr(model_config, "num_actions"):
            return model_config.num_actions
        return 19

    def create_dummy_batch(self, batch_size: int = 1) -> RecsysBatch:
        """Create a dummy batch for initialization."""
        model_config = self.runner.model
        return create_dummy_batch_from_config(
            hash_config=model_config.hash_config,
            history_len=model_config.history_seq_len,
            num_candidates=model_config.candidate_seq_len,
            num_actions=self._get_num_actions(),
            batch_size=batch_size,
        )

    def create_dummy_embeddings(self, batch_size: int = 1) -> RecsysEmbeddings:
        """Create dummy embeddings for initialization."""
        model_config = self.runner.model
        return create_dummy_embeddings_from_config(
            hash_config=model_config.hash_config,
            emb_size=model_config.emb_size,
            history_len=model_config.history_seq_len,
            num_candidates=model_config.candidate_seq_len,
            batch_size=batch_size,
        )

    @abstractmethod
    def initialize(self):
        """Initialize the inference runner. Must be implemented by subclasses."""
        pass


ACTIONS: List[str] = [
    "favorite_score",
    "reply_score",
    "repost_score",
    "photo_expand_score",
    "click_score",
    "profile_click_score",
    "vqv_score",
    "share_score",
    "share_via_dm_score",
    "share_via_copy_link_score",
    "dwell_score",
    "quote_score",
    "quoted_click_score",
    "follow_author_score",
    "not_interested_score",
    "block_author_score",
    "mute_author_score",
    "report_score",
    "dwell_time",
]


class RankingOutput(NamedTuple):
    """Output from ranking candidates.

    Contains both the raw scores array and individual probability fields
    for each engagement type.
    """

    scores: jax.Array

    ranked_indices: jax.Array

    p_favorite_score: jax.Array
    p_reply_score: jax.Array
    p_repost_score: jax.Array
    p_photo_expand_score: jax.Array
    p_click_score: jax.Array
    p_profile_click_score: jax.Array
    p_vqv_score: jax.Array
    p_share_score: jax.Array
    p_share_via_dm_score: jax.Array
    p_share_via_copy_link_score: jax.Array
    p_dwell_score: jax.Array
    p_quote_score: jax.Array
    p_quoted_click_score: jax.Array
    p_follow_author_score: jax.Array
    p_not_interested_score: jax.Array
    p_block_author_score: jax.Array
    p_mute_author_score: jax.Array
    p_report_score: jax.Array
    p_dwell_time: jax.Array


@dataclass
class ModelRunner(BaseModelRunner):
    """Runner for the recommendation ranking model."""

    _model: PhoenixModelConfig = None  # type: ignore

    def __init__(self, model: PhoenixModelConfig, bs_per_device: float = 2.0, rng_seed: int = 42):
        self._model = model
        self.bs_per_device = bs_per_device
        self.rng_seed = rng_seed

    @property
    def model(self) -> PhoenixModelConfig:
        return self._model

    @property
    def _model_name(self) -> str:
        return "ranking model"

    def make_forward_fn(self):  # type: ignore
        def forward(batch: RecsysBatch, recsys_embeddings: RecsysEmbeddings):
            out = self.model.make()(batch, recsys_embeddings)
            return out

        return hk.transform(forward)

    def init(
        self, rng: jax.Array, data: RecsysBatch, embeddings: RecsysEmbeddings
    ) -> TrainingState:
        assert self.forward is not None
        rng, init_rng = jax.random.split(rng)
        params = self.forward.init(init_rng, data, embeddings)
        return TrainingState(params=params)

    def load_or_init(
        self,
        init_data: RecsysBatch,
        init_embeddings: RecsysEmbeddings,
    ):
        rng = jax.random.PRNGKey(self.rng_seed)
        state = self.init(rng, init_data, init_embeddings)
        return state


@dataclass
class RecsysInferenceRunner(BaseInferenceRunner):
    """Inference runner for the recommendation ranking model."""

    _runner: ModelRunner

    def __init__(self, runner: ModelRunner, name: str):
        self.name = name
        self._runner = runner

    @property
    def runner(self) -> ModelRunner:
        return self._runner

    def initialize(self):
        """Initialize the inference runner."""
        runner = self.runner

        dummy_batch = self.create_dummy_batch(batch_size=1)
        dummy_embeddings = self.create_dummy_embeddings(batch_size=1)

        runner.initialize()

        state = runner.load_or_init(dummy_batch, dummy_embeddings)
        self.params = state.params

        @functools.lru_cache
        def model():
            return runner.model.make()

        def hk_forward(
            batch: RecsysBatch, recsys_embeddings: RecsysEmbeddings
        ) -> RecsysModelOutput:
            return model()(batch, recsys_embeddings)

        def hk_rank_candidates(
            batch: RecsysBatch, recsys_embeddings: RecsysEmbeddings
        ) -> RankingOutput:
            """Rank candidates by their predicted engagement scores."""
            output = hk_forward(batch, recsys_embeddings)
            logits = output.logits

            probs = jax.nn.sigmoid(logits)

            primary_scores = probs[:, :, 0]

            ranked_indices = jnp.argsort(-primary_scores, axis=-1)

            return RankingOutput(
                scores=probs,
                ranked_indices=ranked_indices,
                p_favorite_score=probs[:, :, 0],
                p_reply_score=probs[:, :, 1],
                p_repost_score=probs[:, :, 2],
                p_photo_expand_score=probs[:, :, 3],
                p_click_score=probs[:, :, 4],
                p_profile_click_score=probs[:, :, 5],
                p_vqv_score=probs[:, :, 6],
                p_share_score=probs[:, :, 7],
                p_share_via_dm_score=probs[:, :, 8],
                p_share_via_copy_link_score=probs[:, :, 9],
                p_dwell_score=probs[:, :, 10],
                p_quote_score=probs[:, :, 11],
                p_quoted_click_score=probs[:, :, 12],
                p_follow_author_score=probs[:, :, 13],
                p_not_interested_score=probs[:, :, 14],
                p_block_author_score=probs[:, :, 15],
                p_mute_author_score=probs[:, :, 16],
                p_report_score=probs[:, :, 17],
                p_dwell_time=probs[:, :, 18],
            )

        rank_ = hk.without_apply_rng(hk.transform(hk_rank_candidates))
        self.rank_candidates = rank_.apply

    def rank(self, batch: RecsysBatch, recsys_embeddings: RecsysEmbeddings) -> RankingOutput:
        """Rank candidates for the given batch.

        Args:
            batch: RecsysBatch containing hashes, actions, product surfaces
            recsys_embeddings: RecsysEmbeddings containing pre-looked-up embeddings

        Returns:
            RankingOutput with scores and ranked indices
        """
        return self.rank_candidates(self.params, batch, recsys_embeddings)


def create_example_batch(
    batch_size: int,
    emb_size: int,
    history_len: int,
    num_candidates: int,
    num_actions: int,
    num_user_hashes: int = 2,
    num_item_hashes: int = 2,
    num_author_hashes: int = 2,
    product_surface_vocab_size: int = 16,
    num_user_embeddings: int = 100000,
    num_post_embeddings: int = 100000,
    num_author_embeddings: int = 100000,
) -> Tuple[RecsysBatch, RecsysEmbeddings]:
    """Create an example batch with random data for testing.

    This simulates a recommendation scenario where:
    - We have a user with some embedding
    - The user has interacted with some posts in their history
    - We want to rank a set of candidate posts

    Note on embedding table sizes:
        The num_*_embeddings parameters define the size of the embedding tables for each
        entity type. Hash values are generated in the range [1, num_*_embeddings) to ensure
        they can be used as valid indices into the corresponding embedding tables.
        Hash value 0 is reserved for padding/invalid entries.

    Returns:
        Tuple of (RecsysBatch, RecsysEmbeddings)
    """
    rng = np.random.default_rng(42)

    user_hashes = rng.integers(1, num_user_embeddings, size=(batch_size, num_user_hashes)).astype(
        np.int32
    )

    history_post_hashes = rng.integers(
        1, num_post_embeddings, size=(batch_size, history_len, num_item_hashes)
    ).astype(np.int32)

    for b in range(batch_size):
        valid_len = rng.integers(history_len // 2, history_len + 1)
        history_post_hashes[b, valid_len:, :] = 0

    history_author_hashes = rng.integers(
        1, num_author_embeddings, size=(batch_size, history_len, num_author_hashes)
    ).astype(np.int32)
    for b in range(batch_size):
        valid_len = rng.integers(history_len // 2, history_len + 1)
        history_author_hashes[b, valid_len:, :] = 0

    history_actions = (rng.random(size=(batch_size, history_len, num_actions)) > 0.7).astype(
        np.float32
    )

    history_product_surface = rng.integers(
        0, product_surface_vocab_size, size=(batch_size, history_len)
    ).astype(np.int32)

    candidate_post_hashes = rng.integers(
        1, num_post_embeddings, size=(batch_size, num_candidates, num_item_hashes)
    ).astype(np.int32)

    candidate_author_hashes = rng.integers(
        1, num_author_embeddings, size=(batch_size, num_candidates, num_author_hashes)
    ).astype(np.int32)

    candidate_product_surface = rng.integers(
        0, product_surface_vocab_size, size=(batch_size, num_candidates)
    ).astype(np.int32)

    batch = RecsysBatch(
        user_hashes=user_hashes,
        history_post_hashes=history_post_hashes,
        history_author_hashes=history_author_hashes,
        history_actions=history_actions,
        history_product_surface=history_product_surface,
        candidate_post_hashes=candidate_post_hashes,
        candidate_author_hashes=candidate_author_hashes,
        candidate_product_surface=candidate_product_surface,
    )

    embeddings = RecsysEmbeddings(
        user_embeddings=rng.normal(size=(batch_size, num_user_hashes, emb_size)).astype(np.float32),
        history_post_embeddings=rng.normal(
            size=(batch_size, history_len, num_item_hashes, emb_size)
        ).astype(np.float32),
        candidate_post_embeddings=rng.normal(
            size=(batch_size, num_candidates, num_item_hashes, emb_size)
        ).astype(np.float32),
        history_author_embeddings=rng.normal(
            size=(batch_size, history_len, num_author_hashes, emb_size)
        ).astype(np.float32),
        candidate_author_embeddings=rng.normal(
            size=(batch_size, num_candidates, num_author_hashes, emb_size)
        ).astype(np.float32),
    )

    return batch, embeddings


class RetrievalOutput(NamedTuple):
    """Output from retrieval inference.

    Contains user representations and retrieved candidates.
    """

    user_representation: jax.Array

    top_k_indices: jax.Array

    top_k_scores: jax.Array


@dataclass
class RetrievalModelRunner(BaseModelRunner):
    """Runner for the Phoenix retrieval model."""

    _model: PhoenixRetrievalModelConfig = None  # type: ignore

    def __init__(
        self,
        model: PhoenixRetrievalModelConfig,
        bs_per_device: float = 2.0,
        rng_seed: int = 42,
    ):
        self._model = model
        self.bs_per_device = bs_per_device
        self.rng_seed = rng_seed

    @property
    def model(self) -> PhoenixRetrievalModelConfig:
        return self._model

    @property
    def _model_name(self) -> str:
        return "retrieval model"

    def make_forward_fn(self):  # type: ignore
        def forward(
            batch: RecsysBatch,
            recsys_embeddings: RecsysEmbeddings,
            corpus_embeddings: jax.Array,
            top_k: int,
        ) -> ModelRetrievalOutput:
            model = self.model.make()
            out = model(batch, recsys_embeddings, corpus_embeddings, top_k)

            _ = model.build_candidate_representation(batch, recsys_embeddings)
            return out

        return hk.transform(forward)

    def init(
        self,
        rng: jax.Array,
        data: RecsysBatch,
        embeddings: RecsysEmbeddings,
        corpus_embeddings: jax.Array,
        top_k: int,
    ) -> TrainingState:
        assert self.forward is not None
        rng, init_rng = jax.random.split(rng)
        params = self.forward.init(init_rng, data, embeddings, corpus_embeddings, top_k)
        return TrainingState(params=params)

    def load_or_init(
        self,
        init_data: RecsysBatch,
        init_embeddings: RecsysEmbeddings,
        corpus_embeddings: jax.Array,
        top_k: int,
    ):
        rng = jax.random.PRNGKey(self.rng_seed)
        state = self.init(rng, init_data, init_embeddings, corpus_embeddings, top_k)
        return state


@dataclass
class RecsysRetrievalInferenceRunner(BaseInferenceRunner):
    """Inference runner for the Phoenix retrieval model.

    This runner provides methods for:
    1. Encoding users to get user representations
    2. Encoding candidates to get candidate embeddings
    3. Retrieving top-k candidates from a corpus
    """

    _runner: RetrievalModelRunner = None  # type: ignore

    corpus_embeddings: jax.Array | None = None
    corpus_post_ids: jax.Array | None = None

    def __init__(self, runner: RetrievalModelRunner, name: str):
        self.name = name
        self._runner = runner
        self.corpus_embeddings = None
        self.corpus_post_ids = None

    @property
    def runner(self) -> RetrievalModelRunner:
        return self._runner

    def initialize(self):
        """Initialize the retrieval inference runner."""
        runner = self.runner

        dummy_batch = self.create_dummy_batch(batch_size=1)
        dummy_embeddings = self.create_dummy_embeddings(batch_size=1)
        dummy_corpus = jnp.zeros((10, runner.model.emb_size), dtype=jnp.float32)
        dummy_top_k = 5

        runner.initialize()

        state = runner.load_or_init(dummy_batch, dummy_embeddings, dummy_corpus, dummy_top_k)
        self.params = state.params

        @functools.lru_cache
        def model():
            return runner.model.make()

        def hk_encode_user(batch: RecsysBatch, recsys_embeddings: RecsysEmbeddings) -> jax.Array:
            """Encode user to get user representation."""
            m = model()
            user_rep, _ = m.build_user_representation(batch, recsys_embeddings)
            return user_rep

        def hk_encode_candidates(
            batch: RecsysBatch, recsys_embeddings: RecsysEmbeddings
        ) -> jax.Array:
            """Encode candidates to get candidate representations."""
            m = model()
            cand_rep, _ = m.build_candidate_representation(batch, recsys_embeddings)
            return cand_rep

        def hk_retrieve(
            batch: RecsysBatch,
            recsys_embeddings: RecsysEmbeddings,
            corpus_embeddings: jax.Array,
            top_k: int,
        ) -> "RetrievalOutput":
            """Retrieve top-k candidates from corpus."""
            m = model()
            return m(batch, recsys_embeddings, corpus_embeddings, top_k)

        encode_user_ = hk.without_apply_rng(hk.transform(hk_encode_user))
        encode_candidates_ = hk.without_apply_rng(hk.transform(hk_encode_candidates))
        retrieve_ = hk.without_apply_rng(hk.transform(hk_retrieve))

        self.encode_user_fn = encode_user_.apply
        self.encode_candidates_fn = encode_candidates_.apply
        self.retrieve_fn = retrieve_.apply

    def encode_user(self, batch: RecsysBatch, recsys_embeddings: RecsysEmbeddings) -> jax.Array:
        """Encode users to get user representations.

        Args:
            batch: RecsysBatch containing user and history information
            recsys_embeddings: RecsysEmbeddings containing pre-looked-up embeddings

        Returns:
            User representations [B, D]
        """
        return self.encode_user_fn(self.params, batch, recsys_embeddings)

    def encode_candidates(
        self, batch: RecsysBatch, recsys_embeddings: RecsysEmbeddings
    ) -> jax.Array:
        """Encode candidates to get candidate representations.

        Args:
            batch: RecsysBatch containing candidate information
            recsys_embeddings: RecsysEmbeddings containing pre-looked-up embeddings

        Returns:
            Candidate representations [B, C, D]
        """
        return self.encode_candidates_fn(self.params, batch, recsys_embeddings)

    def set_corpus(
        self,
        corpus_embeddings: jax.Array,
        corpus_post_ids: jax.Array,
    ):
        """Set the corpus embeddings for retrieval.

        Args:
            corpus_embeddings: Pre-computed candidate embeddings [N, D]
            corpus_post_ids: Optional post IDs corresponding to embeddings [N]
        """
        self.corpus_embeddings = corpus_embeddings
        self.corpus_post_ids = corpus_post_ids

    def retrieve(
        self,
        batch: RecsysBatch,
        recsys_embeddings: RecsysEmbeddings,
        top_k: int = 100,
        corpus_embeddings: Optional[jax.Array] = None,
    ) -> RetrievalOutput:
        """Retrieve top-k candidates for users.

        Args:
            batch: RecsysBatch containing user and history information
            recsys_embeddings: RecsysEmbeddings containing pre-looked-up embeddings
            top_k: Number of candidates to retrieve per user
            corpus_embeddings: Optional corpus embeddings (uses set_corpus if not provided)

        Returns:
            RetrievalOutput with user representations and top-k candidates
        """
        if corpus_embeddings is None:
            corpus_embeddings = self.corpus_embeddings

        return self.retrieve_fn(self.params, batch, recsys_embeddings, corpus_embeddings, top_k)


def create_example_corpus(
    corpus_size: int,
    emb_size: int,
    seed: int = 123,
) -> Tuple[jax.Array, jax.Array]:
    """Create example corpus embeddings for testing retrieval.

    Args:
        corpus_size: Number of candidates in corpus
        emb_size: Embedding dimension
        seed: Random seed

    Returns:
        Tuple of (corpus_embeddings [N, D], corpus_post_ids [N])
    """
    rng = np.random.default_rng(seed)

    corpus_embeddings = rng.normal(size=(corpus_size, emb_size)).astype(np.float32)
    norms = np.linalg.norm(corpus_embeddings, axis=-1, keepdims=True)
    corpus_embeddings = corpus_embeddings / np.maximum(norms, 1e-12)

    corpus_post_ids = np.arange(corpus_size, dtype=np.int64)

    return jnp.array(corpus_embeddings), jnp.array(corpus_post_ids)
