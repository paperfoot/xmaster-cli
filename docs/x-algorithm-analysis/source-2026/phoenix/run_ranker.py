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
from recsys_model import PhoenixModelConfig, HashConfig
from runners import RecsysInferenceRunner, ModelRunner, create_example_batch, ACTIONS


def main():
    # Model configuration
    emb_size = 128  # Embedding dimension
    num_actions = len(ACTIONS)  # Number of explicit engagement actions
    history_seq_len = 32  # Max history length
    candidate_seq_len = 8  # Max candidates to rank

    # Hash configuration
    hash_config = HashConfig(
        num_user_hashes=2,
        num_item_hashes=2,
        num_author_hashes=2,
    )

    recsys_model = PhoenixModelConfig(
        emb_size=emb_size,
        num_actions=num_actions,
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
    inference_runner = RecsysInferenceRunner(
        runner=ModelRunner(
            model=recsys_model,
            bs_per_device=0.125,
        ),
        name="recsys_local",
    )

    print("Initializing model...")
    inference_runner.initialize()
    print("Model initialized!")

    # Create example batch with simulated posts
    print("\n" + "=" * 70)
    print("RECOMMENDATION SYSTEM DEMO")
    print("=" * 70)

    batch_size = 1
    example_batch, example_embeddings = create_example_batch(
        batch_size=batch_size,
        emb_size=emb_size,
        history_len=history_seq_len,
        num_candidates=candidate_seq_len,
        num_actions=num_actions,
        num_user_hashes=hash_config.num_user_hashes,
        num_item_hashes=hash_config.num_item_hashes,
        num_author_hashes=hash_config.num_author_hashes,
        product_surface_vocab_size=recsys_model.product_surface_vocab_size,
    )

    action_names = [action.replace("_", " ").title() for action in ACTIONS]

    # Count valid history items (where first post hash is non-zero)
    valid_history_count = int((example_batch.history_post_hashes[:, :, 0] != 0).sum())  # type: ignore
    print(f"\nUser has viewed {valid_history_count} posts in their history")
    print(f"Ranking {candidate_seq_len} candidate posts...")

    # Rank candidates
    ranking_output = inference_runner.rank(example_batch, example_embeddings)

    # Display results
    scores = np.array(ranking_output.scores[0])  # [num_candidates, num_actions]
    ranked_indices = np.array(ranking_output.ranked_indices[0])  # [num_candidates]

    print("\n" + "-" * 70)
    print("RANKING RESULTS (ordered by predicted 'Favorite Score' probability)")
    print("-" * 70)

    for rank, idx in enumerate(ranked_indices):
        idx = int(idx)
        print(f"\nRank {rank + 1}: ")
        print("  Predicted engagement probabilities:")
        for action_idx, action_name in enumerate(action_names):
            prob = float(scores[idx, action_idx])
            bar = "█" * int(prob * 20) + "░" * (20 - int(prob * 20))
            print(f"    {action_name:24s}: {bar} {prob:.3f}")

    print("\n" + "=" * 70)
    print("Demo complete!")
    print("=" * 70)


if __name__ == "__main__":
    logging.basicConfig(level=logging.INFO)
    main()
