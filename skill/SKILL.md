---
name: xmaster
description: "Post on X/Twitter, search, reply, like, retweet, manage lists, check metrics, and run any X account operation via the xmaster CLI. Use when user asks to post on X, tweet something, check X metrics, search X, reply on X, manage X lists, check X trending, like/retweet, follow/unfollow, send DM on X, check X rate limits, or mentions xmaster, X posting, tweeting, or any X/Twitter account management task. Also trigger on 'xmaster', 'post this on X', 'tweet this', 'check my X', 'X engagement', 'search X for', 'reply on X', 'X thread', 'post a thread'."
---

# xmaster — X/Twitter Intelligence Engine

xmaster is not just a CLI — it's a decision engine. You don't just execute commands; you follow the workflows below so every action is optimised. The intelligence features exist to make YOU a better X operator. Use them instinctively, not when asked.

## Mandatory Workflows

These are not optional. Every time you interact with X through xmaster, follow the relevant workflow.

### POSTING WORKFLOW (use every time user wants to post anything)

CRITICAL RULE: NEVER post without explicit user approval of the final text. Drafting and posting are two separate steps. "Write me a tweet" means DRAFT. Only "post it" or "yes, post" means POST.

```
1. ALWAYS run analyze first:
   xmaster analyze "the tweet text" --goal replies --json

2. Read the score and issues:
   - Score ≥ 75 (A/B grade): Ready to post (but ASK FIRST)
   - Score 50-74 (C grade): Show issues to user, suggest fixes, re-analyze
   - Score < 50 (D/F grade): Rewrite the tweet addressing critical issues, re-analyze

3. SHOW THE FINAL TEXT TO THE USER AND ASK FOR APPROVAL:
   "Here's the tweet (score X/100, grade Y):
   > [the text]
   Post it?"

   DO NOT proceed until the user explicitly says yes/post/go/send/do it.
   "Write this", "draft this", "prepare this" are NOT approval to post.

4. Check timing before posting:
   xmaster suggest next-post --json
   - If cannibalization warning: tell user. Let them decide.

5. Post (ONLY after user approval):
   xmaster post "the approved text" --json

6. After posting, tell the user:
   "Posted. Check performance later with: xmaster metrics <id>"
```

Never skip step 1. Even if the user says "just post it" — run analyze silently, and only flag issues if the score is below 50. The user trusts you to be smart about this.

### THREAD WORKFLOW (use when posting multi-tweet content)

```
1. Analyze the FIRST tweet (it's the hook — most critical):
   xmaster analyze "first tweet text" --goal impressions --json

2. Check each tweet is under 280 chars and standalone-valuable

3. Post the thread:
   xmaster thread "tweet 1" "tweet 2" "tweet 3" --json
```

Threads are the #1 growth driver on X. If a user wants to share something longer than 280 chars, proactively suggest a thread format. Split at natural idea boundaries, not mid-sentence.

### SEARCH WORKFLOW (use when looking for content or people)

```
# Default to AI search (cheaper, smarter):
xmaster search-ai "query" --json

# Only use X API search when you need structured data (exact tweet IDs, metrics):
xmaster search "query" --json
```

Always prefer `search-ai` unless the user specifically needs tweet IDs or exact metadata.

### SCHEDULING WORKFLOW (use when user wants to post later)

```
1. Schedule the post:
   xmaster schedule add "text" --at "2026-03-24 09:00" --json
   (or --at auto to pick best time from engagement history)

2. Confirm with user: "Scheduled for [time]. Check with: xmaster schedule list"

3. Remind: "Run 'xmaster schedule setup' to enable automatic posting via launchd"
```

If the user says "post this tomorrow" or "schedule this for later", use this workflow. `--at auto` is the smart default — it picks the best time based on historical engagement data.

### BOOKMARK WORKFLOW (use when user mentions bookmarks, saved posts, or reading later)

