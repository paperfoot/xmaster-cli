# X Algorithm Source Code Analysis (2026)

Analysis of X's **current** recommendation algorithm from [`xai-org/x-algorithm`](https://github.com/xai-org/x-algorithm) (January 2026, Rust + Python/JAX).

The full source code is in `source-2026/`.

> **No 2023 contamination.** The previous `twitter/the-algorithm` (TweepCred, SimClusters, Real Graph, MaskNet) has been completely replaced. None of those systems exist in the current algorithm.

## Documents

| # | File | Focus |
|---|---|---|
| 01 | [Architecture](01-architecture-2026.md) | Complete pipeline from gRPC to ranked feed — Phoenix, Thunder, WeightedScorer, all 12 filters |
| 02 | [Engagement Weights](02-engagement-weights-deduced.md) | Deduced weight hierarchy for all 19 signals from code + empirical data |
| 03 | [Small Account Analysis](03-small-account-analysis-2026.md) | Cold start problem for ~96 follower dormant account in the 2026 system |
| 04 | [Growth Playbook](04-growth-playbook-2026.md) | Actionable steps tagged [CODE], [EMPIRICAL], or [INFERRED] |
| 05 | [Negative Signals](05-negative-signals-2026.md) | offset_score() analysis, 4 negative signals, Grok sentiment, penalties |

## The 19 Engagement Signals (from `weighted_scorer.rs`)

### 15 Positive
`favorite`, `reply`, `retweet`, `photo_expand`, `click`, `profile_click`, `vqv` (video quality view), `share`, `share_via_dm`, `share_via_copy_link`, `dwell`, `quote`, `quoted_click`, `dwell_time` (continuous), `follow_author`

### 4 Negative
`not_interested`, `block_author`, `mute_author`, `report`

## Key Findings

- **Bookmarks are NOT a signal** — not in `weighted_scorer.rs`. The "bookmarks = 10x" claim is from dead 2023 code.
- **`reply_engaged_by_author` (the 150x "cheat code") is gone** — replaced by `reply_score` only.
- **DM shares and copy-link shares are separate signals** — `share_via_dm_score` and `share_via_copy_link_score` each get independent weights. Underrated.
- **Dormant accounts literally can't be scored** — `UserActionSeqQueryHydrator` errors on empty history. Need ~2 weeks of active engagement before Phoenix works.
- **No TweepCred, no "3 tweet limit"** — removed in 2026 rewrite. No follower-ratio penalty in code.
- **No SimClusters** — community discovery replaced by Phoenix two-tower retrieval.
- **Premium boost not in this repo** — operates at a different layer. Empirical data confirms ~10x reach.
- **"We have eliminated every single hand-engineered feature"** — direct quote from README.

## Evidence Tagging

All recommendations in the playbook are tagged:
- **[CODE]** — directly from `xai-org/x-algorithm` source
- **[EMPIRICAL]** — from published 2025-2026 research and experiments
- **[INFERRED]** — logical deduction from code architecture

---

*Generated 2026-03-28 from xai-org/x-algorithm source code by 5 parallel analysis agents.*
