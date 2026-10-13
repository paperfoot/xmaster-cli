<div align="center">

# xmaster

**Let your AI agent use X the way you do: read anything, reply to any thread, post, like and follow through your own browser session. No API fees on everyday commands. One Rust binary.**

<br />

[![Star this repo](https://img.shields.io/github/stars/paperfoot/xmaster-cli?style=for-the-badge&logo=github&label=%E2%AD%90%20Star%20this%20repo&color=yellow)](https://github.com/paperfoot/xmaster-cli/stargazers)
&nbsp;&nbsp;
[![Follow @longevityboris](https://img.shields.io/badge/Follow_%40longevityboris-000000?style=for-the-badge&logo=x&logoColor=white)](https://x.com/longevityboris)

<br />

[![Crates.io](https://img.shields.io/crates/v/xmaster?style=for-the-badge&logo=rust&logoColor=white&label=crates.io)](https://crates.io/crates/xmaster)
[![CI](https://img.shields.io/github/actions/workflow/status/paperfoot/xmaster-cli/ci.yml?branch=main&style=for-the-badge&label=CI)](https://github.com/paperfoot/xmaster-cli/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue?style=for-the-badge)](https://github.com/paperfoot/xmaster-cli/blob/main/LICENSE)
[![Homebrew](https://img.shields.io/badge/Homebrew-available-orange?style=for-the-badge&logo=homebrew&logoColor=white)](https://github.com/paperfoot/homebrew-tap)
[![Built with Rust](https://img.shields.io/badge/Built_with-Rust-dea584?style=for-the-badge&logo=rust&logoColor=white)](https://www.rust-lang.org/)

---

For people who run Claude Code, Codex or Gemini on their X account, and for developers who want X in the shell with JSON out.

[Install](#install) | [Quick start](#quick-start) | [Why xmaster](#why-xmaster) | [Commands](#commands) | [For agents](#for-ai-agents) | [Cost](#cost)

</div>

## Install

```bash
brew install paperfoot/tap/xmaster                                                    # macOS, Linux
cargo install xmaster                                                                 # any platform with Rust
curl -fsSL https://raw.githubusercontent.com/paperfoot/xmaster-cli/main/install.sh | sh   # prebuilt binary
```

## Quick start

```bash
xmaster config web-login                    # copy your X session from Chrome, Firefox, Brave, Chromium or Edge
xmaster config check                        # live, read-only test of the session
xmaster engage feed "your niche"            # fresh posts worth a reply
xmaster reply https://x.com/someone/status/123 "your reply" --dry-run   # check the draft, send nothing
```

Log in to x.com in your browser first. To hand X to your agent, run `xmaster skill install` and ask it in plain words: "reply to the best three posts about longevity from the last hour".

## Why xmaster

- **Replies to any thread.** Since February 2026 the X API only lets you reply to people who mentioned you, and on 20 April 2026 it dropped likes, follows and quote posts from self-serve tiers. xmaster sends these through your logged-in browser session, the way x.com does, and finds X's current request IDs on its own when X rotates them.
- **No API fees on everyday commands.** With a session, reading posts, threads, replies, search, timelines, mentions and profiles costs nothing (0 of 13 everyday commands billed in a live check). Posts, replies, likes and follows go through the session too, so you skip the API's $0.015 per post and $0.20 per post with a link.
- **Made for agents.** Compact JSON when piped, one error format with `retryable` and `retry_after_secs`, exit codes that say what to do next, and an `agent-info` summary under 6 KB.
- **Judgment built in (optional).** With a [JEV](https://typesafe.ai) key, xmaster picks the posts worth a reply out of a feed, sorts your inbox into reply, like, ignore and spam, and flags drafts that read as generic or AI-written. On 110 real posts, its picks ranked good reply targets at AUC 0.93, against 0.59 for the size-and-freshness sort it replaced.
- **Ranking facts from X's own code.** Draft checks and `agent-info --tips` use the weights X publishes in [xai-org/x-algorithm](https://github.com/xai-org/x-algorithm): copy-link share 20, reply 5, like 0.6, report −234.
- **Paced writes.** Daily limits and gaps between actions keep agent activity within normal use.

### How it compares

| | xmaster | X API, X MCP, xmcp | X CLIs built on API keys |
|---|---|---|---|
| Reply to any thread | Yes, through your session | Only to people who mentioned you | Same as the API |
| Like, follow, quote | Yes | Removed from self-serve tiers | Same as the API |
| Cost of a post | $0 through your session | $0.015, or $0.20 with a link | Same as the API |
| Reads (posts, threads, search) | $0 through your session or FxTwitter | Billed per read | Billed per read |
| Agent interface | JSON envelope, exit codes, 6 KB `agent-info`, agent skill | MCP tool schemas or raw JSON | Varies |
| Draft checks before sending | Rules from X's ranking code, plus JEV | None | Varies |

## How it works

```
               your agent / your shell
                         |
                      xmaster  ── draft checks, JEV, pacing, local SQLite
                         |
   ┌──────────────┬──────┴───────┬──────────────┬─────────────┐
 web session     FxTwitter      X API v2        xAI          JEV
 (your cookies)  (no login)     (paid fallback) (search-ai)  (judgments)
 reads + writes  post reads     polls, DMs,
 free            free           lists, no-session
```

Every command tries the free route first and falls back to the API only when it has to. `post --dry-run` shows which route a post will take and what it would cost.

## Commands

Post IDs and URLs work anywhere a post is expected; handles, `@handles` and profile URLs anywhere a user is.

| Area | Commands |
|---|---|
| Read | `read`, `conversation` (parent chain + top replies), `replies`, `search`, `timeline`, `mentions`, `user`, `me`, `metrics` (up to 100 posts per call) |
| Write | `post`, `reply`, `quote`, `thread`, `delete`, `like`/`unlike`, `repost`/`unrepost`, `bookmark`/`unbookmark`, `follow`/`unfollow` |
| Engage | `engage feed` (posts worth a reply, in about 3 s, free), `engage inbox` (triage replies and quotes on your post), `engage recommend`, `engage swarm`, `engage hot-targets`, `engage watchlist` |
| Check | `analyze` (score a draft), `post --dry-run`, `suggest best-time`, `suggest next-post` |
| Track | `track run` (snapshots, reply-backs), `track followers`, `track growth`, `report daily`, `report weekly`, `inspire` |
| Schedule | `schedule add --at auto`, `schedule list`, `schedule fire`, `schedule setup` (launchd) |
| Articles | `article preview`, `article draft`, `article publish` |
| More | `bookmarks` (local archive that keeps deleted posts), `lists`, `dm`, `likers`, `reposters`, `quotes`, `users`, `followers`, `following`, `block`, `mute`, `hide-reply`, `search-ai`, `trending` |
| System | `config`, `agent-info`, `skill`, `rails`, `update` |

```bash
xmaster post "Shipped v2 today" --media demo.mp4
xmaster quote 1234567890 "The key number here is 3x, not the headline"
xmaster thread "First post" "Second post" "Third post"
xmaster timeline --user someone --sort impressions -c 50
xmaster search "longevity min_likes:100" -c 20
xmaster metrics 111 222 333
```

`xmaster <command> --help` lists every flag.

## For AI agents

```bash
xmaster skill install        # installs the skill for Claude Code, Codex, Gemini CLI and ~/.agents
xmaster agent-info           # commands, exit codes, cost model, JEV: under 6 KB of JSON
xmaster agent-info --tips    # ranking weights and strategy, by topic: algorithm, replies, posting, growth, metrics, avoid
```

A typical reply loop:

```bash
xmaster engage feed                          # best 5 targets, each with a reason (JEV)
xmaster conversation <id>                    # read the thread before writing
xmaster reply <id> "draft" --dry-run         # preflight, JEV flags, route, cost
xmaster reply <id> "final text"
xmaster engage inbox <your post id>          # later: who needs an answer, what to like
```

Output is JSON when piped (or with `--json`; `--pretty` indents it):

```json
{"version":"1","status":"success","data":{"id":"1234567890","url":"https://x.com/i/status/1234567890","text":"Shipped v2 today","undo":"xmaster delete 1234567890","via":"web"}}
```

Errors go to stderr in the same shape:

```json
{"version":"1","status":"error","error":{"code":"rate_limited","message":"Rate limited by x; retry in 60s","suggestion":"Wait 60s, then retry. Limits: `xmaster rate-limits`","retryable":true,"retry_after_secs":60}}
```

| Exit code | Meaning | Agent should |
|---|---|---|
| 0 | success | continue |
| 1 | runtime error (network, X server, local DB, media) | retry only if `retryable`, after `retry_after_secs` |
| 2 | setup: missing or rejected credentials, bad config, no API credits | follow `suggestion`; don't retry |
| 3 | bad input: invalid arguments, unknown post or user, denied command | fix the arguments |
| 4 | rate limited | wait `retry_after_secs`, then retry |
| 5 | blocked by X (duplicate text, reply limits, protected account) | don't retry unchanged |

Guard rails for unattended agents: `XMASTER_READ_ONLY=1` blocks every write to X (dry runs still work), and `XMASTER_ALLOW_COMMANDS` / `XMASTER_DENY_COMMANDS` limit which commands run.

## Cost

| Route | What goes through it | Price |
|---|---|---|
| Web session | reads (posts, threads, replies, search, timelines, mentions, profiles) and writes (posts, replies, quotes, likes, reposts, bookmarks, follows, deletes, mutes, blocks) | free |
| FxTwitter | post reads and profiles without a login | free |
| X API v2 | fallback without a session; polls, DMs, lists, bookmarks sync, likers, reposters, quote lists, follower lists, your own private metrics | pay-per-use: $0.015 per post, $0.20 with a link |
| xAI | `search-ai`, `trending` | your xAI key |
| JEV | feed picks, inbox triage, draft flags | your JEV key; optional |

API keys are optional once the web session works. Add them for polls, DMs and lists: `xmaster config guide` walks through it.

## Configuration

Config lives at `~/.config/xmaster/config.toml` (override the folder with `XMASTER_CONFIG_DIR`). Environment variables override it: `XMASTER_KEYS__API_KEY`, `XMASTER_SETTINGS__WEB_WRITES`, and so on.

```bash
xmaster config set account.bio "What you do, in one line"   # JEV and engage feed judge fit against it
xmaster config set niche.topics "longevity,AI agents"         # default topics for engage feed
xmaster config set style.voice "short, concrete, no hype"     # voice for drafts
xmaster config set account.premium true                       # 25,000-character posts
xmaster config set settings.web_writes auto                   # always (default), auto, never
xmaster config set settings.web_reads false                   # read through the API instead
xmaster config set keys.jev YOUR_KEY                          # JEV judgments
xmaster config set keys.xai YOUR_KEY                          # search-ai, trending
xmaster config show                                           # everything, keys masked
```

## Build from source

```bash
git clone https://github.com/paperfoot/xmaster-cli
cd xmaster-cli
cargo build --release        # binary at target/release/xmaster
```

## Contributing

Issues and pull requests are welcome. See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

[MIT](LICENSE)

---

<div align="center">

**If xmaster saves you time, a star helps other people find it.**

[![Star this repo](https://img.shields.io/github/stars/paperfoot/xmaster-cli?style=for-the-badge&logo=github&label=%E2%AD%90%20Star%20this%20repo&color=yellow)](https://github.com/paperfoot/xmaster-cli/stargazers)
&nbsp;&nbsp;
[![Follow @longevityboris](https://img.shields.io/badge/Follow_%40longevityboris-000000?style=for-the-badge&logo=x&logoColor=white)](https://x.com/longevityboris)

Built by [Boris Djordjevic](https://github.com/longevityboris) at [Paperfoot AI](https://paperfoot.com) | [borisdjordjevic.com](https://borisdjordjevic.com)

</div>
