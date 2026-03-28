// ──────────────────────────────────────────────────────────
//  How the X Algorithm Works and How to Grow
//  Based on xai-org/x-algorithm (January 2026)
// ──────────────────────────────────────────────────────────

#set page(
  paper: "a4",
  margin: (top: 2.8cm, bottom: 2.8cm, left: 2.5cm, right: 2.5cm),
  numbering: "1",
  number-align: center,
)

#set text(font: "New Computer Modern", size: 10pt, lang: "en")
#set par(leading: 0.65em, first-line-indent: 1.2em, justify: true)
#set heading(numbering: "1.1")

#show heading.where(level: 1): it => {
  v(1em)
  set text(12pt, weight: "bold")
  set par(first-line-indent: 0em)
  block[#counter(heading).display() #it.body]
  v(0.5em)
}

#show heading.where(level: 2): it => {
  v(0.8em)
  set text(10.5pt, weight: "bold")
  set par(first-line-indent: 0em)
  block[#counter(heading).display() #it.body]
  v(0.3em)
}

#show heading.where(level: 3): it => {
  v(0.5em)
  set text(10pt, weight: "bold", style: "italic")
  set par(first-line-indent: 0em)
  block[#it.body]
  v(0.2em)
}

#show raw.where(block: true): it => {
  set text(8.5pt, font: "Menlo")
  block(width: 100%, fill: luma(248), stroke: 0.5pt + luma(200), radius: 2pt, inset: 10pt, it)
}

#show raw.where(block: false): it => {
  set text(9pt, font: "Menlo")
  box(fill: luma(245), outset: (x: 2pt, y: 2pt), radius: 2pt, it)
}

#show figure.caption: it => { set text(9pt); it }

#let code-tag = box(fill: luma(230), stroke: 0.5pt + luma(180), outset: (x: 2pt, y: 1.5pt), radius: 2pt)[#text(7.5pt, weight: "bold")[CODE]]
#let emp-tag = box(fill: luma(230), stroke: 0.5pt + luma(180), outset: (x: 2pt, y: 1.5pt), radius: 2pt)[#text(7.5pt, weight: "bold")[EMP.]]
#let inf-tag = box(fill: luma(230), stroke: 0.5pt + luma(180), outset: (x: 2pt, y: 1.5pt), radius: 2pt)[#text(7.5pt, weight: "bold")[INF.]]

// ════════════════════════════════════════════
//  TITLE
// ════════════════════════════════════════════

#set par(first-line-indent: 0em)

#align(center)[
  #v(0.5cm)
  #text(16pt, weight: "bold")[
    How the X Algorithm Works and How to Grow:\
    A Source Code Analysis
  ]
  #v(0.6cm)
  #text(10.5pt)[Boris Djordjevic]
  #v(0.15cm)
  #text(9pt, fill: luma(100))[199 Biotechnologies · March 2026]
  #v(0.8cm)
]

#block(width: 100%, inset: (x: 2em))[
  #set text(9.5pt)
  #set par(first-line-indent: 0em)
  #text(weight: "bold")[Abstract.] #h(0.5em)
  X open-sourced its recommendation algorithm in January 2026. This paper explains how it works and what it means for anyone trying to grow an account, particularly from a small or dormant starting point. The system uses a Grok-based transformer that scores every post on 19 engagement signals---15 positive and 4 negative---learned entirely from user behaviour, with no hand-engineered features. We explain the scoring pipeline, deduce the relative importance of each signal, and derive a concrete growth strategy grounded in the source code. Every claim is tagged by evidence source: source code (#code-tag), published empirical research (#emp-tag), or first-principles inference (#inf-tag).
]

#v(0.5cm)
#line(length: 100%, stroke: 0.5pt + luma(180))
#v(0.3cm)

#set par(first-line-indent: 1.2em)

// ════════════════════════════════════════════
//  1. HOW THE ALGORITHM WORKS
// ════════════════════════════════════════════

= How the Algorithm Works

