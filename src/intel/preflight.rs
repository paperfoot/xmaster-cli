use regex::Regex;
use serde::Serialize;
use std::sync::OnceLock;

/// Post quality lint. Rules cite xai-org/x-algorithm (home-mixer/params/param.rs,
/// synced 2026-10-12; vm-ranker; grox; visibility-filtering). The ranking
/// weights are public: score = Σ weight × the viewer's predicted probability.

// ---------------------------------------------------------------------------
// Context passed into analyze()
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize)]
pub enum PostMode {
    #[default]
    Standalone,
    Reply,
    Quote,
}

#[derive(Debug, Clone, Serialize)]
pub enum MediaKind {
    Image,
    Video,
    Gif,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct AnalyzeContext {
    pub goal: Option<String>,
    pub mode: Option<PostMode>,
    pub has_media: bool,
    pub media_kind: Option<MediaKind>,
    pub has_poll: bool,
    pub target_text: Option<String>,
    pub author_voice: Option<String>,
    /// Whether the user has X Premium (drives char limit: 25k vs 280).
    pub premium: bool,
}

impl AnalyzeContext {
    pub fn goal_str(&self) -> Option<&str> {
        self.goal.as_deref()
    }

    pub fn is_reply(&self) -> bool {
        matches!(self.mode, Some(PostMode::Reply))
    }

