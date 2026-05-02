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
```

`article draft` saves an unpublished native X Article draft through X's web Article entity endpoint and requires `xmaster config web-login`.

### Finding Posts to Reply To

Replies are ~20x a like in the 2026 algorithm. This is the #1 growth lever for small accounts.

```
# Find fresh posts from big accounts in your niche:
xmaster engage feed "longevity biotech" --min-followers 5000 --max-age-mins 60

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

The CLI embeds knowledge from `xai-org/x-algorithm` (January 2026). Key facts:

- **19 scoring signals** (15 positive, 4 negative) — weights unpublished but estimated
- Top signals: follow-from-post (~30x), reply (~20x), quote (~18x), profile click (~12x)
- **Negative signals are predictive** — Grok suppresses content it predicts users would block/mute
- **Bookmarks are NOT a signal** — ignore advice claiming otherwise
- **share_via_dm fires when OTHERS share YOUR post** — create DM-worthy content, don't DM your own posts
- **Author diversity decay** — space posts 2+ hours apart
- **Engagement history drives everything** — dormant accounts need 2 weeks of active engagement before the algorithm works

`xmaster analyze` checks sentiment (Grok suppresses combative tone), estimates dwell time, and flags algorithm-penalised patterns.

## Config

```
xmaster config set account.premium true    # Enable Premium-aware scoring
xmaster config set style.voice "your style" # Writing voice for drafts
```