When you open your "For You" feed, the system executes a pipeline that narrows millions of posts down to a ranked list of roughly 50. The entire process takes about 200 milliseconds.

== Where posts come from

Posts are sourced from two places, in parallel #code-tag:

- *In-network (Thunder).* Posts from accounts you follow, served from an in-memory store with sub-millisecond lookups.
- *Out-of-network (Phoenix Retrieval).* Posts discovered from a global corpus using a machine learning model that matches your engagement history to candidate posts via dot-product similarity.

Out-of-network is the discovery mechanism. It is how posts reach people who do not follow you. It is also penalised by a multiplicative discount factor (`OON_WEIGHT_FACTOR` < 1.0), meaning in-network content has a structural advantage #code-tag.

== How posts are scored

Every candidate post is scored by a Grok-based transformer model that predicts the probability of 19 different user actions---like, reply, repost, share, block, report, and so on. These predicted probabilities are combined into a single score using a weighted sum #code-tag:

$ "score" = sum_(i=1)^19 w_i dot P(a_i) $

The weight constants ($w_i$) determine how much each action matters. They are not published. We estimate them in Section 2.

== What the model sees about you

The transformer takes your last *128 engagements* as input---every like, reply, repost, and share you made, along with the posts and authors involved #code-tag. This engagement history is how the model understands your interests. It is also how it predicts what you would engage with next.

A dormant account has no engagement history. The system literally cannot score posts for you or about you until you generate data (see Section 4).

== Filtering

Before scoring, 10 filters remove ineligible content---duplicates, old posts, content from blocked or muted accounts, muted keywords, and previously seen posts. After scoring, additional safety filters remove spam, violence, and policy violations #code-tag.

Out-of-network content faces a stricter safety threshold than in-network content #code-tag.

== Author diversity

If the same author appears multiple times in a feed, each successive appearance is penalised by an exponential decay function #code-tag:

$ "multiplier"(n) = (1 - "floor") dot "decay"^n + "floor" $

This means posting 5 times in an hour is counterproductive---your 5th post receives a fraction of its natural score.

// ════════════════════════════════════════════
//  2. WHAT THE ALGORITHM REWARDS
// ════════════════════════════════════════════

= What the Algorithm Rewards (and Punishes)

The scoring formula uses exactly 19 signals. The weight constants are hidden, but we can estimate their relative importance from code structure, platform economics, and empirical research.

== The 19 signals, ranked by estimated impact