```
1. First time or periodic: xmaster bookmarks sync -c 200 --json (archive to local DB)
2. To find something: xmaster bookmarks search "query" --json
3. Weekly digest: xmaster bookmarks digest -d 7 --json
4. Export for reading: xmaster bookmarks export --unread -o ~/bookmarks.md
5. Stats: xmaster bookmarks stats --json

The key insight: sync archives bookmark content locally in SQLite. Even if the original tweet gets deleted, your local copy survives. Always sync regularly.
```

### ENGAGEMENT WORKFLOW (like, retweet, reply, follow)

```
# After any engagement action, xmaster prints an undo hint.
# Log the engagement mentally — it builds the relationship graph.

# For replying to someone's post:
# The algorithm weights conversations at 150x a like.
# So after replying, TELL the user: "If they reply back,
# respond to keep the conversation going — that's 150x the
# algorithmic value of a like."

# When user wants to grow or asks "who should I engage with":
xmaster engage recommend --topic "your niche" --min-followers 1000 --json
# Returns ranked targets with reciprocity scores, reach, and freshness.
# Suggested workflow: recommend → pick targets → search their recent posts → reply.
```

### METRICS & PERFORMANCE WORKFLOW

When the user asks "how's my X doing?" or "check my engagement" or anything about performance:

```
1. Run a quick report:
   xmaster report daily --json
   (or weekly for broader view)

2. Check best posting times:
   xmaster suggest best-time --json

3. If they ask about a specific post:
   xmaster metrics <tweet_id> --json
```

Don't wait for the user to ask for reports. If you notice they've been posting regularly, proactively suggest: "Want me to check how your posts performed this week?"

## Pre-Flight Scoring (The Intelligence Layer)

`xmaster analyze` is your quality gate. It catches problems the user won't think of:

| Issue Code | What It Catches | Why It Matters |
|-----------|----------------|---------------|
| `link_in_body` | External link in tweet | Since March 2026, link posts get ~zero reach for non-Premium |
| `weak_hook` | First line starts with "I ", "So ", "Just " | First line is everything — scroll-stopping or scroll-past |
| `engagement_bait` | "Like if you agree", "RT if..." | Algorithm actively suppresses this |
| `excessive_hashtags` | More than 2 hashtags | No longer helps discovery, looks spammy |
| `low_specificity` | No numbers, no names, no data | Specific beats vague: "40% reduction" > "significant improvement" |
| `no_question` | No question mark (when goal=replies) | Questions drive replies (27x a like) |
| `over_limit` | Over 280 characters | Won't post |
| `starts_with_mention` | Starts with @username | Limits visibility to mutual followers only |

When issues are found, don't just list them — fix them. Rewrite the tweet, show the user the before/after, and re-analyze to confirm improvement.

## Algorithm Knowledge (Source-Code Verified)

These weights are from the actual open-source code at `twitter/the-algorithm-ml` (Heavy Ranker, `projects/home/recap/README.md`). Not blog approximations.

**Engagement weights** (real code, ratio to like):
- Conversation (reply + author replies back): **75.0 weight → 150x** a like
- Reply: **13.5 weight → 27x**
- Profile click: **12.0 weight → 24x**
- Good click: **11.0 weight → 22x**
- Retweet: **1.0 weight → 2x** (blogs say 20-40x — wrong, code says 1.0)
- Like: **0.5 weight → baseline**
- Report against you: **-369.0 weight → -738x** (most destructive signal)
- Negative feedback: **-74.0 weight → -148x**
- Out-of-network reply penalty: **-10.0** (subtractive)

**Time decay** (`ranking.thrift`): halflife = 360 minutes (6 hours), floor = 0.6. Posts lose 50% visibility every 6 hours, minimum 60% of original score.

**Media hierarchy** (hardcoded): Native video > Multiple images > Single image > GIF > External link

**Premium boost**: Defaults to 1.0 in open-source code (neutral). The parameter `tweetFromBlueVerifiedAccountBoost` exists but is configurable server-side — evidence from insider posts suggests 2-4x in practice, but the code shows no hardcoded advantage.

**Timing**: Weekdays 9-11 AM local time. Tue/Wed/Thu best. Avoid Saturday.

