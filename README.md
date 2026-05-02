<div align="center">

# XMaster

**The X/Twitter CLI for developers and AI agents**

<br />

[![Star this repo](https://img.shields.io/github/stars/paperfoot/xmaster-cli?style=for-the-badge&logo=github&label=%E2%AD%90%20Star%20this%20repo&color=yellow)](https://github.com/paperfoot/xmaster-cli/stargazers)
&nbsp;&nbsp;
[![Follow @longevityboris](https://img.shields.io/badge/Follow_%40longevityboris-000000?style=for-the-badge&logo=x&logoColor=white)](https://x.com/longevityboris)

<br />

[![Crates.io](https://img.shields.io/crates/v/xmaster?style=for-the-badge&logo=rust&logoColor=white&label=crates.io)](https://crates.io/crates/xmaster)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue?style=for-the-badge)](https://github.com/paperfoot/xmaster-cli/blob/main/LICENSE)
[![Homebrew](https://img.shields.io/badge/Homebrew-available-orange?style=for-the-badge&logo=homebrew&logoColor=white)](https://github.com/199-biotechnologies/homebrew-tap)
[![Built with Rust](https://img.shields.io/badge/Built_with-Rust-dea584?style=for-the-badge&logo=rust&logoColor=white)](https://www.rust-lang.org/)

---

Post, reply, like, retweet, DM, search, schedule, and analyze -- all from your terminal. XMaster is a single Rust binary that wraps the X API v2, xAI/Grok search, and a web session fallback into one tool. It outputs structured JSON for AI agents and readable tables for humans.

[Install](#install) | [How It Works](#how-it-works) | [Commands](#commands) | [Contributing](#contributing)

</div>

## Why This Exists

I wanted my AI agents to handle X for me. Find posts in my niche, draft replies in my voice, track what works. Not for spamming -- just a less tedious way to stay engaged when I'd rather be building things.

Most X tools make you pick between the official API and scraping. XMaster gives you both, plus the parts nobody else builds: pre-flight post analysis, engagement scoring, reply bypass, local scheduling, and a bookmarks archive that survives tweet deletions.

## Install

**One-liner (macOS / Linux):**
```bash
curl -fsSL https://raw.githubusercontent.com/paperfoot/xmaster-cli/master/install.sh | sh
```

**Homebrew:**
```bash
brew tap 199-biotechnologies/tap
brew install xmaster
```

**Cargo (crates.io):**
```bash
cargo install xmaster
```

**From source:**
```bash
cargo install --git https://github.com/paperfoot/xmaster-cli
```

### Quick Start

```bash
# 1. Get your X API keys from https://developer.x.com

# 2. Configure credentials
xmaster config set keys.api_key YOUR_API_KEY
xmaster config set keys.api_secret YOUR_API_SECRET
xmaster config set keys.access_token YOUR_ACCESS_TOKEN
xmaster config set keys.access_token_secret YOUR_ACCESS_TOKEN_SECRET

# 3. Verify setup
xmaster config check

# 4. Post
xmaster post "Hello from xmaster"
```

Optional extras:
```bash
xmaster config set keys.xai YOUR_XAI_KEY          # AI-powered search via xAI/Grok
xmaster config web-login                           # Enable reply bypass (auto-captures browser cookies)
xmaster config set style.voice "your style here"   # Agents write in your voice
xmaster config set account.premium true            # 25k char limit instead of 280
```

## How It Works

XMaster has three backends behind one CLI:

| Backend | Auth | Used for |
|---------|------|----------|
| **X API v2** | OAuth 1.0a | Posting, likes, retweets, DMs, search, timelines, follows |
| **xAI / Grok** | Bearer token | AI-powered semantic search, trending topics |
| **Web GraphQL** | Browser cookies | Reply bypass when X blocks API replies to strangers |

Every command returns JSON when piped (or with `--json`). Exit codes are semantic: 0 success, 1 runtime error, 2 config error, 3 auth missing, 4 rate limited. Your agent always knows what happened.

```
┌─────────────────────────────────────────────┐
│                 CLI Layer                    │
│   clap + comfy-table (--json / human)       │
├─────────────────────────────────────────────┤
│          Command Router + Pre-flight        │
│   Analyze, score, cannibalization guard     │
├──────────┬──────────────┬───────────────────┤
│ X API v2 │  xAI / Grok  │  Web GraphQL      │
│(OAuth1.0a│(Bearer token)│  (Cookie auth +   │
│ Post,Like│ AI search,   │   transaction ID) │
│ RT, DM,  │ Trending,    │  Reply fallback   │
│ Search,  │ Semantic     │  when API blocks  │
│ Timeline)│ search       │  replies          │
├──────────┴──────────────┴───────────────────┤
│  Rate Limiter │ Intel Store │ Scheduler     │
│  (header-based)│  (SQLite)  │  (launchd)    │
├─────────────────────────────────────────────┤
│             Config (figment)                │
│   TOML + env vars + browser cookies         │
└─────────────────────────────────────────────┘
```

## Commands

### Posting and Engagement

| Command | What it does | Example |
|---------|-------------|---------|
| `post` | Post text, media, replies, quotes, polls | `xmaster post "Hello world"` |
| `reply` | Reply to a post (auto-bypasses API restrictions) | `xmaster reply 1234567890 "Great point"` |
| `thread` | Post a multi-tweet thread | `xmaster thread "First" "Second" "Third"` |
| `article preview` | Generate an X Articles-style HTML preview from Markdown | `xmaster article preview draft.md --header-image cover.png -o preview.html` |
| `article draft` | Save a native X Article draft without publishing | `xmaster article draft draft.md --header-image cover.png` |
| `delete` | Delete a post | `xmaster delete 1234567890` |
| `like` | Like a tweet (ID or URL) | `xmaster like 1234567890` |
| `unlike` | Unlike a tweet | `xmaster unlike 1234567890` |
| `retweet` | Retweet a tweet | `xmaster retweet 1234567890` |
| `unretweet` | Undo a retweet | `xmaster unretweet 1234567890` |
| `bookmark` | Bookmark a tweet | `xmaster bookmark 1234567890` |
| `unbookmark` | Remove a bookmark | `xmaster unbookmark 1234567890` |

Post options:
```bash
xmaster post "Great point!" --reply-to 1234567890      # Reply
xmaster post "This is big" --quote 1234567890           # Quote tweet
xmaster post "Check this" --media photo.jpg             # Attach media (up to 4)
xmaster post "Best language?" --poll "Rust,Go,Python"   # Create a poll
xmaster like https://x.com/user/status/1234567890       # URLs work too
```

`article preview` and `article draft` are for X Articles, not long posts / Note Tweets. Drafts use Markdown and map to the native Article surface: `#` title, `##`/`###` headings, `**bold**`, `*italic*`, `~~strikethrough~~`, indentation/quotes, numbered/bulleted lists, links, images, video/GIF directives, and embedded X posts. Use `::video[Caption](clip.mp4)`, `::gif[Caption](loop.gif)`, `::post(https://x.com/user/status/123)`, or `::article(https://x.com/i/article/123)` for media and embeds that Markdown does not model directly. Native draft creation uses X's private web Article entity endpoint (`ArticleEntityDraftCreate`) with browser cookies from `xmaster config web-login`; it saves a draft and does not publish.

### Reading and Discovery

| Command | What it does | Example |
|---------|-------------|---------|
| `read` | Full post lookup (text, author, metrics, media) | `xmaster read 1234567890` |
| `replies` | Get replies on a post | `xmaster replies 1234567890 -c 30` |
| `metrics` | Detailed engagement metrics | `xmaster metrics 1234567890` |
| `timeline` | Home or user timeline | `xmaster timeline --user elonmusk --since 24h` |
| `mentions` | Your mentions | `xmaster mentions -c 20` |
| `user` | User profile info | `xmaster user elonmusk` |
| `me` | Your own profile | `xmaster me` |

### Search

| Command | What it does | Example |
|---------|-------------|---------|
| `search` | X API v2 search (structured, filterable) | `xmaster search "rust lang" --mode recent` |
| `search-ai` | AI-powered search via xAI/Grok | `xmaster search-ai "latest AI news"` |
| `trending` | Trending topics by region | `xmaster trending --region US` |

### Social Graph

| Command | What it does | Example |
|---------|-------------|---------|
| `follow` | Follow a user | `xmaster follow elonmusk` |
| `unfollow` | Unfollow a user | `xmaster unfollow elonmusk` |
| `followers` | List followers | `xmaster followers elonmusk -c 50` |
| `following` | List who a user follows | `xmaster following elonmusk -c 50` |

### Direct Messages

| Command | What it does | Example |
|---------|-------------|---------|
| `dm send` | Send a DM | `xmaster dm send alice "Hey!"` |
| `dm inbox` | View DM inbox | `xmaster dm inbox -c 20` |
| `dm thread` | View a DM conversation | `xmaster dm thread CONV_ID` |

### Scheduling

| Command | What it does | Example |
|---------|-------------|---------|
| `schedule add` | Schedule a post for later | `xmaster schedule add "text" --at "2026-03-24 09:00"` |
| `schedule add --at auto` | Auto-pick best posting time | `xmaster schedule add "text" --at auto` |
| `schedule list` | List scheduled posts | `xmaster schedule list --status pending` |
| `schedule cancel` | Cancel a scheduled post | `xmaster schedule cancel sched_abc123` |
| `schedule fire` | Execute due posts (for cron) | `xmaster schedule fire` |
| `schedule setup` | Install launchd auto-scheduler | `xmaster schedule setup` |

Posts are stored in local SQLite. No X Ads API needed. The launchd daemon fires every 5 minutes on macOS. Use `--at auto` to pick the best time from your engagement history.

### Bookmark Intelligence

| Command | What it does | Example |
|---------|-------------|---------|
| `bookmarks list` | List recent bookmarks | `xmaster bookmarks list -c 20` |
| `bookmarks sync` | Archive bookmarks locally (survives deletions) | `xmaster bookmarks sync -c 200` |
| `bookmarks search` | Search your archive | `xmaster bookmarks search "longevity"` |
| `bookmarks export` | Export as markdown | `xmaster bookmarks export -o bookmarks.md` |
| `bookmarks digest` | Weekly summary | `xmaster bookmarks digest -d 7` |

`bookmarks sync` archives content in SQLite. If the original tweet gets deleted, your copy survives.

### Engagement Intelligence

| Command | What it does | Example |
|---------|-------------|---------|
| `engage recommend` | Find high-ROI reply targets | `xmaster engage recommend --topic "AI" -c 10` |
| `engage feed` | Fresh posts from large accounts | `xmaster engage feed "AI agents" --min-followers 5000` |
| `engage watchlist add` | Track accounts without following | `xmaster engage watchlist add elonmusk` |
| `engage watchlist list` | List watched accounts | `xmaster engage watchlist list` |

The opportunity scorer ranks targets by reciprocity, reply ROI, size fit (adaptive to your follower count), topicality, and freshness.

### Pre-Flight Analysis

| Command | What it does | Example |
|---------|-------------|---------|
| `analyze` | Score a post before publishing | `xmaster analyze "your text" --goal replies` |
| `suggest best-time` | Best posting time from history | `xmaster suggest best-time` |
| `suggest next-post` | Cannibalization guard | `xmaster suggest next-post` |
| `report daily` | Daily performance digest | `xmaster report daily` |
| `report weekly` | Weekly performance digest | `xmaster report weekly` |
| `track run` | Snapshot recent post metrics | `xmaster track run` |
| `track followers` | Track follower changes | `xmaster track followers` |
| `track growth` | Follower growth history | `xmaster track growth -d 30` |
| `inspire` | Browse your discovered posts library | `xmaster inspire --topic "longevity"` |
| `inspire --long` | Surface long-form / Article-candidate exemplars (>=500 chars) | `xmaster inspire --long --topic "biotech"` |

`analyze` estimates 9 proxy signals aligned with the 2026 X algorithm and scores per goal (replies, quotes, shares, follows, impressions). For drafts above 500 chars it also runs **long-form heuristics** (preview-card hook on the first 280 chars, scannability, payoff density, dwell sweet-spot 500–2000 chars) — informed by the Jan 2026 $1M Article Contest results. Run `xmaster agent-info` for the full long-form pattern set including timing, structure, and a `nanaban` pointer for cover-image generation.

### Lists

| Command | What it does | Example |
|---------|-------------|---------|
| `lists create` | Create a list | `xmaster lists create "AI Builders"` |
| `lists add` | Add user to a list | `xmaster lists add LIST_ID username` |
| `lists timeline` | View a list timeline | `xmaster lists timeline LIST_ID` |
| `lists mine` | View your lists | `xmaster lists mine` |

### Moderation

| Command | What it does | Example |
|---------|-------------|---------|
| `block` / `unblock` | Block or unblock a user | `xmaster block spammer123` |
| `mute` / `unmute` | Mute or unmute a user | `xmaster mute username` |
| `hide-reply` / `unhide-reply` | Hide replies on your posts | `xmaster hide-reply 1234567890` |

### System

| Command | What it does | Example |
|---------|-------------|---------|
| `config show` | View config (keys masked) | `xmaster config show` |
| `config check` | Validate credentials | `xmaster config check` |
| `config web-login` | Capture browser cookies for reply bypass | `xmaster config web-login` |
| `agent-info` | Machine-readable capabilities for AI agents | `xmaster agent-info --json` |
| `rate-limits` | Check API quota status | `xmaster rate-limits` |
| `update` | Self-update from GitHub releases | `xmaster update` |

## For AI Agents

XMaster is built for AI agents from day one. Every command supports `--json` and returns structured envelopes with semantic exit codes.

```bash
# JSON output (auto-enabled when piped)
xmaster --json post "Hello from my agent"
xmaster post "Hello" | jq '.data.id'
```

**Response envelope:**
```json
{
  "version": "1",
  "status": "success",
  "data": { "..." },
  "metadata": {
    "elapsed_ms": 342,
    "provider": "x_api"
  }
}
```

**Exit codes:** 0 = success, 1 = runtime error, 2 = config error, 3 = auth missing, 4 = rate limited.

**Agent discovery:**
```bash
xmaster agent-info --json
# Returns: 64 commands, 18 capabilities, 15 algorithm weights,
# measurement coverage, workflow handoffs, writing style config
```

**Pre-flight analysis:**
```bash
xmaster analyze "your tweet" --goal replies --json
# Returns per-signal scores and per-goal scores (0-100)
```

**Works with:** [Claude Code](https://github.com/anthropics/claude-code), [OpenClaw](https://github.com/openclaw/openclaw), or any agent that can shell out and parse JSON.

## Configuration

Config lives at `~/.config/xmaster/config.toml` on all platforms. Override with `XMASTER_CONFIG_DIR`.

```bash
xmaster config show       # View current config (keys masked)
xmaster config check      # Validate credentials
xmaster config set K V    # Set a value
xmaster config get K      # Read a value
```

**Environment variables** override the config file. Use `XMASTER_` prefix with double underscore for nesting:

```bash
export XMASTER_KEYS__API_KEY=your-api-key
export XMASTER_KEYS__API_SECRET=your-api-secret
export XMASTER_KEYS__ACCESS_TOKEN=your-access-token
export XMASTER_KEYS__ACCESS_TOKEN_SECRET=your-access-token-secret
export XMASTER_KEYS__XAI=your-xai-key
```

## Building from Source

```bash
git clone https://github.com/paperfoot/xmaster-cli
cd xmaster
cargo build --release
# Binary at target/release/xmaster
```

## Contributing

Contributions are welcome. See [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines.

## License

[MIT](LICENSE)

---

<div align="center">

Built by [Boris Djordjevic](https://github.com/longevityboris) at [199 Biotechnologies](https://github.com/199-biotechnologies) | [Paperfoot AI](https://paperfoot.ai)

<br />

**If this is useful to you:**

[![Star this repo](https://img.shields.io/github/stars/paperfoot/xmaster-cli?style=for-the-badge&logo=github&label=%E2%AD%90%20Star%20this%20repo&color=yellow)](https://github.com/paperfoot/xmaster-cli/stargazers)
&nbsp;&nbsp;
[![Follow @longevityboris](https://img.shields.io/badge/Follow_%40longevityboris-000000?style=for-the-badge&logo=x&logoColor=white)](https://x.com/longevityboris)

</div>