#figure(
  table(
    columns: (auto, 1fr, auto, auto),
    stroke: 0.5pt + luma(180),
    inset: 6pt,
    table.header(
      [],
      text(weight: "bold")[Signal],
      text(weight: "bold")[Est. weight],
      text(weight: "bold")[Source],
    ),
    table.hline(),
    text(weight: "bold")[1], [*Follow author* --- user follows you from this post], [$tilde$30$times$], inf-tag,
    text(weight: "bold")[2], [*Share via DM* --- user sends your post in a direct message], [$tilde$25$times$], inf-tag,
    text(weight: "bold")[3], [*Reply* --- user replies to your post], [$tilde$20$times$], emp-tag,
    text(weight: "bold")[4], [*Share via copy link* --- user copies the URL to share elsewhere], [$tilde$20$times$], inf-tag,
    text(weight: "bold")[5], [*Quote tweet* --- user quotes your post with commentary], [$tilde$18$times$], emp-tag,
    text(weight: "bold")[6], [*Profile click* --- user clicks your name or avatar], [$tilde$12$times$], emp-tag,
    text(weight: "bold")[7], [*Click* --- user clicks into the full conversation], [$tilde$10$times$], emp-tag,
    text(weight: "bold")[8], [*Share (generic)* --- user opens the share menu], [$tilde$10$times$], inf-tag,
    text(weight: "bold")[9], [*Dwell* --- user pauses on your post (binary)], [$tilde$8$times$], emp-tag,
    text(weight: "bold")[10], [*Video quality view* --- user watches your video past a threshold], [$tilde$3$times$], code-tag,
    text(weight: "bold")[11], [*Retweet* --- user reposts without commentary], [$tilde$3$times$], emp-tag,
    text(weight: "bold")[12], [*Photo expand* --- user taps to see full image], [$tilde$2$times$], inf-tag,
    text(weight: "bold")[13], [*Favourite (like)* --- baseline], [1$times$], emp-tag,
    text(weight: "bold")[14], [*Dwell time* --- how long the user pauses (continuous, in seconds)], [$tilde$0.1/s], code-tag,
    text(weight: "bold")[15], [*Quoted click* --- user clicks into the original from a quote], [$tilde$4$times$], inf-tag,
    table.hline(),
    text(weight: "bold")[16], [*Not interested* --- user taps "show less"], [$tilde minus$20$times$], inf-tag,
    text(weight: "bold")[17], [*Mute author* --- user mutes you], [$tilde minus$40$times$], inf-tag,
    text(weight: "bold")[18], [*Block author* --- user blocks you], [$tilde minus$74$times$], inf-tag,
    text(weight: "bold")[19], [*Report* --- user reports your post], [$tilde minus$369$times$], inf-tag,
  ),
  caption: [All 19 scoring signals from `weighted_scorer.rs`, ranked by estimated relative weight. Favourite (like) = 1#sym.times baseline. Signals 1--15 are positive; 16--19 are negative. True weight values are in the unpublished `params.rs` module.],
) <tab:signals>

== Key observations

*Likes are the weakest positive signal.* Most growth advice focuses on likes. The algorithm barely values them. A like is the lowest-effort action a user can take, and its weight reflects that.

*Shares are probably the most underrated signals.* DM shares, copy-link shares, and generic shares are three separate dedicated signals---new in 2026. Sending a post via DM is the highest-conviction action a user can take (personally vouching to someone they know). The algorithm treats it accordingly #inf-tag.

*Follows-from-post are the ultimate signal.* If your content causes someone to follow you, that post receives the highest positive weighting. This rewards genuinely novel or valuable content from accounts people have not seen before #inf-tag.

*Negative signals are predictive, not reactive.* The Grok transformer predicts the _probability_ that a user would block, mute, or report your content---and penalises your post _before anyone acts_. Content that the model expects to provoke negative reactions is suppressed pre-emptively #code-tag.

*Negative compression is asymmetric.* Positive scores scale linearly. Negative scores are compressed into a bounded band near zero. This means even moderate negative predictions can kill a post, while positive signals stack without limit #code-tag.

*Bookmarks are not a signal.* They are not among the 19 signals in the scorer. The widely circulated claim that bookmarks carry high weight is incorrect #code-tag.

// ════════════════════════════════════════════
//  3. CONTENT STRATEGY
// ════════════════════════════════════════════

= What to Post (and How)

== Text

Text posts have the highest average engagement rate on X at 0.48%, compared to 0.41% for images and video #emp-tag. They require no production overhead, enabling higher posting frequency with quality. The scoring formula has no format-specific bonus for text---it simply tends to generate more replies and dwell time #inf-tag.

*Structure for maximum impact:*
- Open with a strong first line (visible without expanding).
- Use line breaks for scannability---this increases dwell time.
- End with a question or provocative claim to drive replies.

== Images

The `photo_expand_score` signal fires when a user taps to see the full image #code-tag. Design images that demand expansion:
- Infographics with text too small to read in the feed.
- Charts and data visualisations from papers or research.
- Screenshots that are partially cropped to force a tap.

Native image uploads see up to 40% more engagement than linked images #emp-tag.

== Threads

Threads maximise the continuous `dwell_time` signal---the only non-probability input to the scorer, measured in seconds #code-tag. A 5-tweet thread where someone reads all 5 generates substantially more dwell signal than a single tweet. Threads average 3#sym.times more engagement than single tweets #emp-tag.

== Video