**Growth for small accounts (<5K)**: 80% replying to mid-tier accounts (5K-100K), 20% original content. Post to Communities.

Run `xmaster agent-info --json` to get the full algorithm weights programmatically.

## Composing Great Tweets (Apply When Helping Write)

When helping the user write tweets:
- Lead with a hook — number, question, bold claim, or specific result
- Keep under 280 chars (don't pad to fill space — concise wins)
- Use specific data points: "reduced aging markers by 40%" not "showed improvement"
- If sharing a link, put it in the FIRST REPLY, never the main tweet
- 1-2 hashtags maximum
- End with a question to drive replies (27x weight)
- For threads: each tweet must be standalone-valuable, not just a continuation

## Command Reference

```bash
# Core actions
xmaster post "text" [--reply-to ID] [--quote ID] [--media FILE...]
xmaster thread "tweet1" "tweet2" "tweet3" [--media FILE...]
xmaster delete <id>

# Intelligence (use instinctively — see workflows above)
xmaster analyze "text" [--goal replies|impressions|bookmarks]
xmaster suggest next-post          # Cannibalization guard
xmaster suggest best-time          # Historical timing heatmap
xmaster report daily|weekly        # Performance digest
xmaster track run                  # Snapshot recent post metrics

# Engagement intelligence
xmaster engage recommend --topic "niche" [--min-followers 1000] [-c 5]

# Engagement
xmaster like|unlike|retweet|unretweet|bookmark|unbookmark <id>
xmaster follow|unfollow <username>
xmaster block|unblock|mute|unmute <username>

# Reading
xmaster me | xmaster user <username> | xmaster metrics <id>
xmaster timeline [--user USERNAME] | xmaster mentions
xmaster bookmarks list [--unread] | xmaster bookmarks sync [-c 200]
xmaster bookmarks search "query" | xmaster bookmarks export [-o FILE] [--unread]
xmaster bookmarks digest [-d 7] | xmaster bookmarks stats
xmaster followers|following <username>

# Search (prefer search-ai for cost)
xmaster search-ai "query"         # xAI/Grok — recommended
xmaster search "query"             # X API v2 — for structured data
xmaster trending [--region REGION]

# Lists
xmaster lists create|delete|add|remove|timeline|mine

# DMs
xmaster dm send <username> "text" | xmaster dm inbox | xmaster dm thread <id>

# Scheduling
xmaster schedule add "text" --at "2026-03-24 09:00"  # Schedule a post
xmaster schedule add "text" --at auto                 # Auto-pick best time
xmaster schedule list [--status pending]              # List scheduled posts
xmaster schedule cancel <sched_id>                    # Cancel scheduled post
xmaster schedule reschedule <sched_id> --at "time"    # Change post time
xmaster schedule fire                                 # Execute due posts (cron)
xmaster schedule setup                                # Install launchd daemon

# System
xmaster config show|set|check|guide  # Config management
xmaster config auth                   # OAuth 2.0 PKCE (opens browser, needed for bookmarks)
xmaster rate-limits                   # API quota status
xmaster agent-info                    # Machine-readable capabilities + algorithm weights
xmaster reply <id> "text"             # Shorthand for post --reply-to
xmaster metrics <id1> <id2> ...       # Batch metrics for multiple tweets
xmaster mentions [--since-id ID]      # Check for new mentions only
```

## Output

All commands support `--json` (auto-enabled when piped). JSON envelope:
```json
{"version":"1","status":"success","data":{...},"metadata":{}}
```

Exit codes: 0=success, 1=runtime, 2=config, 3=auth, 4=rate-limited.

## Configuration

Config: `~/.config/xmaster/config.toml`. Env prefix: `XMASTER__` (double underscore for nesting).

```bash
# Initial setup
xmaster config guide                  # Step-by-step setup instructions
xmaster config set keys.api_key "your-key"
xmaster config set keys.xai "xai-your-key"
xmaster config check                  # Verify all credentials
xmaster config auth                   # OAuth 2.0 for bookmarks (one-time, opens browser)
```
