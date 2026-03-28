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

import jax.numpy as jnp
import numpy as np
import pytest

from grok import make_recsys_attn_mask


class TestMakeRecsysAttnMask:
    """Tests for the make_recsys_attn_mask function."""

    def test_output_shape(self):
        """Test that the output has the correct shape [1, 1, seq_len, seq_len]."""
        seq_len = 10
        candidate_start_offset = 5

        mask = make_recsys_attn_mask(seq_len, candidate_start_offset)

        assert mask.shape == (1, 1, seq_len, seq_len)

    def test_user_history_has_causal_attention(self):
        """Test that user+history positions (before candidate_start_offset) have causal attention."""
        seq_len = 8
        candidate_start_offset = 5

        mask = make_recsys_attn_mask(seq_len, candidate_start_offset)
        mask_2d = mask[0, 0]

        for i in range(candidate_start_offset):
            for j in range(candidate_start_offset):
                if j <= i:
                    assert mask_2d[i, j] == 1, f"Position {i} should attend to position {j}"
                else:
                    assert mask_2d[i, j] == 0, (
                        f"Position {i} should NOT attend to future position {j}"
                    )

    def test_candidates_attend_to_user_history(self):
        """Test that candidates can attend to all user+history positions."""
        seq_len = 8
        candidate_start_offset = 5

        mask = make_recsys_attn_mask(seq_len, candidate_start_offset)
        mask_2d = mask[0, 0]

        for candidate_pos in range(candidate_start_offset, seq_len):
            for history_pos in range(candidate_start_offset):
                assert mask_2d[candidate_pos, history_pos] == 1, (
                    f"Candidate at {candidate_pos} should attend to user+history at {history_pos}"
                )

    def test_candidates_attend_to_themselves(self):
        """Test that candidates can attend to themselves (self-attention)."""
        seq_len = 8
        candidate_start_offset = 5

        mask = make_recsys_attn_mask(seq_len, candidate_start_offset)
        mask_2d = mask[0, 0]

        for candidate_pos in range(candidate_start_offset, seq_len):
            assert mask_2d[candidate_pos, candidate_pos] == 1, (
                f"Candidate at {candidate_pos} should attend to itself"
            )

    def test_candidates_do_not_attend_to_other_candidates(self):
        """Test that candidates cannot attend to other candidates."""
        seq_len = 8
        candidate_start_offset = 5

        mask = make_recsys_attn_mask(seq_len, candidate_start_offset)
        mask_2d = mask[0, 0]

        for query_pos in range(candidate_start_offset, seq_len):
            for key_pos in range(candidate_start_offset, seq_len):
                if query_pos != key_pos:
                    assert mask_2d[query_pos, key_pos] == 0, (
                        f"Candidate at {query_pos} should NOT attend to candidate at {key_pos}"
                    )

    def test_full_mask_structure(self):
        """Test the complete mask structure with a small example."""
        # Sequence: [user, h1, h2, c1, c2, c3]
        # Positions:  0     1   2   3   4   5

        seq_len = 6
        candidate_start_offset = 3

        mask = make_recsys_attn_mask(seq_len, candidate_start_offset)
        mask_2d = mask[0, 0]

        # Expected mask structure:
        # Query positions are rows, key positions are columns
        # 1 = can attend, 0 = cannot attend
        #
        #        Keys:  u   h1  h2  c1  c2  c3
        # Query u   :   1   0   0   0   0   0
        # Query h1  :   1   1   0   0   0   0
        # Query h2  :   1   1   1   0   0   0
        # Query c1  :   1   1   1   1   0   0   <- c1 attends to user+history + self
        # Query c2  :   1   1   1   0   1   0   <- c2 attends to user+history + self
        # Query c3  :   1   1   1   0   0   1   <- c3 attends to user+history + self

        expected = np.array(
            [
                [1, 0, 0, 0, 0, 0],  # user
                [1, 1, 0, 0, 0, 0],  # h1
                [1, 1, 1, 0, 0, 0],  # h2
                [1, 1, 1, 1, 0, 0],  # c1: user+history + self
                [1, 1, 1, 0, 1, 0],  # c2: user+history + self
                [1, 1, 1, 0, 0, 1],  # c3: user+history + self
            ],
            dtype=np.float32,
        )

        np.testing.assert_array_equal(
            np.array(mask_2d),
            expected,
            err_msg="Full mask structure does not match expected pattern",
        )

    def test_dtype_preserved(self):
        """Test that the specified dtype is used."""
        seq_len = 5
        candidate_start_offset = 3

        mask_f32 = make_recsys_attn_mask(seq_len, candidate_start_offset, dtype=jnp.float32)
        mask_f16 = make_recsys_attn_mask(seq_len, candidate_start_offset, dtype=jnp.float16)

        assert mask_f32.dtype == jnp.float32
        assert mask_f16.dtype == jnp.float16

    def test_single_candidate(self):
        """Test edge case with a single candidate."""
        seq_len = 4
        candidate_start_offset = 3

        mask = make_recsys_attn_mask(seq_len, candidate_start_offset)
        mask_2d = mask[0, 0]

        expected = np.array(
            [
                [1, 0, 0, 0],
                [1, 1, 0, 0],
                [1, 1, 1, 0],
                [1, 1, 1, 1],
            ],
            dtype=np.float32,
        )

        np.testing.assert_array_equal(np.array(mask_2d), expected)

    def test_all_candidates(self):
        """Test edge case where all positions except first are candidates."""
        seq_len = 4
        candidate_start_offset = 1

        mask = make_recsys_attn_mask(seq_len, candidate_start_offset)
        mask_2d = mask[0, 0]

        expected = np.array(
            [
                [1, 0, 0, 0],  # user
                [1, 1, 0, 0],  # c1: user + self
                [1, 0, 1, 0],  # c2: user + self
                [1, 0, 0, 1],  # c3: user + self
            ],
            dtype=np.float32,
        )

        np.testing.assert_array_equal(np.array(mask_2d), expected)


if __name__ == "__main__":
    pytest.main([__file__, "-v"])