Videos must exceed a minimum duration threshold (`MIN_VIDEO_DURATION_MS`, value not published) to qualify for the `vqv_score` (video quality view) signal. Videos shorter than this threshold receive zero contribution from VQV #code-tag. Aim for 15--60 seconds minimum.

== Posting frequency and spacing

The author diversity decay means each successive post from you in the same feed session gets `decay`#super[_n_] of its natural score. *Space posts at least 2 hours apart* to avoid cannibalisation #code-tag. A rhythm of 3--5 posts per day, well spaced, outperforms 10 posts dumped in quick succession.

== What not to post

#figure(
  table(
    columns: (1fr, 1fr, auto),
    stroke: 0.5pt + luma(180),
    inset: 6pt,
    table.header(
      text(weight: "bold")[Behaviour],
      text(weight: "bold")[Why it hurts],
      text(weight: "bold")[Source],
    ),
    [Off-topic content], [Elevates P(not interested) prediction], inf-tag,
    [Engagement bait ("Like if you agree!")], [Trained users ignore or mute; elevates P(mute)], inf-tag,
    [Combative or aggressive tone], [Grok predicts higher P(block), P(mute) even with high engagement], emp-tag,
    [Spam-like patterns (copy-pasted replies)], [Triggers P(report); may activate safety filters], emp-tag,
    [5+ hashtags], [Associated with spam; 40% engagement reduction observed], emp-tag,
    [Posting 5+ times in 1 hour], [Author diversity decay: 5th post gets decay#super[4] of score], code-tag,
  ),
  caption: [Behaviours that trigger negative signals or scoring penalties.],
)

// ════════════════════════════════════════════
//  4. GROWING FROM A SMALL OR DORMANT ACCOUNT
// ════════════════════════════════════════════

= Growing from a Small or Dormant Account

This section addresses a specific scenario: an account with fewer than 150 followers, dormant or low-activity, now trying to grow.

== The cold start problem

The Grok transformer requires engagement history as input. The model takes your last 128 interactions and uses them to understand your interests and predict what you would engage with. If your engagement history is empty, the query hydrator returns an error and the scoring pipeline short-circuits #code-tag:

```rust
if thrift_user_actions.is_empty() {
    return Err(format!("No user actions found for user {}", user_id));
}
```

*This means step zero is using X actively*---liking, replying, reposting---for at least 1--2 weeks before expecting any organic reach from original content.

== Phase 1: Build your engagement history (Days 1--14)

*Daily time: 45--60 minutes.*

#set par(first-line-indent: 0em)

*Morning (20 min):*
- Like 20--30 posts in your niche. Each like enters `history_actions` and teaches the retrieval model your topics #code-tag.
- Reply to 5--10 posts from accounts with 1K--50K followers, targeting posts under 30 minutes old #emp-tag.
- Quote 2--3 posts with added context. Quote tweets are a separate signal from retweets #code-tag.

*Midday (15 min):*
- Post 1--2 original tweets (text-only at this stage).
- Reply to every response you receive within 30 minutes.

*Evening (10 min):*
- DM 1--2 posts to people who would genuinely value them. This fires the `share_via_dm` signal---separate, high-value, and almost universally ignored by growth practitioners #code-tag.
- Follow 5--10 relevant accounts. Your following list determines what Thunder serves as in-network candidates, shaping your own engagement history #code-tag.

#set par(first-line-indent: 1.2em)

== Phase 2: Establish a rhythm (Days 15--60)

With engagement history populated, the transformer can score your content.

- Increase to 3--5 original posts per day, spaced $gt.eq$ 2 hours apart #code-tag.
- Maintain a 70/30 split: 70% engaging with others, 30% original content #emp-tag.
- Add 1 thread per week (5--7 tweets) for dwell time #code-tag.
- Add 1 image post per day designed for tap-to-expand #code-tag.

== Phase 3: Compound (Days 60--180)

