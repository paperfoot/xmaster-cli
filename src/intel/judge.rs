//! JEV judgments that decide what an agent needs to see: which feed posts are
//! worth a reply, which inbox items need an answer, which thread replies are
//! worth reading, and whether a draft reads as generic, AI slop, off-topic or
//! bait.
//!
//! The questions live in `judge_prompts.json`, exported verbatim from the eval
//! harness; the weights and thresholds below were measured with exactly those
//! questions on `jev-1.13.0`. Everything here is advisory: each function
//! returns `None` when JEV is unavailable and the caller keeps its heuristic.

use crate::config::AppConfig;
use crate::providers::jev::{Answers, Jev};
use serde::Serialize;
use serde_json::{json, Value};
use std::sync::OnceLock;

/// Most items judged per command; the rest pass through unjudged.
pub const MAX_ITEMS: usize = 40;
/// Feed posts below this pick score are dropped (kept 24/24 strong targets
/// and cut 59/66 poor ones in the eval).
pub const PICK_FLOOR: f64 = 0.35;
/// A draft flag fires above this probability (1 false warning in 30 good
/// replies, 40/42 bad drafts caught in the eval).
const DRAFT_FLAG_AT: f64 = 0.7;

fn questions(set: &str) -> &'static Value {
    static PROMPTS: OnceLock<Value> = OnceLock::new();
    &PROMPTS.get_or_init(|| {
        serde_json::from_str(include_str!("judge_prompts.json")).expect("judge_prompts.json is valid")
    })[set]
}

fn handle(author: &str) -> String {
    format!("@{}", author.trim_start_matches('@'))
}

/// Who the judgments are for: `account.bio` and `niche.topics`, else the
/// owner's recent posts from the local DB. None when there is nothing.
pub fn owner(config: &AppConfig) -> Option<Value> {
    owner_on(config, &config.niche.topic_list())
}

/// [`owner`] with `topics` in place of `niche.topics`: a feed's topics named
/// on the command line.
pub fn owner_on(config: &AppConfig, topics: &[String]) -> Option<Value> {
    let mut owner = serde_json::Map::new();
    let bio = config.account.bio.trim();
    if !bio.is_empty() {
        owner.insert("bio".into(), bio.into());
    }
    if !topics.is_empty() {
        owner.insert("topics".into(), json!(topics));
    }
    if owner.is_empty() {
        let posts = recent_own_posts();
        if !posts.is_empty() {
            owner.insert("recent_posts".into(), json!(posts));
        }
    }
    (!owner.is_empty()).then_some(Value::Object(owner))
}

fn recent_own_posts() -> Vec<String> {
    let Ok(conn) = crate::intel::open_db("xmaster.db") else {
        return Vec::new();
    };
    conn.prepare(
        "SELECT text FROM posts WHERE content_type IN ('text','reply','media','quote')
         AND length(text) >= 60 AND text NOT LIKE 'test%' ORDER BY id DESC LIMIT 8",
    )
    .and_then(|mut stmt| {
        stmt.query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()
    })
    .map(|texts| {
        texts
            .iter()
            .map(|t| crate::utils::safe_truncate(t, 280).to_string())
            .collect()
    })
    .unwrap_or_default()
}

/// `None` when every request failed (JEV down), else per-item results.
fn any_answered<T>(answers: Vec<Option<Answers>>, f: impl Fn(&Answers) -> Option<T>) -> Option<Vec<Option<T>>> {
    if answers.iter().all(Option::is_none) {
        return None;
    }
    Some(answers.iter().map(|a| a.as_ref().and_then(&f)).collect())
}

// ---------------------------------------------------------------------------
// Reply-target picker
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Pick {
    pub score: f64,
    pub why: String,
}

/// Judge `(author, text)` posts as reply targets for `owner`. `on_topic`
/// means the caller matched every post to a topic it named, and that match
/// replaces the niche gate: a bio from another field held `on_niche` at
/// 0.2-0.4 on posts squarely about the named topic (live, 26 Oct).
pub async fn rank_posts(jev: &Jev, owner: &Value, posts: &[(&str, &str)], on_topic: bool) -> Option<Vec<Option<Pick>>> {
    let states = posts
        .iter()
        .map(|(author, text)| json!({"owner": owner, "post": {"author": handle(author), "text": text}}))
        .collect();
    let mut qs = questions("picker").clone();
    if let Some(q) = qs.as_object_mut().filter(|_| on_topic) {
        q.remove("on_niche");
    }
    any_answered(jev.ask_all(states, &qs).await, |a| pick(a, on_topic))
}

