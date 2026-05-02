use serde::Serialize;

/// Heuristic post quality analysis. Checks for common patterns that hurt or
/// help reach based on estimated 2026 X algorithm signals. This is NOT a direct
/// algorithm score — it's a quality lint.
///
/// Reference: xai-org/x-algorithm (January 2026, Grok-based transformer).
/// Exact weight constants are NOT published. Estimates below from code structure
/// + empirical data.
///
/// Top positive signals (estimated): follow_author (~30x), share_via_dm (~25x),
///   reply (~20x), share_via_copy_link (~20x), quote (~18x), profile_click (~12x)
/// Negative signals: report (~-369x), block (~-74x), mute (~-40x), not_interested (~-20x)
pub const ALGORITHM_SOURCE: &str = "xai-org/x-algorithm (January 2026, Grok-based)";

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
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub enum Severity {
    Critical,
    Warning,
    Info,
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
        });
        score -= 30;
    }

    let char_limit = if ctx.premium { 25_000 } else { 280 };
    if features.char_count > char_limit {
        issues.push(Issue {
            severity: Severity::Critical,
            code: "over_limit".into(),
            message: format!(
                "Post is {} characters (limit: {})",
                features.char_count, char_limit
            ),
            fix: Some(format!(
                "Remove {} characters",
                features.char_count - char_limit
            )),
        });
        score -= 30;
    }

    if features.has_link && features.link_position.as_deref() == Some("body") {
        issues.push(Issue {
            severity: Severity::Critical,
            code: "link_in_body".into(),
            message: "External link in tweet body kills reach — non-Premium accounts get near-zero engagement, Premium accounts lose 30-50% reach (Q1 2026 data)".into(),
            fix: Some("Move the link to a reply instead".into()),
        });
        score -= 30;
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
            fix: Some("Lead with a number, question, or bold claim".into()),
        });
        score -= 15;
    }

    let lower = trimmed.to_lowercase();
    let bait_phrases = ["like if", "rt if", "follow for"];
    if bait_phrases.iter().any(|b| lower.contains(b)) {
        issues.push(Issue {
            severity: Severity::Warning,
            code: "engagement_bait".into(),
            message: "Engagement bait detected — X algorithm penalizes this".into(),
            fix: Some("Remove explicit engagement requests".into()),
        });
        score -= 15;
    }

    if features.hashtag_count > 2 {
        issues.push(Issue {
            severity: Severity::Warning,
            code: "excessive_hashtags".into(),
            message: format!(
                "{} hashtags — more than 2 looks spammy and hurts reach",
                features.hashtag_count
            ),
            fix: Some("Keep to 1-2 relevant hashtags max".into()),
        });
        score -= 15;
    }

    if !features.has_numbers && !has_proper_nouns(trimmed) {
        issues.push(Issue {
            severity: Severity::Warning,
            code: "low_specificity".into(),
            message: "No numbers, names, or data — specificity drives engagement".into(),
            fix: Some("Add a concrete number, name, or data point".into()),
        });
        score -= 15;
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
        });
        score -= 5;
    }

    if trimmed.starts_with('@') {
        issues.push(Issue {
            severity: Severity::Warning,
            code: "starts_with_mention".into(),
            message: "Starting with @mention limits visibility to mutual followers".into(),
            fix: Some("Put a word before the @mention, e.g. \".@user\"".into()),
        });
        score -= 15;
    }

    // --- Info issues (score -= 5) ---
    if !features.has_question {
        issues.push(Issue {
            severity: Severity::Info,
            code: "no_question".into(),
            message: "No question mark — questions drive replies (reply_engaged_by_author is +75, the single highest signal)".into(),
            fix: Some("Consider ending with a question to invite discussion".into()),
        });
        score -= 5;
    }

    if features.line_count <= 1 && features.char_count > 100 {
        issues.push(Issue {
            severity: Severity::Info,
            code: "no_formatting".into(),
            message: "Wall of text — line breaks improve readability and stop-rate".into(),
            fix: Some("Break into 2-3 short lines".into()),
        });
        score -= 5;
    }

    if trimmed == lower && trimmed.chars().any(|c| c.is_alphabetic()) {
        issues.push(Issue {
            severity: Severity::Info,
            code: "all_lowercase".into(),
            message: "All lowercase — proper capitalization looks more authoritative".into(),
            fix: None,
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

    // --- Author diversity + daily cap warnings ---
    // The algorithm only shows 2-3 of your posts per feed session, and 2026
    // spam-flag heuristics treat >4 standalone posts/day as suspicious. Check
    // the store for recent posting velocity. Replies are exempt from the daily
    // cap — heavy replying is fine (the Feb 2026 algorithm update rewards it).
    if !ctx.is_reply() {
        if let Ok(store) = crate::intel::store::IntelStore::open() {
            if let Ok(velocity) = store.get_recent_post_velocity() {
                if velocity.standalone_24h >= 4 {
                    issues.push(Issue {
                        severity: Severity::Critical,
                        code: "daily_cap_exceeded".into(),
                        message: format!(
                            "{} standalone posts in last 24h — 2026 spam heuristic flags >4/day, harming future reach. Posting now risks account-score damage",
                            velocity.standalone_24h
                        ),
                        fix: Some("Stop posting today. Reply to others instead — replies don't count toward the cap and drive the highest signal (reply_engaged_by_author ~150x a like)".into()),
                    });
                    score -= 30;
                } else if velocity.posts_6h >= 3 {
                    issues.push(Issue {
                        severity: Severity::Warning,
                        code: "author_diversity_penalty".into(),
                        message: format!(
                            "{} posts in the last 6h — author diversity scorer limits you to 2-3 per feed session, extra posts dilute your average without adding reach",
                            velocity.posts_6h
                        ),
                        fix: Some("Wait at least 2 hours between posts — fewer, better posts outperform high volume".into()),
                    });
                    score -= 15;
                } else if velocity.posts_1h >= 1 {
                    issues.push(Issue {
                        severity: Severity::Info,
                        code: "recent_post".into(),
                        message: format!(
                            "You posted {} time(s) in the last hour — the algorithm's 30-60 min distribution gate means your previous post may still be in its critical traction window",
                            velocity.posts_1h
                        ),
                        fix: Some("Consider waiting — posting now may split attention from your previous post's traction window".into()),
                    });
                    score -= 5;
                }
            }
        }
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
            });
            score -= 25;
        }
        // Generic / low-effort phrase match
        else if generic_phrases.contains(&stripped_compact) {
            issues.push(Issue {
                severity: Severity::Critical,
                code: "reply_generic".into(),
                message: "Generic agreement reply — short replies get no algorithmic push in 2026, and the original author won't engage-back with them".into(),
                fix: Some("Add a specific observation, counter-point, or question tied to the post's content".into()),
            });
            score -= 25;
        }
        // Too short to be substantive (< 25 chars is almost never a meaningful reply)
        else if features.char_count < 25 {
            issues.push(Issue {
                severity: Severity::Warning,
                code: "reply_too_short".into(),
                message: format!(
                    "Reply is {} chars — 2026 update: short replies get no push and rarely earn a reply-back (the ~150x signal)",
                    features.char_count
                ),
                fix: Some("Expand to 1-2 sentences with a specific detail the author can respond to".into()),
            });
            score -= 15;
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

    // --- Long-form (note tweet / Article candidate) heuristics ---
    // Triggered above 500 chars. Single long-form posts now get a distribution
    // edge over multi-tweet threads (algo doc 06 §7, OpenTweet 2026), and X is
    // actively boosting Articles (Jan 2026 $1M Article Contest, won by data-dense
    // investigative pieces — @beaverd, @KobeissiLetter, @thedankoe). The dwell
    // band of 500–2000 chars is the empirical sweet spot.
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
                    "{} chars — past the 500–2000 dwell sweet spot. Still works as a note tweet, but consider publishing as an Article (boosted in 2026, won the $1M contest)",
                    features.char_count
                ),
                fix: Some("Trim to <=2000 chars, or publish via the Articles feature for the boost + cover image preview-card".into()),
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
                fix: Some("Split into 2 long-form posts 2h apart, or publish as a native Article (Premium feature, currently boosted)".into()),
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
            });
            score -= 5;
        }
    }

    // --- Sentiment check ---
    if features.sentiment == "negative" {
        issues.push(Issue {
            severity: Severity::Warning,
            code: "negative_sentiment".into(),
            message: "Combative or negative tone — Grok predicts P(block) and P(mute) and suppresses pre-emptively".into(),
            fix: Some("Reframe constructively — critique the idea, not the person".into()),
        });
        score -= 15;
    } else if features.sentiment == "mixed" {
        issues.push(Issue {
            severity: Severity::Info,
            code: "mixed_sentiment".into(),
            message: "Mildly negative language detected — may elevate P(mute) prediction".into(),
            fix: Some(
                "Consider softening — the algorithm penalises predicted negative reactions".into(),
            ),
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
        let bait = ["like if", "rt if", "follow for"];
        if bait.iter().any(|b| lower.contains(b)) {
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
    let char_count = text.chars().count();
    let word_count = text.split_whitespace().count();
    let line_count = text.lines().count();

    let has_link = text.contains("http://") || text.contains("https://");
    let link_position = if has_link { Some("body".into()) } else { None };

    let hashtag_count = text.matches('#').count();
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
                    "Add a question — questions are the #1 driver of replies (~20x weight)".into(),
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
                "Add practical value (how-to, data, framework) — it drives DM shares (~25x weight)".into(),
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
            "This looks DM-shareable — insider data and how-tos drive share_via_dm (~25x signal)"
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

    #[test]
    fn link_in_body_detected() {
        let result = analyze("Check this out https://example.com", &default_ctx());
        assert!(result.issues.iter().any(|i| i.code == "link_in_body"));
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
    fn link_in_body_is_critical() {
        let result = analyze(
            "Great article https://example.com about Rust",
            &default_ctx(),
        );
        let issue = result
            .issues
            .iter()
            .find(|i| i.code == "link_in_body")
            .unwrap();
        assert_eq!(issue.severity, Severity::Critical);
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
}