    pub fn is_quote(&self) -> bool {
        matches!(self.mode, Some(PostMode::Quote))
    }
}

// ---------------------------------------------------------------------------
// Proxy signal scores
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct ProxyScores {
    pub reply: f32,
    pub quote: f32,
    pub profile_click: f32,
    pub follow_author: f32,
    pub share_via_dm: f32,
    pub share_via_copy_link: f32,
    pub dwell: f32,
    pub media_expand: f32,
    pub negative_risk: f32,
}

#[derive(Debug, Clone, Serialize)]
pub struct GoalScores {
    pub replies: u32,
    pub quotes: u32,
    pub shares: u32,
    pub follows: u32,
    pub impressions: u32,
}

// ---------------------------------------------------------------------------
// PreflightResult
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct PreflightResult {
    pub text: String,
    pub score: u32,
    pub grade: String,
    pub issues: Vec<Issue>,
    pub suggestions: Vec<String>,
    pub features: FeatureVector,
    pub suggested_next_commands: Vec<String>,
    pub proxy_scores: ProxyScores,
    pub goal_scores: GoalScores,
}

#[derive(Debug, Clone, Serialize)]
pub struct Issue {
    pub severity: Severity,
    pub code: String,
    pub message: String,
    pub fix: Option<String>,
    /// UX grouping (v1.7.0): collapses ~20 individual codes into 6 action categories.
    /// JSON consumers can keep using `code`; human output groups by `group`.
    pub group: IssueGroup,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub enum Severity {
    Critical,
    Warning,
    Info,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
pub enum IssueGroup {
    /// First-line / preview-hook strength
    Hook,
    /// Content depth, payoff, POV
    Substance,
    /// Triggers algorithmic suppression
    Safety,
    /// Looks like something you've already posted
    Dedup,
    /// Posting velocity / cadence
    Cadence,
    /// Reply-mode-only
    Reply,
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Severity::Critical => write!(f, "CRITICAL"),
            Severity::Warning => write!(f, "WARNING"),
            Severity::Info => write!(f, "INFO"),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct FeatureVector {
    pub char_count: usize,
    pub word_count: usize,
    pub has_link: bool,
    pub link_position: Option<String>,
    pub has_media: bool,
    pub hashtag_count: usize,
    pub has_question: bool,
    pub has_numbers: bool,
    pub has_cta: bool,
    pub hook_strength: u32,
    pub line_count: usize,
    pub starts_with_i: bool,
    pub content_type_guess: String,
    pub est_dwell_seconds: f64,
    pub sentiment: String,
}

// ---------------------------------------------------------------------------
// Core analysis
// ---------------------------------------------------------------------------

pub fn analyze(text: &str, ctx: &AnalyzeContext) -> PreflightResult {
    let trimmed = text.trim();
    let mut features = extract_features(trimmed);
    let goal = ctx.goal_str();

    if ctx.has_media {
        features.has_media = true;
    }

    let mut issues = Vec::new();
    let mut score: i32 = 70;

    // --- Critical issues (score -= 30) ---
    if trimmed.is_empty() {
        issues.push(Issue {
            severity: Severity::Critical,
            code: "empty_content".into(),
            message: "Tweet is empty or whitespace-only".into(),
            fix: Some("Add tweet text".into()),
            group: IssueGroup::Substance,
        });
        score -= 30;
    }

    let char_limit = if ctx.premium { 25_000 } else { 280 };
    if features.char_count > char_limit {
        issues.push(Issue {
            severity: Severity::Critical,
            code: "over_limit".into(),
            message: format!(
                "Post is {} characters as X counts them (URLs 23, emoji and CJK 2; limit: {})",
                features.char_count, char_limit
            ),
            fix: Some(format!(
                "Remove {} characters",
                features.char_count - char_limit
            )),
            group: IssueGroup::Safety,
        });
        score -= 30;
    }

    // --- Links: no ranking penalty (open_link weighs +0.2, param.rs:348, and
    // Phoenix has no URL feature). The risks are URL reputation, link spam in
    // replies, and the paid API's URL price.
    let urls = find_urls(trimmed);
    let mentions = count_body_mentions(trimmed, ctx.is_reply());
    if let Some(host) = urls.iter().find_map(|u| shortener_host(u)) {
        issues.push(Issue {
            severity: Severity::Critical,
            code: "link_shortener".into(),
            message: format!(
                "{host} is a URL shortener. X scores every hop of the redirect chain: a low-quality hop labels the post SpamHighRecall (hidden from non-followers), and a pinned post with a bad link labels the whole account for 7 days"
            ),
            fix: Some("Use the full destination URL".into()),
            group: IssueGroup::Safety,
        });
        score -= 30;
    }
    if !urls.is_empty() && ctx.is_reply() {
        issues.push(Issue {
            severity: Severity::Critical,
            code: "reply_has_link".into(),
            message: "Link in a reply. To a stranger it reads as link spam: X's reply judge sees the URL, and the reply must also clear URL-reputation rules and Agatha (blocks and reports from non-followers)".into(),
            fix: Some("Drop the link; answer in the reply and let them ask for the source".into()),
            group: IssueGroup::Reply,
        });
        score -= 30;
    } else if !urls.is_empty() && mentions > 0 {
        issues.push(Issue {
            severity: Severity::Warning,
            code: "link_mention_nonfollower".into(),
            message: "Link plus @mention: X's URL-spam rules look for links aimed at accounts; if the mentioned account doesn't follow you it reads as link spam".into(),
            fix: Some("Post the link without the @mention, or mention them without the link".into()),
            group: IssueGroup::Safety,
        });
        score -= 15;
    }
    if !urls.is_empty() {
        if !ctx.is_reply() && residual_text_len(trimmed) < 40 {
            issues.push(Issue {
                severity: Severity::Warning,
                code: "link_bare".into(),
                message: "Mostly a bare link. The ranker scores the post's own text and media, so a link with no description gives it little to work with".into(),
                fix: Some("Say what's behind the link and why it matters in 1-2 lines".into()),
                group: IssueGroup::Substance,
            });
            score -= 10;
        }
        issues.push(Issue {
            severity: Severity::Info,
            code: "link_api_cost".into(),
            message: "Links carry no ranking penalty (open_link +0.2). Cost only: through the paid API a post with a URL is $0.20 instead of $0.015; the web session posts it free".into(),
            fix: None,
            group: IssueGroup::Safety,
        });
    }
    if mentions >= 2 {
        issues.push(Issue {
            severity: Severity::Warning,
            code: "mention_burst".into(),
            message: format!(
                "{mentions} @mentions in the body. Posts with 2+ mentions get X's real-time spam check (PTOS) 10 s after posting"
            ),
            fix: Some("Keep one @mention, or none".into()),
            group: IssueGroup::Safety,
        });
        score -= 10;
    }

    // --- Warning issues (score -= 15) ---
    let first_line = trimmed.lines().next().unwrap_or("");
    let weak_openers = ["I ", "So ", "Just ", "The "];
    if weak_openers.iter().any(|w| first_line.starts_with(w)) {
        issues.push(Issue {
            severity: Severity::Warning,
            code: "weak_hook".into(),
            message: format!(
                "Weak opening — \"{}...\" doesn't grab attention",
                crate::utils::safe_truncate(first_line, 30)
            ),
            fix: Some("Lead with a number, named subject, or status verb. Contest-winning Articles all open with a weighted single line (Kobeissi: 14/30 originals open with BREAKING:; beaverd: $74-billion + named adversary; Koe: second-person directive)".into()),
            group: IssueGroup::Hook,
        });
        score -= 15;
    }

    let lower = trimmed.to_lowercase();

    // --- weak_hook (research-sharpened) ---
    // Even if the opener isn't on the weak-opener list, fire weak_hook if the
    // first line carries no "weight" — no number, no proper noun, no status
    // verb. Contest-winning Articles all open with one of these.
    let status_verbs = [
        "breaking:", "breaking ", "surging", "metastasized", "exposed", "leaked",
        "collapsed", "soaring", "plunging", "obliterated", "demolished", "introducing",
        "announcing", "launching", "shipping", "released",
        // Past-tense forms — "Google announced layoffs" is exactly the kind of
        // weighted lead this check exists to recognize
        "announced", "launched", "shipped", "raised", "acquired", "banned", "approved",
    ];
    let first_line_lower = first_line.to_lowercase();
    let first_line_has_number = first_line.chars().any(|c| c.is_ascii_digit());
    let first_line_has_proper_noun = has_proper_nouns(first_line);
    let first_line_has_status_verb = status_verbs.iter().any(|v| first_line_lower.contains(v));
    let first_line_has_weight =
        first_line_has_number || first_line_has_proper_noun || first_line_has_status_verb;
    // Only fire if not already flagged by the weak-opener check, and the post is substantive
    if !first_line_has_weight
        && !weak_openers.iter().any(|w| first_line.starts_with(w))
        && features.char_count > 60
        && !ctx.is_reply()
    {
        issues.push(Issue {
            severity: Severity::Info,
            code: "weak_hook_no_weight".into(),
            message: "First line carries no number, named subject, or status verb — Articles that win opens carry weight in the first line".into(),
            fix: Some("Rewrite line 1 to lead with a quantity ($83B), a named entity (Deloitte), or a status verb (BREAKING / launched / collapsed)".into()),
            group: IssueGroup::Hook,
        });
        score -= 10;
    }

    if let Some(hit) = engagement_bait(trimmed) {
        issues.push(Issue {
            severity: Severity::Critical,
            code: "engagement_bait".into(),
            message: format!(
                "Engagement bait (\"{hit}\"). X's post classifier labels baiting and engagement farming SpamHighRecall (hidden from non-followers), with no exemption for big accounts"
            ),
            fix: Some("Ask a real question instead. A deliberate comment-to-DM lead magnet pays this cost".into()),
            group: IssueGroup::Safety,
        });
        score -= 30;
    }

    if let Some(insult) = insult_target(trimmed) {
        issues.push(Issue {
            severity: Severity::Warning,
            code: "insult_target".into(),
            message: format!(
                "Insult aimed at a person (\"{insult}\"). The FOSNR_ABUSE_INSULTS label hides the post from non-followers and puts a notice in front of followers"
            ),
            fix: Some("Attack the claim, not the person".into()),
            group: IssueGroup::Safety,
        });
        score -= 15;
    }

    if features.hashtag_count > 2 {
        issues.push(Issue {
            severity: Severity::Warning,
            code: "excessive_hashtags".into(),
            message: format!(
                "{} hashtags. Hashtags carry no ranking weight (Grok assigns topics from the text), and 3+ reads as spam",
                features.hashtag_count
            ),
            fix: Some("Keep to 0-1 hashtags".into()),
            group: IssueGroup::Safety,
        });
        score -= 15;
    }

    if !features.has_numbers && !has_proper_nouns(trimmed) {
        issues.push(Issue {
            severity: Severity::Warning,
            code: "low_specificity".into(),
            message: "No numbers, names, or data — specificity drives engagement".into(),
            fix: Some("Add a concrete number, name, or data point".into()),
            group: IssueGroup::Substance,
        });
        score -= 15;
    }

    // --- reference_only_risk (doublenickk trap) ---
    // High specificity + zero first-person/POV markers + substantive length =
    // pure reference content. Readers SAVE it but don't FOLLOW because there's
    // no persona to follow. doublenickk's algorithm Article: 255K impressions,
    // 1263 bookmarks, only 12.7K followers — bookmark:like 0.58:1 is the
    // signature.
    let pov_markers = [
        " i ", " i'", " i,", " i.", " my ", " me ", " we ", " our ", " i'm",
        " i've", "i think", "i believe", "i'd argue", "in my view", "imo",
        "personally", "from where i sit", "i'd say", "hot take", "unpopular",
    ];
    let trimmed_lower_padded = format!(" {} ", lower);
    let pov_count = pov_markers
        .iter()
        .filter(|m| trimmed_lower_padded.contains(*m))
        .count();
    let has_high_specificity = features.has_numbers && has_proper_nouns(trimmed);
    if has_high_specificity && pov_count == 0 && features.char_count > 200 && !ctx.is_reply() {
        issues.push(Issue {
            severity: Severity::Warning,
            code: "reference_only_risk".into(),
            message: "Data without a POV — reads as reference content. Readers will SAVE it but won't FOLLOW you (the doublenickk trap: 255K impressions, 1263 bookmarks, only 12.7K followers).".into(),
            fix: Some("Add a first-person stance: what's YOUR take on the data? Lead with the claim, then the evidence — not the other way around".into()),
            group: IssueGroup::Substance,
        });
        score -= 10;
    }

    // --- multiple_ctas ---
    // Justin Welsh's rule: pick exactly ONE ask. Multiple CTAs degrade all of
    // them. Count distinct CTA phrases; warn if >1.
    let cta_phrases: &[&str] = &[
        "follow me", "follow @", "follow for", "hit follow", "follow if",
        "subscribe", "sign up", "join the newsletter", "link in bio", "dm me",
        "reply with", "bookmark this", "save this", "share this", "repost ",
        "retweet ", "like if", "rt if",
    ];
    let cta_hit_count = cta_phrases
        .iter()
        .filter(|p| lower.contains(*p))
        .count();
    if cta_hit_count > 1 {
        issues.push(Issue {
            severity: Severity::Warning,
            code: "multiple_ctas".into(),
            message: format!(
                "{} CTAs detected — pick ONE ask per post (multiple CTAs degrade all of them; Justin Welsh: 0→315k followers using one specific cadence-promise close)",
                cta_hit_count
            ),
            fix: Some("Cut all but one CTA. For save-worthy posts use 'Bookmark this for later' (saves outperform follow asks); for personality posts use 'I share [niche] tactics every [day]. Follow @handle'".into()),
            group: IssueGroup::Substance,
        });
        score -= 10;
    }

    if features.char_count < 30
        && !features.has_media
        && !features.has_question
        && !ctx.is_reply()
    {
        issues.push(Issue {
            severity: Severity::Info,
            code: "too_short".into(),
            message: "Very short post — longer content drives more dwell time (a scoring signal)"
                .into(),
            fix: Some("Consider adding depth — the algorithm rewards dwell time".into()),
            group: IssueGroup::Substance,
        });
        score -= 5;
    }

    if trimmed.starts_with('@') {
        issues.push(Issue {
            severity: Severity::Warning,
            code: "starts_with_mention".into(),
            message: "Starting with @mention limits visibility to mutual followers".into(),
            fix: Some("Put a word before the @mention, e.g. \".@user\"".into()),
            group: IssueGroup::Safety,
        });
        score -= 15;
    }

    // --- Info issues (score -= 5) ---
    if !features.has_question {
        issues.push(Issue {
            severity: Severity::Info,
            code: "no_question".into(),
            message: "No question mark — questions drive replies, a top positive scorer term".into(),
            fix: Some("Consider ending with a question to invite discussion".into()),
            group: IssueGroup::Substance,
        });
        score -= 5;
    }

    if features.line_count <= 1 && features.char_count > 100 {
        issues.push(Issue {
            severity: Severity::Info,
            code: "no_formatting".into(),
            message: "Wall of text — line breaks improve readability and stop-rate".into(),
            fix: Some("Break into 2-3 short lines".into()),
            group: IssueGroup::Substance,
        });
        score -= 5;
    }

    if trimmed == lower && trimmed.chars().any(|c| c.is_alphabetic()) {
        issues.push(Issue {
            severity: Severity::Info,
            code: "all_lowercase".into(),
            message: "All lowercase — proper capitalization looks more authoritative".into(),
            fix: None,
            group: IssueGroup::Substance,
        });
        score -= 5;
    }

    // --- Positive signals ---
    if features.has_numbers {
        score += 10;
    }
    if features.has_question {
        score += 5;
        if goal == Some("replies") {
            score += 10;
        }
    }
    if features.char_count > 0 && features.char_count < 200 {
        score += 5;
    }
    if features.line_count > 1 {
        score += 5;
    }
    if features.hook_strength >= 70 {
        score += 10;
    }

    if features.est_dwell_seconds >= 10.0 {
        score += 5;
    }

    // Posting cadence. Replies are exempt. Author diversity (vm-ranker
    // params.rs:169-186): your k-th unseen post in one feed request scores
    // ×(0.75·0.5^k + 0.25) = ×1, ×0.625, ×0.44, ×0.34 … floor ×0.25.
    if !ctx.is_reply() {
        if let Ok(store) = crate::intel::store::IntelStore::open() {
            if let Ok(velocity) = store.get_recent_post_velocity() {
                if velocity.standalone_24h >= 4 {
                    issues.push(Issue {
                        severity: Severity::Warning,
                        code: "daily_cap_exceeded".into(),
                        message: format!(
                            "{} originals in 24 h (the ~4/day cap is empirical). Code fact: your 2nd/3rd/4th unseen post in a viewer's 48 h window scores ×0.625/×0.44/×0.34 (author diversity)",
                            velocity.standalone_24h
                        ),
                        fix: Some("Reply to others instead: replies don't compete with your originals".into()),
                        group: IssueGroup::Cadence,
                    });
                    score -= 15;
                } else if velocity.posts_6h >= 3 {
                    issues.push(Issue {
                        severity: Severity::Warning,
                        code: "author_diversity_penalty".into(),
                        message: format!(
                            "{} posts in the last 6 h. Each extra unseen post from you in a viewer's feed is multiplied by ×0.625, ×0.44, ×0.34 (vm-ranker author diversity, floor ×0.25)",
                            velocity.posts_6h
                        ),
                        fix: Some("Space originals 2 h+ apart: posts a viewer has seen leave the pool, so the decay only hits unseen ones".into()),
                        group: IssueGroup::Cadence,
                    });
                    score -= 15;
                } else if velocity.posts_1h >= 1 {
                    issues.push(Issue {
                        severity: Severity::Info,
                        code: "recent_post".into(),
                        message: format!(
                            "You posted {} time(s) in the last hour. That post is still in its 2 h cold-start window; a new original competes with it and scores ×0.625 in shared feeds",
                            velocity.posts_1h
                        ),
                        fix: Some("Wait until the last post is 2 h old".into()),
                        group: IssueGroup::Cadence,
                    });
                    score -= 5;
                }
            }
        }
        issues.extend(cold_start_issue(own_follower_count()));
    }

    // --- Reply-quality checks (only for replies) ---
    // 2026 algorithm update: short generic replies get no push. A reply that
    // reads as low-effort signals low-quality engagement and is suppressed.
    // These checks only apply in reply mode — standalone posts are scored
    // differently above.
    if ctx.is_reply() {
        let generic_phrases = [
            "great post", "great point", "great take", "well said", "this",
            "agreed", "i agree", "100%", "exactly", "so true", "facts",
            "love this", "love it", "nice", "based", "fire", "good point",
            "thanks for sharing", "preach", "yes", "yep", "truth", "real",
        ];
        let trimmed_lower = trimmed.to_lowercase();
        let stripped: String = trimmed_lower
            .chars()
            .filter(|c| c.is_alphanumeric() || c.is_whitespace())
            .collect();
        let stripped_compact = stripped.trim();

        // Emoji/symbol-only (no alphabetic chars at all)
        if !trimmed.chars().any(|c| c.is_alphabetic()) {
            issues.push(Issue {
                severity: Severity::Critical,
                code: "reply_emoji_only".into(),
                message: "Emoji-or-symbols-only reply — X treats these as noise, minimal algorithmic lift".into(),
                fix: Some("Add substance: an observation, question, or specific reaction".into()),
            group: IssueGroup::Reply,
            });
            score -= 25;
        }
        // Generic / low-effort phrase match
        else if generic_phrases.contains(&stripped_compact) {
            issues.push(Issue {
                severity: Severity::Critical,
                code: "reply_generic".into(),
                message: format!("Generic agreement reply. {REPLY_JUDGE}, and the author rarely replies to these"),
                fix: Some("Add a specific observation, counter-point, or question tied to the post's content".into()),
                group: IssueGroup::Reply,
            });
            score -= 25;
        }
        // Too short to be substantive (< 25 chars is almost never a meaningful reply)
        else if features.char_count < 25 {
            issues.push(Issue {
                severity: Severity::Warning,
                code: "reply_too_short".into(),
                message: format!(
                    "Reply is {} chars. {REPLY_JUDGE}; short replies rarely earn a reply-back",
                    features.char_count
                ),
                fix: Some("Expand to 1-2 sentences with a specific detail the author can respond to".into()),
                group: IssueGroup::Reply,
            });
            score -= 15;
        }

        let tells = slop_tells(trimmed);
        if !tells.is_empty() {
            let critical = tells.len() >= 3;
            issues.push(Issue {
                severity: if critical { Severity::Critical } else { Severity::Warning },
                code: "reply_slop".into(),
                message: format!(
                    "AI-writing tells: {}. X labels slop replies (llm_slop_post: down-ranked for 30 days), and an account that keeps it up gets llm_slop_user: SpamHighRecall for 30 days, hidden from every non-follower",
                    tells.join(", ")
                ),
                fix: Some("Rewrite it plainly, the way you'd say it out loud; one concrete point".into()),
                group: IssueGroup::Reply,
            });
            score -= if critical { 30 } else { 10 };
        }

        if let Some(overlap) = duplicate_overlap(trimmed, &own_recent_texts(30)) {
            issues.push(Issue {
                severity: Severity::Critical,
                code: "reply_duplicate".into(),
                message: format!(
                    "{:.0}% word overlap with one of your posts from the last 30 days. X clusters replies by words and labels every reply in a cluster COPYPASTA_SPAM",
                    overlap * 100.0
                ),
                fix: Some("Write this one fresh, about this post's specifics".into()),
                group: IssueGroup::Dedup,
            });
            score -= 30;
        }
    }

    // --- Structural markers that boost share-worthiness / dwell ---
    // 2026 community guidance: "here's how", "do this", "tbh" before a hot take,
    // and one-sentence-per-line formatting correlate with higher engagement.
    // These are small positive nudges, NOT large — they don't replace substance.
    let has_hot_take_marker = lower.starts_with("tbh ")
        || lower.contains("\ntbh ")
        || lower.contains(" tbh ")
        || lower.starts_with("hot take:")
        || lower.starts_with("unpopular opinion:");
    let has_instructional_marker = lower.contains("here's how")
        || lower.contains("here's why")
        || lower.contains("do this")
        || lower.contains("try this");
    if has_hot_take_marker || has_instructional_marker {
        score += 3;
    }
    // Sentence-per-line density — if >60% of lines are <= 12 words, reward.
    if features.line_count >= 3 {
        let short_lines = trimmed
            .lines()
            .filter(|l| !l.trim().is_empty() && l.split_whitespace().count() <= 12)
            .count();
        let non_empty_lines = trimmed.lines().filter(|l| !l.trim().is_empty()).count();
        if non_empty_lines > 0 && (short_lines * 100) / non_empty_lines >= 60 {
            score += 3;
        }
    }

    // Long-form heuristics (>500 chars).
    if features.char_count > 500 {
        // a) Sweet-spot reward (500–2000 chars).
        if features.char_count <= 2000 {
            score += 5;
        }
        // b) Beyond optimal note-tweet band — suggest splitting or publishing as Article.
        if features.char_count > 2000 && features.char_count <= 5000 {
            issues.push(Issue {
                severity: Severity::Info,
                code: "long_form_above_band".into(),
                message: format!(
                    "{} chars — past the 500-2000 dwell sweet spot. Article format typically gets better click-through with a cover card",
                    features.char_count
                ),
                fix: Some("Trim to <=2000 chars, or publish via the Articles feature".into()),
                group: IssueGroup::Substance,
            });
        }
        if features.char_count > 5000 {
            issues.push(Issue {
                severity: Severity::Warning,
                code: "long_form_too_long".into(),
                message: format!(
                    "{} chars — beyond optimal dwell band; readers drop off and the note-tweet preview truncates",
                    features.char_count
                ),
                fix: Some("Split into 2 long-form posts 2h apart, or publish as a native Article".into()),
                group: IssueGroup::Substance,
            });
            score -= 10;
        }

        // c) Preview-card hook check — only the first 280 chars surface in the
        //    feed before the "show more" cut. Re-run weak-opener detection on
        //    that slice so a strong long-form body isn't undone by a flat lead.
        let preview_end = trimmed.char_indices().nth(280).map(|(i, _)| i).unwrap_or(trimmed.len());
        let preview = &trimmed[..preview_end];
        let preview_first_line = preview.lines().next().unwrap_or("");
        if !preview_first_line.is_empty()
            && weak_openers.iter().any(|w| preview_first_line.starts_with(w))
            // Only flag if the short-post hook check above didn't already fire
            && !issues.iter().any(|i| i.code == "weak_hook")
        {
            issues.push(Issue {
                severity: Severity::Warning,
                code: "long_form_weak_preview".into(),
                message: "Long-form preview (first 280 chars) opens weakly — that's all the feed shows before 'show more'".into(),
                fix: Some("Lead with a number, a contrarian claim, or a named subject. Save soft setup for paragraph 2".into()),
            group: IssueGroup::Hook,
            });
            score -= 10;
        }

        // d) Scannability — long-form without paragraph breaks is a wall-of-text.
        //    Aim for >=1 line break per ~400 chars (rough density check).
        let breaks = trimmed.matches("\n\n").count() + trimmed.matches('\n').count();
        let needed_breaks = features.char_count / 400;
        if breaks < needed_breaks.max(2) {
            issues.push(Issue {
                severity: Severity::Warning,
                code: "long_form_wall_of_text".into(),
                message: format!(
                    "Wall of text — {} chars with only {} line breaks. Long-form on X is scanned, not read; people bounce on dense paragraphs",
                    features.char_count, breaks
                ),
                fix: Some("Break into short paragraphs (2-4 lines each), use bullet/number lists for enumeration, blank line between sections".into()),
                group: IssueGroup::Substance,
            });
            score -= 10;
        }

        // e) Payoff density — for long-form, low specificity is more punishing.
        //    Demand at least one number per 500 chars OR at least 2 proper nouns.
        let has_proper = has_proper_nouns(trimmed);
        let number_count = trimmed.split_whitespace()
            .filter(|w| w.chars().any(|c| c.is_ascii_digit()))
            .count();
        if number_count < (features.char_count / 500).max(1) && !has_proper {
            issues.push(Issue {
                severity: Severity::Info,
                code: "long_form_low_density".into(),
                message: "Long-form needs payoff density — concrete numbers, named subjects, or specific evidence. Without them readers feel padded out".into(),
                fix: Some("Add stats, dates, $ amounts, or named people/companies. The 2026 contest winners were data-dense investigations, not reflective essays".into()),
            group: IssueGroup::Substance,
            });
            score -= 5;
        }
    }

    // --- Sentiment check ---
    if features.sentiment == "negative" {
        issues.push(Issue {
            severity: Severity::Warning,
            code: "negative_sentiment".into(),
            message: "Combative tone. The ranker predicts each viewer's report (−234), mute (−58.8), not-interested (−47.5) and block (−31.2); when those outweigh the positives, the post sinks below every positive post for that viewer".into(),
            fix: Some("Reframe constructively — critique the idea, not the person".into()),
            group: IssueGroup::Safety,
        });
        score -= 15;
    } else if features.sentiment == "mixed" {
        issues.push(Issue {
            severity: Severity::Info,
            code: "mixed_sentiment".into(),
            message: "Mildly negative language: raises predicted mute (−58.8) and not-interested (−47.5)".into(),
            fix: Some(
                "Consider softening — the algorithm penalises predicted negative reactions".into(),
            ),
            group: IssueGroup::Safety,
        });
        score -= 5;
    }

    let score = score.clamp(0, 100) as u32;
    let grade = match score {
        90..=100 => "A",
        75..=89 => "B",
        60..=74 => "C",
        40..=59 => "D",
        _ => "F",
    }
    .to_string();

    let proxy_scores = estimate_proxies(trimmed, &features, ctx);
    let goal_scores = score_goals(&proxy_scores);
    let suggestions = suggest_improvements(&issues, &features, &proxy_scores, goal);
    let suggested_next_commands = build_next_commands(trimmed, score);

    let display_text = if trimmed.chars().count() > 200 {
        format!("{}...", crate::utils::safe_truncate(trimmed, 200))
    } else {
        trimmed.to_string()
    };

    PreflightResult {
        text: display_text,
        score,
        grade,
        issues,
        suggestions,
        features,
        suggested_next_commands,
        proxy_scores,
        goal_scores,
    }
}

// ---------------------------------------------------------------------------
// Proxy signal estimation
// ---------------------------------------------------------------------------

fn estimate_proxies(text: &str, features: &FeatureVector, ctx: &AnalyzeContext) -> ProxyScores {
    let lower = text.to_lowercase();

    let p_reply = {
        let mut s: f32 = 0.15;
        if features.has_question {
            s += 0.30;
        }
        let open_ended = ["what", "how", "why", "which", "where", "who"];
        if open_ended
            .iter()
            .any(|w| lower.starts_with(w) || lower.contains(&format!(" {w} ")))
        {
            s += 0.10;
        }
        let debate = [
            "unpopular opinion",
            "hot take",
            "controversial",
            "change my mind",
            "am i wrong",
            "disagree",
            "debate",
        ];
        if debate.iter().any(|d| lower.contains(d)) {
            s += 0.15;
        }
        if features.has_numbers || has_proper_nouns(text) {
            s += 0.05;
        }
        if ctx.is_reply() {
            s += 0.10;
        }
        s.min(1.0)
    };

    let p_quote = {
        let mut s: f32 = 0.08;
        if features.content_type_guess == "data" {
            s += 0.20;
        }
        let contrarian = [
            "actually",
            "most people",
            "nobody talks about",
            "the truth is",
            "unpopular",
        ];
        if contrarian.iter().any(|c| lower.contains(c)) {
            s += 0.15;
        }
        if lower.contains("1.") || lower.contains("1)") || lower.contains("step 1") {
            s += 0.10;
        }
        if features.word_count <= 30 && features.hook_strength >= 60 {
            s += 0.10;
        }
        if ctx.is_quote() {
            s += 0.10;
        }
        s.min(1.0)
    };

    let p_profile_click = {
        let mut s: f32 = 0.10;
        let curiosity = [
            "i spent",
            "after years of",
            "i've been",
            "here's what i learned",
            "lessons from",
        ];
        if curiosity.iter().any(|c| lower.contains(c)) {
            s += 0.20;
        }
        let authority = [
            "ceo", "founder", "built", "shipped", "years", "clients", "revenue", "raised",
        ];
        if authority.iter().any(|a| lower.contains(a)) {
            s += 0.10;
        }
        if ctx.author_voice.is_some() {
            s += 0.05;
        }
        if features.hook_strength >= 70 {
            s += 0.10;
        }
        s.min(1.0)
    };

    let p_follow = {
        let mut s: f32 = 0.05;
        if features.content_type_guess == "how-to" || features.content_type_guess == "data" {
            s += 0.15;
        }
        if lower.contains("thread") || lower.contains("1.") {
            s += 0.10;
        }
        if features.has_numbers && has_proper_nouns(text) {
            s += 0.10;
        }
        s += p_profile_click * 0.2;
        s.min(1.0)
    };

    let p_dm_share = {
        let mut s: f32 = 0.05;
        let practical = [
            "how to",
            "step by step",
            "guide",
            "tutorial",
            "template",
            "checklist",
            "framework",
            "playbook",
            "here's how",
            "hack",
            "trick",
            "tip",
        ];
        if practical.iter().any(|p| lower.contains(p)) {
            s += 0.25;
        }
        let insider = [
            "nobody talks about",
            "most people don't know",
            "insider",
            "behind the scenes",
            "secret",
            "hidden",
            "underrated",
        ];
        if insider.iter().any(|i| lower.contains(i)) {
            s += 0.20;
        }
        if features.content_type_guess == "data" {
            s += 0.15;
        }
        s.min(1.0)
    };

    let p_link_share = {
        let mut s: f32 = 0.05;
        if features.word_count <= 25 && features.hook_strength >= 60 {
            s += 0.15;
        }
        if features.has_numbers && features.content_type_guess == "data" {
            s += 0.15;
        }
        if features.content_type_guess == "announcement" {
            s += 0.15;
        }
        s.min(1.0)
    };

    let p_dwell = {
        let mut s: f32 = (features.est_dwell_seconds as f32 / 30.0).min(0.6);
        if features.line_count > 2 {
            s += 0.10;
        }
        if features.has_media {
            s += 0.15;
        }
        if ctx.has_poll {
            s += 0.10;
        }
        s.min(1.0)
    };

    let p_media_expand = if features.has_media {
        let mut s: f32 = 0.40;
        match ctx.media_kind {
            Some(MediaKind::Video) => s += 0.25,
            Some(MediaKind::Gif) => s += 0.15,
            Some(MediaKind::Image) => s += 0.10,
            None => {}
        }
        s.min(1.0)
    } else {
        0.0
    };

    let p_negative = {
        let mut s: f32 = 0.0;
        if features.sentiment == "negative" {
            s += 0.40;
        } else if features.sentiment == "mixed" {
            s += 0.15;
        }
        if engagement_bait(text).is_some() {
            s += 0.20;
        }
        let attacks = ["you're wrong", "shut up", "stfu", "cope", "ratio", "l + ratio"];
        if attacks.iter().any(|a| lower.contains(a)) {
            s += 0.25;
        }
        s.min(1.0)
    };

    ProxyScores {
        reply: p_reply,
        quote: p_quote,
        profile_click: p_profile_click,
        follow_author: p_follow,
        share_via_dm: p_dm_share,
        share_via_copy_link: p_link_share,
        dwell: p_dwell,
        media_expand: p_media_expand,
        negative_risk: p_negative,
    }
}

// ---------------------------------------------------------------------------
// Goal scoring
// ---------------------------------------------------------------------------

fn score_goals(proxies: &ProxyScores) -> GoalScores {
    let neg_penalty = 1.0 - (proxies.negative_risk * 0.6);

    let replies = ((proxies.reply * 0.65
        + proxies.dwell * 0.15
        + proxies.profile_click * 0.10
        + proxies.quote * 0.10)
        * neg_penalty
        * 100.0) as u32;

    let quotes = ((proxies.quote * 0.55
        + proxies.share_via_copy_link * 0.20
        + proxies.profile_click * 0.15
        + proxies.reply * 0.10)
        * neg_penalty
        * 100.0) as u32;

    let shares = ((proxies.share_via_dm * 0.45
        + proxies.share_via_copy_link * 0.35
        + proxies.dwell * 0.10
        + proxies.follow_author * 0.10)
        * neg_penalty
        * 100.0) as u32;

    let follows = ((proxies.follow_author * 0.50
        + proxies.profile_click * 0.25
        + proxies.share_via_dm * 0.15
        + proxies.dwell * 0.10)
        * neg_penalty
        * 100.0) as u32;

    let impressions = ((proxies.dwell * 0.25
        + proxies.reply * 0.20
        + proxies.share_via_dm * 0.15
        + proxies.share_via_copy_link * 0.10
        + proxies.quote * 0.10
        + proxies.media_expand * 0.10
        + proxies.follow_author * 0.10)
        * neg_penalty
        * 100.0) as u32;

    GoalScores {
        replies: replies.min(100),
        quotes: quotes.min(100),
        shares: shares.min(100),
        follows: follows.min(100),
        impressions: impressions.min(100),
    }
}

// ---------------------------------------------------------------------------
// Feature extraction
// ---------------------------------------------------------------------------

fn extract_features(text: &str) -> FeatureVector {
    let char_count = crate::utils::weighted_len(text);
    let word_count = text.split_whitespace().count();
    let line_count = text.lines().count();

    let has_link = !find_urls(text).is_empty();
    let link_position = if has_link { Some("body".into()) } else { None };

    let hashtag_count = count_real_hashtags(text);
    let has_question = text.contains('?');
    let has_numbers = text.chars().any(|c| c.is_ascii_digit());
    let starts_with_i = text.starts_with("I ") || text.starts_with("I'");

    let cta_patterns = [
        "check out",
        "click",
        "sign up",
        "subscribe",
        "join",
        "try it",
        "grab it",
        "get it",
        "learn more",
        "read more",
        "download",
    ];
    let lower = text.to_lowercase();
    let has_cta = cta_patterns.iter().any(|p| lower.contains(p));

    let hook_strength = score_hook(text.lines().next().unwrap_or(""));
    let content_type_guess = detect_content_type(text);

    let est_dwell_seconds = 1.0 + (word_count as f64 / 200.0) * 60.0;

    let negative_words = [
        "stupid",
        "idiot",
        "dumb",
        "hate",
        "terrible",
        "awful",
        "disgusting",
        "pathetic",
        "garbage",
        "trash",
        "worst",
        "moron",
        "clown",
        "fraud",
        "scam",
        "sucks",
        "useless",
        "incompetent",
        "liar",
        "bs",
        "stfu",
        "shut up",
        "you're wrong",
        "cope",
        "ratio",
    ];
    let aggressive_patterns = [
        "imagine thinking",
        "tell me you",
        "nobody asked",
        "stay mad",
        "cry about it",
        "skill issue",
        "l + ratio",
    ];
    let neg_count = negative_words
        .iter()
        .filter(|w| lower.contains(*w))
        .count();
    let aggro_count = aggressive_patterns
        .iter()
        .filter(|p| lower.contains(*p))
        .count();

    let sentiment = if neg_count >= 2 || aggro_count >= 1 {
        "negative".to_string()
    } else if neg_count == 1 {
        "mixed".to_string()
    } else {
        "neutral".to_string()
    };

    FeatureVector {
        char_count,
        word_count,
        has_link,
        link_position,
        has_media: false,
        hashtag_count,
        has_question,
        has_numbers,
        has_cta,
        hook_strength,
        line_count,
        starts_with_i,
        content_type_guess,
        est_dwell_seconds,
        sentiment,
    }
}

fn score_hook(first_line: &str) -> u32 {
    let trimmed = first_line.trim();
    if trimmed.is_empty() {
        return 0;
    }

    let mut score: u32 = 40;

    if trimmed.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        score += 30;
    }
    if trimmed.ends_with('?') {
        score += 20;
    }

    let bold_words = [
        "never", "always", "stop", "wrong", "truth", "secret", "nobody", "everyone",
    ];
    let lower = trimmed.to_lowercase();
    if bold_words.iter().any(|w| lower.contains(w)) {
        score += 15;
    }

    let weak = ["I ", "So ", "Just ", "The ", "It's ", "This is "];
    if weak.iter().any(|w| trimmed.starts_with(w)) {
        score = score.saturating_sub(20);
    }

    score.min(100)
}

fn detect_content_type(text: &str) -> String {
    let lower = text.to_lowercase();

    if lower.contains('?') && lower.lines().count() <= 3 {
        return "question".into();
    }

    let how_to_signals = ["how to", "step 1", "here's how", "guide", "tutorial", "tip:"];
    if how_to_signals.iter().any(|s| lower.contains(s)) {
        return "how-to".into();
    }

    let data_signals = ["%", "million", "billion", "$", "data shows", "study", "research"];
    if data_signals.iter().any(|s| lower.contains(s)) && text.chars().any(|c| c.is_ascii_digit()) {
        return "data".into();
    }

    let announcement_signals = [
        "announcing",
        "launching",
        "introducing",
        "excited to",
        "just shipped",
        "now available",
        "new:",
        "release",
    ];
    if announcement_signals.iter().any(|s| lower.contains(s)) {
        return "announcement".into();
    }

    "opinion".into()
}

/// Count real X hashtags only: `#` followed by alphanumeric (not space, not
/// another `#`). Skips markdown `##`/`###` header prefixes that surface in
/// long-form / Article bodies fetched via FxTwitter. Shared with
/// `IntelStore::log_post` so stored metadata agrees with preflight.
pub(crate) fn count_real_hashtags(text: &str) -> usize {
    let chars: Vec<char> = text.chars().collect();
    let mut count = 0;
    for (i, &c) in chars.iter().enumerate() {
        if c != '#' {
            continue;
        }
        // Previous char must NOT be `#` (skip ## / ### markdown)
        if i > 0 && chars[i - 1] == '#' {
            continue;
        }
        // Next char must be alphanumeric (real hashtag)
        match chars.get(i + 1) {
            Some(&next) if next.is_alphanumeric() => count += 1,
            _ => {}
        }
    }
    count
}

fn has_proper_nouns(text: &str) -> bool {
    let words: Vec<&str> = text.split_whitespace().collect();
    for (i, word) in words.iter().enumerate() {
        if i == 0 {
            continue;
        }
        let prev = words[i - 1];
        if prev.ends_with('.') || prev.ends_with('!') || prev.ends_with('?') {
            continue;
        }
        if word.chars().next().is_some_and(|c| c.is_uppercase())
            && !word.starts_with('#')
            && !word.starts_with('@')
            && !word.starts_with("http")
        {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Code-anchored detectors (xai-org/x-algorithm: grox, botmaker, VF rules)
// ---------------------------------------------------------------------------

/// grox/flows/reply_spam: task_write.py:140-141.
const REPLY_JUDGE: &str = "X's reply judge (an LLM) scores every reply to someone else's thread 0-3, and a 0 is labelled RISKY_HIGH_VIZ_REPLY and down-ranked under the post";

/// Redirectors whose hops X's URL-reputation rules score (a LOW_QUALITY hop
/// whose final hop isn't GOOD → SPAM_HIGH_RECALL). t.co is X's own wrapper.
const SHORTENERS: &[&str] = &[
    "bit.ly", "tinyurl.com", "ow.ly", "buff.ly", "lnkd.in", "rebrand.ly", "cutt.ly", "is.gd",
    "shorturl.at", "t.ly", "dlvr.it", "goo.gl", "tiny.cc", "rb.gy", "bl.ink", "shorte.st",
    "adf.ly", "trib.al", "s.id", "v.gd", "qr.ae", "soo.gd", "clck.ru", "bitly.com",
];

fn regex(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("static regex"))
}

fn trim_token(token: &str) -> &str {
    token.trim_end_matches([')', ']', ',', '.', '!', '?', ';', ':', '"', '\''])
}

fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let host = rest.split(['/', '?', '#']).next().unwrap_or("").to_lowercase();
    host.strip_prefix("www.").map(str::to_string).unwrap_or(host)
}

/// URLs X would link: anything with a scheme, plus bare shortener links.
pub(crate) fn find_urls(text: &str) -> Vec<&str> {
    text.split_whitespace()
        .map(trim_token)
        .filter(|t| {
            t.starts_with("http://")
                || t.starts_with("https://")
                || (t.contains('/') && SHORTENERS.contains(&host_of(t).as_str()))
        })
        .collect()
}

fn shortener_host(url: &str) -> Option<String> {
    let host = host_of(url);
    SHORTENERS.contains(&host.as_str()).then_some(host)
}

fn is_mention(token: &str) -> bool {
    let t = token.trim_start_matches('.');
    t.strip_prefix('@')
        .is_some_and(|h| h.chars().next().is_some_and(|c| c.is_ascii_alphanumeric() || c == '_'))
}

/// @handles in the body. A reply's leading run of @handles is X's own reply
/// prefix and doesn't count.
fn count_body_mentions(text: &str, is_reply: bool) -> usize {
    let mut leading = is_reply;
    text.split_whitespace()
        .filter(|t| {
            let m = is_mention(t);
            if leading && m {
                return false;
            }
            leading = false;
            m
        })
        .count()
}

/// Characters left once URLs, @handles and #hashtags are removed.
pub(crate) fn residual_text_len(text: &str) -> usize {
    let urls = find_urls(text);
    text.split_whitespace()
        .filter(|t| !is_mention(t) && !t.starts_with('#') && !urls.contains(&trim_token(t)))
        .map(|t| t.chars().count() + 1)
        .sum::<usize>()
        .saturating_sub(1)
}

/// Solicitation the PTOS classifier labels SpamEngagementBaiting/Farming
/// (→ SpamHighRecall, no PageRank exemption). Returns the matched phrase.
fn engagement_bait(text: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = regex(
        &RE,
        r"(?ix)\b(?:
            (?:like|rt|retweet|repost)\s+if
          | (?:rt|retweet|repost)\s+(?:to|for)\s+(?:win|a\s+chance|visibility|reach|more)
          | follow\s*(?:4|for)\s*follow | f4f | follow\s*back | follow\s+train
          | (?:like|follow)\s+(?:and|&|\+)\s+(?:rt|retweet|repost|comment|follow)
          | tag\s+(?:a|\d+|your|some)\s+(?:friends?|buddy|people) | tag\s+someone
          | drop\s+your\s+(?:handle|link|@)
          | (?:comment|reply|drop|type)\s[^.!?\n]{0,30}?\s(?:and|&)\s+(?:i'?ll|i\s+will|you'?ll\s+get|get|receive)
          | (?:i'?ll|i\s+will)\s+(?:follow|dm|send\s+it\s+to)\s+(?:back\s+)?(?:everyone|all|anyone)
        )\b",
    );
    re.find(text).map(|m| m.as_str().to_string())
}

/// Second-person insults: FOSNR_ABUSE_INSULTS (visibility-filtering
/// tweet_rules.rs:63-161).
fn insult_target(text: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = regex(
        &RE,
        r"(?ix)\b(?:
            idiots? | morons? | clowns? | imbeciles? | dumbass(?:es)? | cretins? | halfwits?
          | you(?:'re|\s+are)?\s+(?:so\s+|such\s+an?\s+|an?\s+)?(?:stupid|dumb|pathetic|delusional|clueless)
        )\b",
    );
    re.find(text).map(|m| m.as_str().to_string())
}

fn is_emoji(c: char) -> bool {
    matches!(c, '\u{1F300}'..='\u{1FAFF}' | '\u{2600}'..='\u{27BF}' | '\u{2B50}' | '\u{2B55}')
}

/// AI-writing tells that feed X's llm_slop labels.
fn slop_tells(text: &str) -> Vec<&'static str> {
    static PHRASES: OnceLock<Regex> = OnceLock::new();
    static NOT_X: OnceLock<Regex> = OnceLock::new();
    const LABELS: &[(&str, &str)] = &[
        ("great point", "\"great point\""),
        ("great question", "\"great question\""),
        ("so important", "\"so important\""),
        ("absolutely", "\"absolutely\""),
        ("delv", "\"delve\""),
        ("in a world where", "\"in a world where\""),
        ("game", "\"game-changer\""),
        ("testament", "\"a testament to\""),
        ("couldn", "\"couldn't agree more\""),
        ("spot on", "\"spot on\""),
        ("dive", "\"let's dive in\""),
        ("worth noting", "\"worth noting\""),
        ("tapestry", "\"tapestry\""),
        ("resonates", "\"resonates\""),
        ("underscores", "\"underscores\""),
        ("real question", "\"the real question is\""),
    ];
    let phrases = regex(
        &PHRASES,
        r"(?i)\b(?:great point|great question|this is so important|absolutely|delv(?:e|es|ing)|in a world where|game[- ]changer|a testament to|couldn'?t agree more|spot on|let'?s dive|worth noting|tapestry|resonates|underscores|the real question is)\b",
    );
    let not_x = regex(
        &NOT_X,
        r"(?i)\b(?:(?:it'?s|this is|that'?s) not|isn'?t)\s+(?:just\s+|only\s+|about\s+)?[^.!?\n]{1,60}?[,;—–]\s*(?:it'?s|this is|that'?s|but)\b",
    );

    let mut tells = Vec::new();
    for m in phrases.find_iter(text) {
        let found = m.as_str().to_lowercase();
        if let Some((_, label)) = LABELS.iter().find(|(key, _)| found.contains(key)) {
            if !tells.contains(label) {
                tells.push(*label);
            }
        }
    }
    if text.matches('—').count() >= 2 {
        tells.push("em-dash chain");
    }
    if not_x.is_match(text) {
        tells.push("\"it's not X, it's Y\"");
    }
    let lower = text.trim().to_lowercase();
    let sign_offs = [
        "what do you think?", "thoughts?", "agree?", "wdyt?", "what's your take?",
        "curious what you think", "curious to hear your thoughts", "would love to hear your thoughts",
    ];
    if sign_offs.iter().any(|s| lower.ends_with(s)) {
        tells.push("generic sign-off question");
    }
    let emoji_lines = text
        .lines()
        .filter(|l| l.trim_start().chars().next().is_some_and(is_emoji))
        .count();
    if emoji_lines >= 2 {
        tells.push("emoji bullets");
    }
    tells
}

/// Lowercase word set, without URLs and @handles (how X's copypasta jobs
/// cluster replies: word unigrams).
fn word_set(text: &str) -> std::collections::HashSet<String> {
    text.split_whitespace()
        .filter(|t| !is_mention(t) && !t.starts_with("http"))
        .flat_map(|t| t.split(|c: char| !c.is_alphanumeric()))
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// Highest unigram Jaccard of `text` against `history`, when it reaches 0.5
/// (botmaker BBQDuplicateTextRepliesProd; the threshold is a margin, X's is
/// redacted). Drafts under 4 distinct words are left to the length checks.
fn duplicate_overlap(text: &str, history: &[String]) -> Option<f64> {
    let mine = word_set(text);
    if mine.len() < 4 {
        return None;
    }
    history
        .iter()
        .map(|old| {
            let theirs = word_set(old);
            let union = mine.union(&theirs).count();
            if union == 0 { 0.0 } else { mine.intersection(&theirs).count() as f64 / union as f64 }
        })
        .fold(None, |best: Option<f64>, j| Some(best.map_or(j, |b| b.max(j))))
        .filter(|j| *j >= 0.5)
}

/// Your posts and replies from the last `days` days (local DB; empty when
/// there is none).
fn own_recent_texts(days: i64) -> Vec<String> {
    let since = chrono::Utc::now().timestamp() - days * 86_400;
    let Ok(conn) = crate::intel::open_db("xmaster.db") else { return Vec::new() };
    let Ok(mut stmt) = conn.prepare(
        "SELECT text FROM posts WHERE CAST(posted_at AS INTEGER) >= ?1 ORDER BY CAST(posted_at AS INTEGER) DESC LIMIT 500",
    ) else {
        return Vec::new();
    };
    stmt.query_map([since], |row| row.get::<_, String>(0))
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
}

/// Your follower count from the last `track` account snapshot (free; `None`
/// before the first snapshot).
pub(crate) fn own_follower_count() -> Option<u64> {
    let conn = crate::intel::open_db("xmaster.db").ok()?;
    conn.query_row(
        "SELECT followers FROM account_snapshots ORDER BY CAST(snapshot_at AS INTEGER) DESC LIMIT 1",
        [],
        |row| row.get::<_, i64>(0),
    )
    .ok()
    .filter(|f| *f > 0)
    .map(|f| f as u64)
}

/// Cold start (home-mixer author_cold_start.rs; param.rs:489-562, live since
/// 29 Sep 2026): originals by authors with ≤50k followers. Unknown follower
/// count counts as eligible.
fn cold_start_issue(followers: Option<u64>) -> Option<Issue> {
    if followers.is_some_and(|f| f > 50_000) {
        return None;
    }
    Some(Issue {
        severity: Severity::Info,
        code: "cold_start_window".into(),
        message: "Cold start: while this original is under 2 h old and under 200 home impressions it can win one exploration slot (rank 16 of a For You request), picked by likes per impression (authors with ≤50k followers). Early likes decide it".into(),
        fix: Some("Post when your followers are online, and don't post another original within 2 h".into()),
        group: IssueGroup::Cadence,
    })
}

fn suggest_improvements(
    issues: &[Issue],
    features: &FeatureVector,
    proxies: &ProxyScores,
    goal: Option<&str>,
) -> Vec<String> {
    let mut suggestions = Vec::new();

    for issue in issues {
        if let Some(ref fix) = issue.fix {
            suggestions.push(fix.clone());
        }
    }

    match goal {
        Some("replies") => {
            if !features.has_question {
                suggestions.push(
                    "Add a question — questions drive replies, a top positive scorer term".into(),
                );
            }
            if proxies.reply < 0.30 {
                suggestions.push(
                    "Try an open-ended question (what/how/why) to boost reply probability".into(),
                );
            }
        }
        Some("impressions") => {
            if features.hook_strength < 70 {
                suggestions.push(
                    "Strengthen your hook — first line determines if people stop scrolling".into(),
                );
            }
            if features.line_count <= 1 && features.char_count > 80 {
                suggestions.push(
                    "Add line breaks — visual spacing increases dwell time (a scoring signal)"
                        .into(),
                );
            }
            if proxies.dwell < 0.20 {
                suggestions
                    .push("Add more depth — longer dwell time increases distribution".into());
            }
        }
        Some("shares") if proxies.share_via_dm < 0.15 => {
            suggestions.push(
                "Make it worth sending: copy-link shares (weight 20) and DM shares (5) are the top positive signals".into(),
            );
        }
        Some("follows") if proxies.profile_click < 0.20 => {
            suggestions.push(
                "Add a curiosity gap or credentials — profile clicks are the gateway to follows".into(),
            );
        }
        Some("quotes") if proxies.quote < 0.15 => {
            suggestions.push(
                "Make it quotable — contrarian takes, data points, or short punchy claims".into(),
            );
        }
        _ => {}
    }

    if features.est_dwell_seconds < 5.0 && features.char_count > 0 {
        suggestions.push(format!(
            "Est. dwell time: {:.0}s — longer posts drive more dwell_time signal. Consider adding depth.",
            features.est_dwell_seconds
        ));
    }

    if features.content_type_guess == "opinion" && !features.has_numbers {
        suggestions
            .push("Data-backed opinions outperform pure takes — add a number or citation".into());
    }

    if features.content_type_guess == "data" || features.content_type_guess == "how-to" {
        suggestions.push(
            "This looks shareable: insider data and how-tos drive copy-link (20) and DM (5) shares"
                .into(),
        );
    }

    if proxies.negative_risk >= 0.30 {
        suggestions
            .push("High negative-reaction risk — Grok will suppress this. Soften the tone.".into());
    }

    suggestions.dedup();
    suggestions
}

fn build_next_commands(text: &str, score: u32) -> Vec<String> {
    let escaped = text.replace('"', "\\\"");
    if score >= 75 {
        vec![format!("xmaster post \"{}\"", escaped)]
    } else {
        vec!["xmaster analyze \"<your revised text>\" --goal replies".to_string()]
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn default_ctx() -> AnalyzeContext {
        AnalyzeContext::default()
    }

    fn ctx_with_goal(goal: &str) -> AnalyzeContext {
        AnalyzeContext {
            goal: Some(goal.to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn empty_tweet_is_critical() {
        let result = analyze("", &default_ctx());
        assert!(result.score < 50);
        assert_eq!(result.issues[0].code, "empty_content");
    }

    fn has(result: &PreflightResult, code: &str) -> bool {
        result.issues.iter().any(|i| i.code == code)
    }

    fn severity(result: &PreflightResult, code: &str) -> Option<Severity> {
        result.issues.iter().find(|i| i.code == code).map(|i| i.severity.clone())
    }

    #[test]
    fn plain_link_is_only_a_cost_note() {
        // open_link weighs +0.2 (param.rs:348): no reach penalty for a link.
        let result = analyze(
            "Rapamycin extended median lifespan 12% in the ITP 2024 cohort, full data here https://example.com/itp",
            &default_ctx(),
        );
        assert_eq!(severity(&result, "link_api_cost"), Some(Severity::Info));
        assert!(!has(&result, "link_in_body"));
        assert!(!has(&result, "link_bare"));
        assert!(!result.issues.iter().any(|i| i.severity == Severity::Critical));
    }

    #[test]
    fn bare_link_is_a_warning() {
        let result = analyze("Check this out https://example.com", &default_ctx());
        assert_eq!(severity(&result, "link_bare"), Some(Severity::Warning));
    }

    #[test]
    fn shortener_is_critical_with_or_without_scheme() {
        for text in ["New results on senolytics in mice: https://bit.ly/3xYz", "New results on senolytics in mice: bit.ly/3xYz"] {
            let result = analyze(text, &default_ctx());
            assert_eq!(severity(&result, "link_shortener"), Some(Severity::Critical), "{text}");
        }
        // X's own t.co wrapper is not a shortener hop to flag.
        let result = analyze("Senolytics data from the 2024 cohort https://t.co/abc123", &default_ctx());
        assert!(!has(&result, "link_shortener"));
    }

    #[test]
    fn link_in_reply_is_critical() {
        let result = analyze(
            "We saw the same effect in our 2023 cohort, writeup at https://example.com/x",
            &reply_ctx(),
        );
        assert_eq!(severity(&result, "reply_has_link"), Some(Severity::Critical));
        assert!(!has(&result, "link_mention_nonfollower"));
    }

    #[test]
    fn link_plus_mention_warns() {
        let result = analyze(
            "Great breakdown of the ITP data from @peterattiamd https://example.com/itp-2024-results",
            &default_ctx(),
        );
        assert_eq!(severity(&result, "link_mention_nonfollower"), Some(Severity::Warning));
    }

    #[test]
    fn two_mentions_trigger_mention_burst_but_reply_prefix_does_not() {
        let result = analyze("Thanks @alice and @bob for the 2024 data", &default_ctx());
        assert_eq!(severity(&result, "mention_burst"), Some(Severity::Warning));
        let reply = analyze("@alice @bob The 2024 cohort showed the same thing at 12 months", &reply_ctx());
        assert!(!has(&reply, "mention_burst"));
        let reply_body = analyze("@alice agreed, and @bob and @carol saw it too in 2024", &reply_ctx());
        assert!(has(&reply_body, "mention_burst"));
    }

    #[test]
    fn engagement_bait_patterns_are_critical() {
        for text in [
            "Comment GENE and I'll DM you the protocol",
            "Drop your handle below, building a list of longevity founders",
            "Follow back if you're into biotech",
            "Follow for follow, longevity people",
            "RT to win a free copy of the book",
            "Tag a friend who needs to see this",
            "Like and repost if you agree",
            "I'll follow everyone who replies",
        ] {
            let result = analyze(text, &default_ctx());
            assert_eq!(severity(&result, "engagement_bait"), Some(Severity::Critical), "{text}");
        }
    }

    #[test]
    fn engagement_bait_respects_word_boundaries() {
        // "start if" used to match "rt if".
        for text in [
            "Start if you have the 2024 data ready, then rerun the model",
            "Follow the money: 3 biotech funds raised $2B in Q3",
        ] {
            let result = analyze(text, &default_ctx());
            assert!(!has(&result, "engagement_bait"), "{text}");
        }
    }

    #[test]
    fn insult_target_warns() {
        let result = analyze("You're delusional if you think Bryan Johnson's protocol scales", &default_ctx());
        assert_eq!(severity(&result, "insult_target"), Some(Severity::Warning));
        let calm = analyze("Bryan Johnson's protocol won't scale: it costs $2M a year", &default_ctx());
        assert!(!has(&calm, "insult_target"));
    }

    #[test]
    fn negative_sentiment_cites_real_weights() {
        let result = analyze("This is stupid garbage and pathetic", &default_ctx());
        let issue = result.issues.iter().find(|i| i.code == "negative_sentiment").unwrap();
        assert!(issue.message.contains("−234"), "{}", issue.message);
    }

    #[test]
    fn reply_slop_warns_then_goes_critical() {
        let one = analyze("Absolutely, the 2024 ITP numbers back that up for rapamycin", &reply_ctx());
        assert_eq!(severity(&one, "reply_slop"), Some(Severity::Warning));
        let many = analyze(
            "Great point! It's not just about lifespan — it's about healthspan — and this underscores everything. Thoughts?",
            &reply_ctx(),
        );
        assert_eq!(severity(&many, "reply_slop"), Some(Severity::Critical));
        let clean = analyze("We saw a 12% drop in IGF-1 at month 3 on the same dose", &reply_ctx());
        assert!(!has(&clean, "reply_slop"));
        // Standalone posts are left to the JEV slop flag.
        assert!(!has(&analyze("Absolutely wild 2024 data on rapamycin", &default_ctx()), "reply_slop"));
    }

    #[test]
    fn duplicate_overlap_uses_word_unigrams() {
        let history = vec!["Rapamycin at 6mg weekly dropped my IGF-1 by 12% in 3 months".to_string()];
        let dup = duplicate_overlap("rapamycin at 6mg weekly dropped my IGF-1 by 12% in three months", &history);
        assert!(dup.is_some_and(|j| j >= 0.5), "{dup:?}");
        assert!(duplicate_overlap("Metformin did nothing for my HbA1c over a year", &history).is_none());
        // Mentions and URLs don't count; drafts under 4 words are skipped.
        assert!(duplicate_overlap("@a @b yes indeed", &history).is_none());
    }

    #[test]
    fn cold_start_only_for_small_authors() {
        assert!(cold_start_issue(Some(463)).is_some_and(|i| i.severity == Severity::Info));
        assert!(cold_start_issue(None).is_some());
        assert!(cold_start_issue(Some(50_000)).is_some());
        assert!(cold_start_issue(Some(50_001)).is_none());
        // Replies are not eligible (author_cold_start.rs:188-193).
        assert!(!has(&analyze("We saw the same 12% effect at month 3", &reply_ctx()), "cold_start_window"));
    }

    #[test]
    fn length_is_weighted_like_x() {
        // A URL counts 23 and an emoji 2, whatever their raw length.
        let url = format!("https://example.com/{}", "a".repeat(200));
        let result = analyze(&format!("Read this {url}"), &default_ctx());
        assert_eq!(result.features.char_count, "Read this ".len() + 23);
        let emoji = "🧬".repeat(141);
        assert!(has(&analyze(&emoji, &default_ctx()), "over_limit"));
    }

    #[test]
    fn over_280_is_critical_without_premium() {
        let long = "a".repeat(300);
        let result = analyze(&long, &default_ctx());
        assert!(result.issues.iter().any(|i| i.code == "over_limit"));
    }

    #[test]
    fn over_280_ok_with_premium() {
        let long = "a".repeat(300);
        let ctx = AnalyzeContext { premium: true, ..Default::default() };
        let result = analyze(&long, &ctx);
        assert!(!result.issues.iter().any(|i| i.code == "over_limit"));
    }

    #[test]
    fn over_25000_is_critical_even_with_premium() {
        let long = "a".repeat(25_001);
        let ctx = AnalyzeContext { premium: true, ..Default::default() };
        let result = analyze(&long, &ctx);
        assert!(result.issues.iter().any(|i| i.code == "over_limit"));
    }

    #[test]
    fn clean_tweet_scores_well() {
        let result = analyze(
            "7 things I learned building a startup in 2024:\n\n1. Speed beats perfection\n2. Talk to users daily\n3. Ship or die",
            &default_ctx(),
        );
        assert!(result.score >= 60, "score was {}", result.score);
        assert!(!result.features.content_type_guess.is_empty());
    }

    #[test]
    fn question_detected() {
        let result = analyze(
            "What's the hardest lesson you learned this year?",
            &ctx_with_goal("replies"),
        );
        assert!(result.features.has_question);
        assert!(result.score >= 50, "score was {}", result.score);
    }

    #[test]
    fn weak_hook_flagged() {
        let result = analyze(
            "I think this is an interesting take on the market",
            &default_ctx(),
        );
        assert!(result.issues.iter().any(|i| i.code == "weak_hook"));
    }

    #[test]
    fn grade_mapping() {
        let result = analyze(
            "Stop sleeping on Rust.\n\n3 reasons it will dominate backend in 2025:",
            &default_ctx(),
        );
        assert!(
            ["A", "B", "C"].contains(&result.grade.as_str()),
            "grade was {}",
            result.grade
        );
    }

    #[test]
    fn engagement_bait_detected() {
        let result = analyze(
            "Like if you agree with this take on AI",
            &default_ctx(),
        );
        assert!(result.issues.iter().any(|i| i.code == "engagement_bait"));
    }

    #[test]
    fn starts_with_mention_flagged() {
        let result = analyze(
            "@elonmusk what do you think about this?",
            &default_ctx(),
        );
        assert!(result
            .issues
            .iter()
            .any(|i| i.code == "starts_with_mention"));
    }

    #[test]
    fn at_281_is_over_limit_without_premium() {
        let long = "x".repeat(281);
        let result = analyze(&long, &default_ctx());
        let issue = result
            .issues
            .iter()
            .find(|i| i.code == "over_limit")
            .unwrap();
        assert_eq!(issue.severity, Severity::Critical);
    }

    #[test]
    fn at_281_is_fine_with_premium() {
        let long = "x".repeat(281);
        let ctx = AnalyzeContext { premium: true, ..Default::default() };
        let result = analyze(&long, &ctx);
        assert!(!result.issues.iter().any(|i| i.code == "over_limit"));
    }

    #[test]
    fn short_question_not_penalized_as_too_short() {
        let result = analyze("What's your biggest regret?", &default_ctx());
        assert!(result.features.has_question);
        assert!(
            !result.issues.iter().any(|i| i.code == "too_short"),
            "short question should not be flagged as too_short"
        );
    }

    #[test]
    fn specific_numbers_boost_score() {
        let with_numbers = analyze(
            "3 things I learned building startups in 2024",
            &default_ctx(),
        );
        let without_numbers = analyze(
            "Things I learned building startups recently",
            &default_ctx(),
        );
        assert!(
            with_numbers.score > without_numbers.score,
            "with_numbers={} should beat without_numbers={}",
            with_numbers.score,
            without_numbers.score
        );
    }

    #[test]
    fn perfect_tweet_scores_high() {
        let text = "3 things Google taught me about scaling:\n\n1. Cache everything\n2. Fail fast\n\nWhat would you add?";
        let result = analyze(text, &default_ctx());
        assert!(
            result.score >= 75,
            "perfect tweet score was {}",
            result.score
        );
        assert!(
            result.grade == "A" || result.grade == "B",
            "grade was {}",
            result.grade
        );
    }

    #[test]
    fn empty_text_is_critical() {
        let result = analyze("   ", &default_ctx());
        let issue = result
            .issues
            .iter()
            .find(|i| i.code == "empty_content")
            .unwrap();
        assert_eq!(issue.severity, Severity::Critical);
    }

    #[test]
    fn rt_if_detected_as_engagement_bait() {
        let result = analyze(
            "RT if you think Rust is the future of systems programming",
            &default_ctx(),
        );
        assert!(result.issues.iter().any(|i| i.code == "engagement_bait"));
    }

    #[test]
    fn excessive_hashtags_warned() {
        let result = analyze(
            "Great day #rust #programming #code #dev",
            &default_ctx(),
        );
        assert!(result
            .issues
            .iter()
            .any(|i| i.code == "excessive_hashtags"));
    }

    // --- Proxy signal tests ---

    #[test]
    fn question_drives_reply_proxy() {
        let q = analyze(
            "What's the biggest mistake founders make?",
            &default_ctx(),
        );
        let s = analyze(
            "Founders make a lot of mistakes in their journey.",
            &default_ctx(),
        );
        assert!(
            q.proxy_scores.reply > s.proxy_scores.reply,
            "question reply={:.2} should beat statement reply={:.2}",
            q.proxy_scores.reply,
            s.proxy_scores.reply
        );
    }

    #[test]
    fn data_content_drives_quote_proxy() {
        let data = analyze(
            "73% of startups fail because of premature scaling — research from 2024",
            &default_ctx(),
        );
        let opinion = analyze(
            "I think startups fail because of bad decisions",
            &default_ctx(),
        );
        assert!(
            data.proxy_scores.quote > opinion.proxy_scores.quote,
            "data quote={:.2} should beat opinion quote={:.2}",
            data.proxy_scores.quote,
            opinion.proxy_scores.quote
        );
    }

    #[test]
    fn practical_content_drives_dm_share() {
        let howto = analyze(
            "How to build a CLI in Rust — step by step guide:",
            &default_ctx(),
        );
        let opinion = analyze(
            "Rust is a great language for building tools",
            &default_ctx(),
        );
        assert!(
            howto.proxy_scores.share_via_dm > opinion.proxy_scores.share_via_dm,
            "howto dm_share={:.2} should beat opinion dm_share={:.2}",
            howto.proxy_scores.share_via_dm,
            opinion.proxy_scores.share_via_dm
        );
    }

    #[test]
    fn negative_tone_raises_negative_risk() {
        let neg = analyze(
            "This is stupid garbage and you're an idiot if you believe it",
            &default_ctx(),
        );
        let pos = analyze(
            "Here's a thoughtful take on why this approach works better",
            &default_ctx(),
        );
        assert!(
            neg.proxy_scores.negative_risk > pos.proxy_scores.negative_risk,
            "negative risk={:.2} should beat positive risk={:.2}",
            neg.proxy_scores.negative_risk,
            pos.proxy_scores.negative_risk
        );
    }

    #[test]
    fn media_context_drives_media_expand() {
        let with_media = analyze(
            "Check this out",
            &AnalyzeContext {
                has_media: true,
                media_kind: Some(MediaKind::Image),
                ..Default::default()
            },
        );
        let without_media = analyze("Check this out", &default_ctx());
        assert!(
            with_media.proxy_scores.media_expand > without_media.proxy_scores.media_expand,
            "media expand={:.2} should beat no-media={:.2}",
            with_media.proxy_scores.media_expand,
            without_media.proxy_scores.media_expand
        );
    }

    #[test]
    fn goal_scores_populated() {
        let result = analyze(
            "3 things Google taught me about scaling:\n\n1. Cache everything\n2. Fail fast\n\nWhat would you add?",
            &default_ctx(),
        );
        assert!(result.goal_scores.replies > 0);
        assert!(result.goal_scores.impressions > 0);
    }

    #[test]
    fn goal_scores_capped_at_100() {
        let result = analyze(
            "What's the #1 thing nobody talks about in startups? Here's how to build a $1M ARR company step by step — the secret framework:",
            &default_ctx(),
        );
        assert!(result.goal_scores.replies <= 100);
        assert!(result.goal_scores.quotes <= 100);
        assert!(result.goal_scores.shares <= 100);
        assert!(result.goal_scores.follows <= 100);
        assert!(result.goal_scores.impressions <= 100);
    }

    fn reply_ctx() -> AnalyzeContext {
        AnalyzeContext {
            mode: Some(PostMode::Reply),
            ..Default::default()
        }
    }

    #[test]
    fn reply_generic_phrase_is_critical() {
        let result = analyze("great post", &reply_ctx());
        assert!(
            result.issues.iter().any(|i| i.code == "reply_generic"),
            "got issues: {:?}",
            result.issues.iter().map(|i| &i.code).collect::<Vec<_>>()
        );
    }

    #[test]
    fn reply_emoji_only_is_critical() {
        let result = analyze("🔥🔥🔥", &reply_ctx());
        assert!(result.issues.iter().any(|i| i.code == "reply_emoji_only"));
    }

    #[test]
    fn reply_too_short_is_warning() {
        let result = analyze("yeah makes sense", &reply_ctx());
        assert!(result.issues.iter().any(|i| i.code == "reply_too_short"));
    }

    #[test]
    fn substantive_reply_has_no_reply_quality_issue() {
        let result = analyze(
            "Interesting angle — did you consider the latency trade-off when the cache invalidates under load?",
            &reply_ctx(),
        );
        assert!(!result
            .issues
            .iter()
            .any(|i| matches!(i.code.as_str(), "reply_generic" | "reply_emoji_only" | "reply_too_short")));
    }

    #[test]
    fn reply_mode_skips_standalone_quality_warnings() {
        // Short (<30 chars) replies should NOT trigger the "too_short" standalone
        // warning — they only get reply-specific warnings above.
        let result = analyze(
            "Fair point, though I'd weight latency higher here.",
            &reply_ctx(),
        );
        assert!(!result.issues.iter().any(|i| i.code == "too_short"));
    }

    // --- v1.7.0: research-derived gates ---

    #[test]
    fn weak_hook_no_weight_fires_for_generic_opener() {
        // Generic opener with no number, no proper noun, no status verb
        let result = analyze(
            "Something to consider when you're working through hard problems lately",
            &default_ctx(),
        );
        assert!(
            result.issues.iter().any(|i| i.code == "weak_hook_no_weight"),
            "expected weak_hook_no_weight; got: {:?}",
            result.issues.iter().map(|i| &i.code).collect::<Vec<_>>()
        );
    }

    #[test]
    fn weak_hook_no_weight_skips_when_number_present() {
        let result = analyze(
            "73% of startups fail because they scale prematurely",
            &default_ctx(),
        );
        assert!(!result.issues.iter().any(|i| i.code == "weak_hook_no_weight"));
    }

    #[test]
    fn weak_hook_no_weight_skips_when_status_verb_present() {
        let result = analyze(
            "BREAKING: the latest data on engagement scoring",
            &default_ctx(),
        );
        assert!(!result.issues.iter().any(|i| i.code == "weak_hook_no_weight"));
    }

    #[test]
    fn reference_only_risk_fires_on_data_without_pov() {
        // High specificity (numbers + proper nouns) + zero first-person/POV markers + >200 chars
        let text = "Deloitte received $74 billion from Department of Defense contracts in 2024. \
                    The CMS spent $4.2 billion on Accenture in the same year. \
                    Boeing pulled in $3.8 billion from FAA. \
                    Lockheed Martin had the highest single contract at $2.5 billion.";
        let result = analyze(text, &default_ctx());
        assert!(
            result.issues.iter().any(|i| i.code == "reference_only_risk"),
            "expected reference_only_risk; got: {:?}",
            result.issues.iter().map(|i| &i.code).collect::<Vec<_>>()
        );
    }

    #[test]
    fn reference_only_risk_skips_when_pov_present() {
        let text = "Deloitte received $74 billion from DoD in 2024. I think this is \
                    obscene rent-seeking. Most people don't realize how much consulting \
                    money is just relabelled government spending.";
        let result = analyze(text, &default_ctx());
        assert!(!result.issues.iter().any(|i| i.code == "reference_only_risk"));
    }

    #[test]
    fn multiple_ctas_fires_on_two_or_more() {
        let text = "Great thread on shipping speed. Bookmark this for your next launch. \
                    Also follow me for more like this. DM me if you want the template.";
        let result = analyze(text, &default_ctx());
        assert!(
            result.issues.iter().any(|i| i.code == "multiple_ctas"),
            "expected multiple_ctas; got: {:?}",
            result.issues.iter().map(|i| &i.code).collect::<Vec<_>>()
        );
    }

    #[test]
    fn multiple_ctas_skips_on_single_cta() {
        let text = "Great thread on shipping speed. Bookmark this for your next launch.";
        let result = analyze(text, &default_ctx());
        assert!(!result.issues.iter().any(|i| i.code == "multiple_ctas"));
    }

    #[test]
    fn every_issue_has_group_field() {
        // Trigger many issues at once
        let text = "I just shipped something today";
        let result = analyze(text, &default_ctx());
        for issue in &result.issues {
            // group is non-optional, so this just verifies it compiles + serializes.
            let _ = issue.group;
        }
        // Sanity: at least one issue fires
        assert!(!result.issues.is_empty());
    }

    #[test]
    fn hashtag_count_ignores_markdown_headers() {
        // v1.7.1 fix: long-form Article bodies surfaced by FxTwitter use
        // markdown header prefixes (## / ### ) for header-two/three blocks.
        // The hashtag check must NOT count these as real X hashtags.
        let text = "## Introduction\n\nSome words here.\n\n### Subsection\n\nMore content.";
        let result = analyze(text, &default_ctx());
        assert!(
            !result.issues.iter().any(|i| i.code == "excessive_hashtags"),
            "markdown headers should not trigger excessive_hashtags; got: {:?}",
            result.issues.iter().map(|i| &i.code).collect::<Vec<_>>()
        );
    }

    #[test]
    fn hashtag_count_still_catches_real_hashtags() {
        let text = "Loving #rust #programming #cli #dev — all the fun";
        let result = analyze(text, &default_ctx());
        assert!(
            result.issues.iter().any(|i| i.code == "excessive_hashtags"),
            "real hashtags should still trigger; got: {:?}",
            result.issues.iter().map(|i| &i.code).collect::<Vec<_>>()
        );
    }
}