fn pick(a: &Answers, on_topic: bool) -> Option<Pick> {
    let niche = if on_topic { 1.0 } else { a.noul("on_niche")? };
    let gate = niche
        * (1.0 - a.noul("promo")?)
        * (1.0 - a.noul("unwelcome")?)
        * a.noul("english")?;
    let (add, invites, target) = (a.score("can_add")?, a.noul("invites")?, a.score("target")?);
    let mut why = Vec::new();
    if add >= 1.4 {
        why.push("you can add specifics from your own work");
    } else if add >= 0.9 {
        why.push("room for your view");
    }
    if invites >= 0.6 {
        why.push("invites replies");
    }
    Some(Pick {
        score: gate * (0.4 * add / 2.0 + 0.3 * invites + 0.3 * target / 2.0),
        why: why.join("; "),
    })
}

// ---------------------------------------------------------------------------
// Inbox triage
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Triage {
    Reply,
    Like,
    Ignore,
    Spam,
}

/// Decide what `owner` should do with each `(parent_text, author, text)`.
pub async fn triage(jev: &Jev, owner: &Value, items: &[(&str, &str, &str)]) -> Option<Vec<Option<Triage>>> {
    let states = items
        .iter()
        .map(|(parent, author, text)| {
            json!({"owner": owner, "parent": parent, "reply": {"author": handle(author), "text": text}})
        })
        .collect();
    any_answered(jev.ask_all(states, questions("inbox")).await, triage_one)
}

fn triage_one(a: &Answers) -> Option<Triage> {
    if a.noul("spam")? > 0.5 {
        return Some(Triage::Spam);
    }
    let (reply, like, ignore) = (
        a.prob("triage", "reply")?,
        a.prob("triage", "like")?,
        a.prob("triage", "ignore")?,
    );
    Some(if reply >= like && reply >= ignore {
        Triage::Reply
    } else if like >= ignore {
        Triage::Like
    } else {
        Triage::Ignore
    })
}

// ---------------------------------------------------------------------------
// Thread trimming
// ---------------------------------------------------------------------------

/// Whether each `(author, text)` reply under `parent` is worth reading.
pub async fn worth_reading(jev: &Jev, parent: &str, replies: &[(&str, &str)]) -> Option<Vec<Option<bool>>> {
    let states = replies
        .iter()
        .map(|(author, text)| json!({"parent": parent, "reply": {"author": handle(author), "text": text}}))
        .collect();
    any_answered(jev.ask_all(states, questions("thread")).await, |a| {
        Some(a.noul("substance")? * (1.0 - a.noul("spam")?) >= 0.5)
    })
}

// ---------------------------------------------------------------------------
// Draft guard
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct DraftFlag {
    pub code: &'static str,
    pub p: f64,
    pub fix: &'static str,
}

const FLAGS: [(&str, &str); 4] = [
    ("generic", "Name a specific detail from the post and add your own fact, number or experience, or ask the author something they can answer"),
    ("slop", "Rewrite plainly: one concrete point, no hype words, emoji decoration or hashtags"),
    ("off_topic", "Answer what the post actually says, or post this as a standalone"),
    ("bait", "Drop the ask for likes, follows or reposts, the plug or the insult; people mute these"),
];

/// Flags for a draft; `target` is the text of the post being replied to.
/// Without a target only the slop and bait checks apply.
pub async fn guard_draft(jev: &Jev, draft: &str, target: Option<&str>) -> Option<Vec<DraftFlag>> {
    let all = questions("draft");
    let (state, qs) = match target {
        Some(target) => (json!({"draft": draft, "target": target}), all.clone()),
        None => (json!({"draft": draft}), json!({"slop": all["slop"], "bait": all["bait"]})),
    };
    let answers = jev.ask(&state, &qs).await?;
    Some(
        FLAGS
            .iter()
            .filter_map(|&(code, fix)| {
                let p = answers.noul(code)?;
                (p > DRAFT_FLAG_AT).then(|| DraftFlag { code, p: (p * 100.0).round() / 100.0, fix })
            })
            .collect(),
    )
}

// ---------------------------------------------------------------------------
// Reach signal beside the preflight score
// ---------------------------------------------------------------------------

