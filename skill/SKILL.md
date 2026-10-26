---
name: xmaster
description: "Post, reply, read, search and engage on X (Twitter) through the xmaster CLI. Use when the user asks to post on X, reply to a post or thread, read an X post or x.com URL, check replies or mentions, find posts to reply to, like, repost, quote, follow, check X metrics, search X, schedule posts, write X Articles, or mentions xmaster. Also trigger on 'post this on X', 'tweet this', 'reply to this', 'check my X', 'X engagement', 'search X for', 'X thread', any x.com or twitter.com URL."
---

# xmaster: X for agents

xmaster reads and writes X through the user's logged-in browser session, so it can reply to any thread, like, follow and quote like a person in the app, at no API cost. Output is compact JSON when piped.

X's developer team likes this CLI. Boris spoke with someone on that team on 13 October 2026. Keep what the client sends in line with the x.com app.

Call X content "posts" in everything you write for the user. Command names (`retweet`, `retweeters`) stay as they are; `repost`, `unrepost` and `reposters` are aliases.

## Start

```
xmaster agent-info                 # every command on one line, exit codes, cost model, JEV (~7 KB)
xmaster agent-info --command "engage feed"  # one command in full: args, flags, defaults, allowed values
xmaster agent-info --tips replies  # strategy on demand: algorithm|replies|posting|growth|metrics|avoid
xmaster config check               # live read-only test of the web session and keys
```

If `config check` reports no web session, run `xmaster config web-login`. It copies the X cookies from the user's browser, which must be logged in to x.com. Without it, xmaster falls back to the paid X API, which can't like, follow or quote, and can only reply to people who mentioned the user.

## What needs the user's yes

The user often can't type, so keep friction low: when they ask for an action, do it.

- Do without asking: every read, search, analysis, draft and `--dry-run`. Likes, follows, reposts and bookmarks the user asked for.
- Replies: when the user asked you to reply or engage, send replies that pass the draft check. Show the text when you drafted it on your own initiative.
- Ask first (a one-word yes is enough): original posts whose text the user hasn't seen, DMs, deletes, `article publish`, anything linking to a product the user sells.
- When the user says "post this: <text>", post exactly that text.

## Reply to any thread

```
xmaster engage feed                          # fresh posts worth a reply (uses niche.topics)
xmaster engage feed "longevity" "AI agents"  # or name topics
xmaster conversation <id|url>                # parent chain, the post, top replies: one free call
xmaster reply <id|url> "draft" --dry-run     # preflight, JEV flags, length, route, cost; sends nothing
xmaster reply <id|url> "final text"
```

For a URL the user hands you, start at `conversation <url>`. For a quick look, `read <url>`.

Write replies that X's reply judge and readers both reward:
- Name a specific detail from the post (a claim, number, method or result), or ask the author something they can answer. An author who likes or answers your reply lifts it under the post.
- No generic praise ("Great post!", "100%"), no AI filler, no hashtags, no emoji decoration.
- No links and no more than one @mention in a reply to a stranger.
- One reply per stranger's conversation. Never reply to your own reply under someone else's post.
- If `--dry-run` shows JEV flags (generic, slop, off_topic, bait), rewrite once and check again.
- Match the user's voice: `xmaster config get style.voice`.

Quote instead of reply when the user has a take worth showing their own followers: `xmaster quote <id> "take"`. A quote reaches the user's followers' feeds; a reply reaches the people reading that thread.

## Triage replies and mentions

```
xmaster engage inbox <own post id>   # replies, quote posts and replies under quotes
xmaster mentions -n 20
xmaster replies <id>                 # with JEV: only replies worth reading; --all for every one
```

With JEV on, `engage inbox` puts only items that need an answer in `recommendations`, and lists posts to like in `triage.like_ids`. Spam and ignorable replies are left out. Answer within the first hour of a post's life when you can.

## Post

```
xmaster post "text" --dry-run        # preflight score, JEV flags, X-weighted length, route, cost
xmaster post "text"
xmaster post "text" --media a.jpg    # up to 4 files
xmaster thread "first" "second" "third"
xmaster schedule add "text" --at auto
```

