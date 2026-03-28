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

import numpy as np

from grok import TransformerConfig
from recsys_model import HashConfig
from recsys_retrieval_model import PhoenixRetrievalModelConfig
from runners import (
    RecsysRetrievalInferenceRunner,
    RetrievalModelRunner,
    create_example_batch,
    create_example_corpus,
    ACTIONS,
)


def main():
    # Model configuration - same architecture as Phoenix ranker
    emb_size = 128  # Embedding dimension
    num_actions = len(ACTIONS)  # Number of explicit engagement actions
    history_seq_len = 32  # Max history length
    candidate_seq_len = 8  # Max candidates per batch (for training)

    # Hash configuration
    hash_config = HashConfig(
        num_user_hashes=2,
        num_item_hashes=2,
        num_author_hashes=2,
    )

    # Configure the retrieval model - uses same transformer as Phoenix
    retrieval_model_config = PhoenixRetrievalModelConfig(
        emb_size=emb_size,
        history_seq_len=history_seq_len,
        candidate_seq_len=candidate_seq_len,
        hash_config=hash_config,
        product_surface_vocab_size=16,
        model=TransformerConfig(
            emb_size=emb_size,
            widening_factor=2,
            key_size=64,
            num_q_heads=2,
            num_kv_heads=2,
            num_layers=2,
            attn_output_multiplier=0.125,
        ),
    )

    # Create inference runner
    inference_runner = RecsysRetrievalInferenceRunner(
        runner=RetrievalModelRunner(
            model=retrieval_model_config,
            bs_per_device=0.125,
        ),
        name="retrieval_local",
    )

    print("Initializing retrieval model...")
    inference_runner.initialize()
    print("Model initialized!")

    # Create example batch with simulated user and history
    print("\n" + "=" * 70)
    print("RETRIEVAL SYSTEM DEMO")
    print("=" * 70)

    batch_size = 2  # Two users for demo
    example_batch, example_embeddings = create_example_batch(
        batch_size=batch_size,
        emb_size=emb_size,
        history_len=history_seq_len,
        num_candidates=candidate_seq_len,
        num_actions=num_actions,
        num_user_hashes=hash_config.num_user_hashes,
        num_item_hashes=hash_config.num_item_hashes,
        num_author_hashes=hash_config.num_author_hashes,
        product_surface_vocab_size=16,
    )

    # Count valid history items
    valid_history_count = int((example_batch.history_post_hashes[:, :, 0] != 0).sum())  # type: ignore
    print(f"\nUsers have viewed {valid_history_count} posts total in their history")

    # Step 1: Create a corpus of candidate posts
    print("\n" + "-" * 70)
    print("STEP 1: Creating Candidate Corpus")
    print("-" * 70)

    corpus_size = 1000  # Simulated corpus of 1000 posts
    corpus_embeddings, corpus_post_ids = create_example_corpus(
        corpus_size=corpus_size,
        emb_size=emb_size,
        seed=456,
    )
    print(f"Corpus size: {corpus_size} posts")
    print(f"Corpus embeddings shape: {corpus_embeddings.shape}")

    # Set corpus for retrieval
    inference_runner.set_corpus(corpus_embeddings, corpus_post_ids)

    # Step 2: Retrieve top-k candidates for each user
    print("\n" + "-" * 70)
    print("STEP 2: Retrieving Top-K Candidates")
    print("-" * 70)

    top_k = 10
    retrieval_output = inference_runner.retrieve(
        example_batch,
        example_embeddings,
        top_k=top_k,
    )

    print(f"\nRetrieved top {top_k} candidates for each of {batch_size} users:")

    top_k_indices = np.array(retrieval_output.top_k_indices)
    top_k_scores = np.array(retrieval_output.top_k_scores)

    for user_idx in range(batch_size):
        print(f"\n  User {user_idx + 1}:")
        print(f"    {'Rank':<6} {'Post ID':<12} {'Score':<12}")
        print(f"    {'-' * 30}")
        for rank in range(top_k):
            post_id = top_k_indices[user_idx, rank]
            score = top_k_scores[user_idx, rank]
            bar = "█" * int((score + 1) * 10) + "░" * (20 - int((score + 1) * 10))
            print(f"    {rank + 1:<6} {post_id:<12} {bar} {score:.4f}")

    print("\n" + "=" * 70)
    print("Demo complete!")
    print("=" * 70)


if __name__ == "__main__":
    logging.basicConfig(level=logging.INFO)
    main()
