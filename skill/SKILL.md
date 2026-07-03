---
name: xmaster
description: "Post on X, search, reply, like, retweet, manage lists, check metrics, read any X post by URL or ID, and run any X account operation via the xmaster CLI. Use when user asks to post on X, tweet something, check X metrics, search X, reply on X, manage X lists, check X trending, like/retweet, follow/unfollow, send DM on X, check X rate limits, READ an X post/URL, check what someone posted, or mentions xmaster, X posting, tweeting, or any X account management task. Also trigger on 'xmaster', 'post this on X', 'tweet this', 'check my X', 'X engagement', 'search X for', 'reply on X', 'X thread', 'post a thread', any x.com URL, or 'read this tweet/post'."
---

# xmaster — X/Twitter CLI

Run `xmaster agent-info --json` to get full capabilities, commands, and algorithm intelligence.

## Core Workflows

### Posting

NEVER post without explicit user approval. Always analyze first.

```
1. xmaster analyze "text" --goal replies --json   # Score it
2. Show user the score/grade and final text
3. Only post after explicit "yes"/"post it"/"go"
4. xmaster post "approved text" --json
```

### X Articles

Articles are a separate X feature, not long posts / Note Tweets.

```
xmaster article preview draft.md --header-image cover.png -o preview.html
xmaster article draft draft.md --header-image cover.png --json
xmaster article draft draft.md --api --json          # official v2 endpoint, no cookies
xmaster article publish <article_id> --json          # makes the Article PUBLIC
```

`article draft` saves an unpublished native X Article draft through X's web Article entity endpoint (requires `xmaster config web-login`). With `--api` it uses the official X API v2 Articles endpoint (added June 2026) — OAuth keys only, but cover images stay in the body. `article publish` is official-API only and NEVER runs without explicit user approval.

### Finding Posts to Reply To

Replies are the #1 growth lever for small accounts — a top positive signal, and the only surface that reliably reaches non-followers. (Exact signal weights are unpublished runtime params; distrust any "Nx" figure.)

```
# Find fresh posts from big accounts in your niche:
xmaster engage feed "longevity biotech" --min-followers 5000 --max-age-mins 60

# After publishing, inspect the full conversation surface:
xmaster engage inbox <post_id> --json
# Checks root replies, quote tweets, and replies under quote tweets.
# Prioritizes author reply-back opportunities (reciprocity heuristic).

# Or find accounts to build relationships with:
xmaster engage recommend --topic "longevity" --min-followers 1000

# Search their recent posts (from: operator parsed into hard author filter):
xmaster search-ai "from:username"                    # Latest posts by user
xmaster search-ai "from:username longevity"           # Their posts on a topic
xmaster search-ai "from:user1 from:user2 senolytics"  # Multiple authors
```

### Checking Performance

```
xmaster timeline --user USERNAME --sort impressions -c 50    # Top posts by reach
xmaster timeline --user USERNAME --since 24h                 # Recent posts
xmaster track followers                                       # New/lost followers
xmaster track growth -d 30                                    # Follower history
xmaster metrics <id>                                          # Single post deep dive
```

## Algorithm Intelligence (2026)

The CLI embeds knowledge from `xai-org/x-algorithm` (May 2026 re-release). Key facts:

- **22 scoring signals** (13 positive, 4 continuous, 5 negative). Weight magnitudes are runtime params, NOT in the open source — treat any specific "Nx" multiplier you read online as fiction
- Strong positive signals: follow_author, share_via_dm, reply, profile_click, dwell. Strong negatives: not_dwelled (predicted scroll-past — why shallow one-liners get suppressed with zero explicit user action), not_interested, mute/block/report
- **Negative signals are predictive** — Grok suppresses content it predicts users would block/mute
- **Bookmarks are NOT a signal** — ignore advice claiming otherwise
- **share_via_dm fires when OTHERS share YOUR post** — create DM-worthy content, don't DM your own posts
- **Author diversity decay** — space posts 2+ hours apart
- **Engagement history drives everything** — dormant accounts need 2 weeks of active engagement before the algorithm works

`xmaster analyze` checks sentiment (Grok suppresses combative tone), estimates dwell time, and flags algorithm-penalised patterns.

## API Restrictions & Pricing (2026)

- **Likes, Follows, Quote-posts REMOVED from all self-serve API tiers (Apr 20, 2026).** `xmaster like/follow` will 403 on non-Enterprise keys — like/follow manually in the app. Quote posts auto-fall back to the web session (needs `xmaster config web-login`)
- **Replies to non-mentioners are API-blocked (Feb 2026)** — xmaster auto-falls back to the web session
- **Pay-per-use pricing**: $0.015/post, $0.20/post if it contains a URL (one more reason links go in the first reply), $0.001/owned read. HTTP 402 / exit code 2 = credits exhausted → top up at console.x.com
- **Search (since May 2026)**: `min_likes:`, `min_replies:`, `min_reposts:` operators work in queries; keyword search no longer returns retweets

## Config

```
xmaster config set account.premium true    # Enable Premium-aware scoring
xmaster config set style.voice "your style" # Writing voice for drafts
```