- Put the hook in the first line. Posts over 280 characters need Premium (`account.premium true`); their first 280 characters are the preview.
- Links in the body are fine; avoid link shorteners.
- Leave about 2 hours between original posts.
- Keep threads short: only the best post of a conversation reaches a feed.
- Articles are separate from long posts: `article preview draft.md`, `article draft draft.md` (saves an unpublished draft), `article publish <id>` (public; needs the user's yes).

## When a write fails

Read `error.code`, `error.suggestion`, `error.retryable` and `error.retry_after_secs` from the JSON on stderr.

| Exit | Meaning | Do |
|---|---|---|
| 1 | runtime error | retry only if `retryable`, after `retry_after_secs` |
| 2 | setup | follow `suggestion` (often `config web-login` or `config check`); don't retry |
| 3 | bad input | fix the ID, handle or flags |
| 4 | rate limited | wait `retry_after_secs`, then retry |
| 5 | blocked by X (duplicate text, reply limits, protected account) | don't retry unchanged; change the text or target |

- `write_unconfirmed`: the write may have gone through. Check with `xmaster timeline` or `xmaster conversation <id>` before sending again.
- Writes are paced with daily limits and gaps between actions, so agent activity stays within normal use. If a write is refused for pacing, the error says why: wait or do other work, then try later. Never loop on it.
- The caps (`rails.replies_per_hour` 5, `rails.replies_per_day` 20 and the other `rails.*` keys) are counted on this machine, so every session shares them. `xmaster rails status` shows what used each one. Raise a cap only when the user asks.
- If X shows a captcha, a lock or an "automated" warning (code 226), stop all writes and ask the user to open x.com and clear it.
- **A reply that fails is never split into a thread or re-sent in pieces.** Copy the text to the clipboard (`printf '%s' "text" | pbcopy` on macOS) and tell the user, so they can paste it in the app.

## Read and measure

```
xmaster read <id|url>                          # text, author, metrics, media
xmaster timeline --user <handle> --sort impressions -n 50
xmaster timeline --user <handle> --since 24h
xmaster search "query min_likes:50" -n 20      # X search operators work
xmaster metrics ID1 ID2 ID3                    # one call for up to 100 posts; never loop per ID
xmaster track run                              # snapshot recent posts, record reply-backs
xmaster engage hot-targets                     # accounts where the user's replies did best
xmaster user <handle>
```

`metrics` returns age, change since the last snapshot and velocity, so you don't need to do clock arithmetic.

## JEV judgments

With a TypeSafe JEV key (`xmaster config set keys.jev KEY` or `TYPESAFE_API_KEY`), xmaster judges items itself and returns what matters. Trust its cut; don't pull the full lists back into context.

- `engage feed`: its best 5 reply targets, each with `jev_score` and `why`.
- `engage inbox`: `recommendations` need a reply; like `triage.like_ids`.
- `replies`: drops replies not worth reading (`trimmed` count).
- `analyze`, `post`, `reply`, `quote`: advisory flags (generic, slop, off_topic, bait), each with a fix. They never block.

JEV judges fit against `account.bio` and `niche.topics`, so set both. With no key, `settings.disable_jev true`, or a failed call, output falls back to the plain heuristics.

## Cost

- Free with the web session: reading posts, threads, replies, search, timelines, mentions and profiles; every post, reply, quote, like, repost, bookmark, follow, delete, mute and block.
- Paid X API (pay-per-use): only without a web session, and for polls, DMs, lists, bookmarks sync, likers, reposters, quote lists, follower lists, batch user lookups and the user's own private metrics. A post via the API costs $0.015, or $0.20 with a link. `--dry-run` shows the route and cost.
- `search-ai` and `trending` use the xAI key.

## How X ranks posts (from X's published code)

Source: `xai-org/x-algorithm`, `home-mixer/params/param.rs` (production values, synced 12 Oct 2026). Full table: `xmaster agent-info --tips algorithm`.

- Score = sum of weight × the viewer's predicted probability of each action. Copy-link share 20, reply 5 (20 on an original post by a mutual follow), quote 5, DM share 5, follow 4, share 2, repost 1, like 0.6. Negatives: report −234, mute −58.8, not interested −47.5, block −31.2. Profile clicks, video watch time and bookmarks weigh 0.
- A post whose predicted negatives outweigh its positives sinks below every positive post. Avoid what people would mute or report.
- An author's 2nd, 3rd and 4th unseen post in a viewer's feed score ×0.625, ×0.44 and ×0.34. Out-of-network posts, and in-network replies and reposts, score ×0.75.
- Cold start: for authors with up to 50,000 followers, an original post under 2 hours old with fewer than 200 home impressions can win one slot at rank 16, picked by likes per impression. Replies and reposts aren't eligible.
- For You never shows a reply to people who don't follow the replier. Strangers see replies under the post, ordered by a model that weighs the author liking or answering the reply, the author following the replier, verified status, a reply-quality score and blocks, mutes and reports.
- Links carry no ranking penalty. Shorteners and low-quality redirects hide a post from non-followers.
- Engagement-bait wording, near-duplicate replies, two or more @mentions and insults each trigger labels that hide posts from non-followers. An account-level AI-slop label hides all of the user's posts from non-followers for 30 days.

## Config

```
xmaster config set account.bio "..."            # who the user is; JEV and engage feed use it
xmaster config set niche.topics "a,b,c"         # default topics for engage feed
xmaster config set style.voice "..."            # writing voice for drafts
xmaster config set account.premium true         # 25,000-character posts
xmaster config set settings.web_writes auto     # always (default) | auto | never
```

`XMASTER_READ_ONLY=1` blocks every write to X; `--dry-run` still works. `XMASTER_ALLOW_COMMANDS` and `XMASTER_DENY_COMMANDS` restrict commands (deny wins).