- 5--7 posts per day if quality is maintained.
- 1--2 video posts per week exceeding the minimum duration for VQV scoring #code-tag.
- Actively seek quote-post opportunities on larger accounts.
- Share valuable posts via DM consistently---the most underrated lever in the algorithm.

== The reply strategy in detail

Replies are estimated at $tilde$20#sym.times the weight of a like. They are the single highest-impact action available to a small account for three reasons:

+ *Algorithmic weight.* A reply fires `ServerTweetReply`, one of the highest-weighted positive signals.
+ *Visibility.* Your reply appears in the thread below the original post, exposing you to the original author's audience.
+ *Profile clicks.* Readers who find your reply valuable click your profile, firing `profile_click_score` ($tilde$12#sym.times) for your other content.

*What makes a good reply:* Add information, cite data, offer a contrarian perspective, or share relevant experience. Never write "great post" or emoji-only responses---these generate zero engagement and teach the model that your content is low-value #inf-tag.

*Target selection:* Accounts with 1K--50K followers in your niche. Large enough to have active threads, small enough to notice you. Avoid accounts with 500K+ followers---your reply will be buried #emp-tag.

// ════════════════════════════════════════════
//  5. PREMIUM
// ════════════════════════════════════════════

= Premium Subscription

The Premium boost is not present in the 2026 recommendation source code #code-tag. It likely operates at a different layer of the stack. However, empirical data consistently reports substantial reach advantages #emp-tag:

- Premium accounts average $tilde$600 impressions per post vs. significantly fewer for free accounts ($tilde$10#sym.times advantage).
- Premium+ accounts average $tilde$1,550 impressions per post.

*When to subscribe:* Not on Day 1. The boost is multiplicative---it amplifies your existing score. If your engagement history is empty and your content generates no engagement, multiplying zero is still zero. Subscribe at Day 21--30, once you are posting consistently and receiving measurable engagement #inf-tag.

Choose Premium (\$8/month) initially, not Premium+ (\$16/month). The incremental benefit matters more at higher follower counts where the reach differential compounds.

// ════════════════════════════════════════════
//  6. NEGATIVE SIGNALS
// ════════════════════════════════════════════

= How the Algorithm Punishes Content

== Predictive, not reactive

The four negative signals---not interested, mute, block, report---are _predictions_. The Grok transformer estimates the probability that a user would take these actions and penalises your post before anyone actually does #code-tag.

Your historical blocks and mutes become training data. The model generalises: if users who engage with your niche topic tend to mute your posts, the model will suppress your content for the entire niche-interested audience segment #inf-tag.

== Asymmetric compression

Positive scores scale linearly. Negative scores are compressed into a bounded band near zero by the `offset_score()` function #code-tag. This means:

- A post with strong positive signals and a few negative signals survives---the positive dominates.
- A post with even moderate negative signals and weak positive signals enters compression and is effectively killed.
- There is a floor---mass-reporting cannot drive a score to negative infinity. But the floor is near zero, which is functionally invisible.

== The penalty hierarchy

#figure(
  table(
    columns: (auto, 1fr, auto),
    stroke: 0.5pt + luma(180),
    inset: 6pt,
    table.header(
      text(weight: "bold")[Rank],
      text(weight: "bold")[Penalty],
      text(weight: "bold")[Recovery],
    ),
    [1], [Safety filter drop (spam, violence, policy)], [Irreversible for that post],
    [2], [Blocked/muted by viewer (hard filter---removed before scoring)], [Unblock/unmute by viewer],
    [3], [High P(report) prediction], [Model must relearn from positive signals],
    [4], [High P(block) prediction], [Same],
    [5], [High P(mute) prediction], [Same],
    [6], [High P(not interested) prediction], [Same],
    [7], [Out-of-network discount factor], [Viewer follows you],
    [8], [Author diversity decay], [Resets each feed session],
    [9], [Rate-limit shadowban (>100 likes/hr, follow cycling)], [48 hours to 3 months],
  ),
  caption: [Penalty mechanisms ranked by severity. Predictions (ranks 3--6) are persistent and require sustained positive engagement to reverse.],
)

// ════════════════════════════════════════════
//  7. MILESTONES
// ════════════════════════════════════════════

= Expected Growth Timeline

#figure(
  table(
    columns: (1fr, auto, 1fr),
    stroke: 0.5pt + luma(180),
    inset: 6pt,
    table.header(
      text(weight: "bold")[Milestone],
      text(weight: "bold")[Timeline],
      text(weight: "bold")[What it means],
    ),
    [Engagement history populated], [Day 7--14], [The algorithm can now score your content],
    [For You feed shows your niche], [Day 7--10], [Retrieval model has learned your interests],
    [First meaningful reply exchange], [Day 2--3], [Engagement edges forming],
    [Subscribe to Premium], [Day 21--30], [$tilde$10#sym.times reach amplification],
    [250 followers], [Week 3--4], [Meaningful in-network audience],
    [Post exceeding 1,000 impressions], [Week 3--4], [Out-of-network retrieval working],
    [500 followers], [Month 2--3], [Compounding growth begins],
    [First viral post (10,000+ impressions)], [Month 2--3], [You are established in the retrieval model's embedding space],
    [1,000 followers], [Month 4--6], [Self-sustaining growth],
  ),
  caption: [Expected milestones for a dormant account ($tilde$100 followers) following the described strategy.],
)

// ════════════════════════════════════════════
//  8. DAILY CHECKLIST
// ════════════════════════════════════════════

= Daily Checklist

#block(width: 100%, stroke: 0.5pt + luma(180), radius: 3pt, inset: 14pt)[
  #set par(first-line-indent: 0em)
  #set text(9.5pt)

  #text(weight: "bold")[Morning (20 min)]
  - Like 20--30 niche posts (builds engagement history)
  - Reply to 5--10 posts from larger accounts (target posts < 30 min old)
  - Quote 2--3 valuable posts with added context

  #v(6pt)
  #text(weight: "bold")[Midday (20 min)]
  - Post 1--2 original posts (spaced $gt.eq$ 2 hours from each other)
  - Reply to all replies on your content within 30 minutes
  - DM 1 great post to someone who would value it

  #v(6pt)
  #text(weight: "bold")[Evening (10 min)]
  - Post 1 more original post or start a thread segment
  - Check metrics: which posts generated profile clicks? Double down on that format.
  - Follow 3--5 new relevant accounts

  #v(6pt)
  #text(weight: "bold")[Weekly]
  - 1 thread (5--7 tweets)
  - 1 image post designed for tap-to-expand
  - Review: which content types drove the most engagement?
  - Unfollow accounts that pollute your engagement history with off-topic content
]

// ════════════════════════════════════════════
//  REFERENCES
// ════════════════════════════════════════════

#heading(numbering: none)[References]

#set par(first-line-indent: 0em)
#set text(9pt)

#block(inset: (left: 1.5em))[
  + xAI Corp. _x-algorithm_. GitHub, Apache 2.0, January 20, 2026. `github.com/xai-org/x-algorithm`

  + Buffer Research. "Does X Premium Really Boost Your Reach? We Analyzed 18.8 Million Posts." 2025.

  + PostEverywhere. "How the X/Twitter Algorithm Works in 2026 (From the Source Code)." 2026.

  + Tweet Archivist. "Complete Technical Breakdown: How the X Algorithm Works." 2025--2026.

  + Social Media Today. "X Reveals Key Signals for Post Reach." 2025.

  + Pixelscan. "Twitter Shadowban: Causes, Detection & Fixes (2026 Guide)." 2026.

  + Circleboom. "The Hidden X Algorithm: TweepCred, Shadow Hierarchy, and Dwell Time." 2025.

  + Tomorrow's Publisher. "X Softens Stance on External Links." October 2025.

  + TechCrunch. "X open sources its algorithm while facing a transparency fine." January 2026.
]