/// JEV's 0-1 read of how much attention `post` will get: hook strength,
/// engagement pull and expected reach, averaged. On the owner's organic
/// posts it tracked reach better than the rule score (Spearman +0.46 vs
/// +0.31, n=63) and adds to it; it says nothing about promoted posts.
pub async fn reach(jev: &Jev, post: &str) -> Option<f64> {
    let a = jev.ask(&json!({"post": post}), questions("quality")).await?;
    let q = (a.score("hook")? / 3.0 + a.score("engage")? / 2.0 + a.score("reach")? / 2.0) / 3.0;
    Some((q * 100.0).round() / 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_sets_are_complete() {
        let ids = |set: &str| -> Vec<String> { questions(set).as_object().unwrap().keys().cloned().collect() };
        assert_eq!(
            ids("picker"),
            ["can_add", "english", "invites", "on_niche", "promo", "target", "unwelcome"]
        );
        assert_eq!(ids("inbox"), ["spam", "triage"]);
        assert_eq!(ids("thread"), ["spam", "substance"]);
        assert_eq!(ids("draft"), ["bait", "generic", "off_topic", "slop"]);
        assert_eq!(ids("quality"), ["engage", "hook", "reach"]);
        for (code, _) in FLAGS {
            assert_eq!(questions("draft")[code]["type"], "noul");
        }
        let options: Vec<&String> = questions("inbox")["triage"]["criteria"].as_object().unwrap().keys().collect();
        assert_eq!(options, ["ignore", "like", "reply", "spam"]);
    }

    fn answers(v: Value) -> Answers {
        serde_json::from_value::<serde_json::Map<String, Value>>(v).map(Answers::from).unwrap()
    }

    #[test]
    fn pick_gates_off_niche_and_explains() {
        let strong = answers(json!({
            "on_niche": {"noul": 0.95}, "promo": {"noul": 0.02}, "unwelcome": {"noul": 0.01},
            "english": {"noul": 0.99}, "can_add": {"score": 1.8}, "invites": {"noul": 0.8}, "target": {"score": 1.7}
        }));
        let p = pick(&strong, false).unwrap();
        assert!(p.score > PICK_FLOOR);
        assert_eq!(p.why, "you can add specifics from your own work; invites replies");
        let off = answers(json!({
            "on_niche": {"noul": 0.05}, "promo": {"noul": 0.02}, "unwelcome": {"noul": 0.01},
            "english": {"noul": 0.99}, "can_add": {"score": 1.8}, "invites": {"noul": 0.8}, "target": {"score": 1.7}
        }));
        assert!(pick(&off, false).unwrap().score < PICK_FLOOR);
    }

    #[test]
    fn named_topic_match_stands_in_for_the_niche_gate() {
        // Live answers for "junior ships a feature with claude code" judged
        // for a longevity bio, asked without on_niche.
        let a = answers(json!({
            "promo": {"noul": 0.08}, "unwelcome": {"noul": 0.06}, "english": {"noul": 0.99},
            "can_add": {"score": 1.08}, "invites": {"noul": 0.89}, "target": {"score": 1.21}
        }));
        assert!(pick(&a, true).unwrap().score > PICK_FLOOR);
        assert!(pick(&a, false).is_none(), "the configured niche still needs on_niche");
        let promo = answers(json!({
            "promo": {"noul": 0.77}, "unwelcome": {"noul": 0.03}, "english": {"noul": 0.99},
            "can_add": {"score": 0.99}, "invites": {"noul": 0.81}, "target": {"score": 1.12}
        }));
        assert!(pick(&promo, true).unwrap().score < PICK_FLOOR, "promo still drops");
    }

    #[test]
    fn named_topics_replace_the_configured_niche() {
        let mut config = AppConfig::default();
        config.account.bio = "Longevity founder".into();
        config.niche.topics = "longevity science,aging biology".into();
        let named = owner_on(&config, &["Claude Code".to_string()]).unwrap();
        assert_eq!(named, json!({"bio": "Longevity founder", "topics": ["Claude Code"]}));
        assert_eq!(owner(&config).unwrap()["topics"], json!(["longevity science", "aging biology"]));
    }

    #[test]
    fn triage_spam_gate_then_choice() {
        let t = |spam: f64, reply: f64, like: f64, ignore: f64| {
            triage_one(&answers(json!({
                "spam": {"noul": spam},
                "triage": {"probabilities": {"reply": reply, "like": like, "ignore": ignore, "spam": 0.0}}
            })))
        };
        assert_eq!(t(0.9, 0.9, 0.05, 0.05), Some(Triage::Spam));
        assert_eq!(t(0.1, 0.6, 0.3, 0.1), Some(Triage::Reply));
        assert_eq!(t(0.1, 0.2, 0.7, 0.1), Some(Triage::Like));
        assert_eq!(t(0.1, 0.1, 0.2, 0.7), Some(Triage::Ignore));
        assert_eq!(triage_one(&answers(json!({"spam": {"noul": 0.1}}))), None);
    }

    #[test]
    fn all_failed_means_unavailable() {
        assert!(any_answered(vec![None, None], |_| Some(1)).is_none());
        let some = any_answered(vec![None, Some(answers(json!({})))], |_| Some(1)).unwrap();
        assert_eq!(some, vec![None, Some(1)]);
    }
}
