use std::borrow::Cow;

/// Truncate a string at a safe UTF-8 boundary.
/// Returns at most `max_chars` characters (not bytes).
pub fn safe_truncate(s: &str, max_chars: usize) -> &str {
    match s.char_indices().nth(max_chars) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}

/// Shorten to at most `max_chars` characters, ending in "..." when cut.
pub fn ellipsize(s: &str, max_chars: usize) -> Cow<'_, str> {
    if s.chars().nth(max_chars).is_none() {
        return Cow::Borrowed(s);
    }
    Cow::Owned(format!("{}...", safe_truncate(s, max_chars.saturating_sub(3))))
}

/// Hosts whose `/status/<id>` links name an X post (x.com, twitter.com and
/// the embed mirrors people paste), including subdomains such as `mobile.`.
const POST_HOSTS: [&str; 6] = ["x.com", "twitter.com", "fxtwitter.com", "fixupx.com", "vxtwitter.com", "fixvx.com"];

/// Split `https://host/path?query#frag` into a known X host's path segments.
fn x_path(input: &str) -> Option<Vec<&str>> {
    let rest = input.split_once("://").map_or(input, |(_, r)| r);
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let host = host.to_ascii_lowercase();
    let host = host.rsplit_once('@').map_or(host.as_str(), |(_, h)| h);
    let known = POST_HOSTS
        .iter()
        .any(|h| host == *h || host.strip_suffix(h).is_some_and(|sub| sub.ends_with('.')));
    if !known {
        return None;
    }
    let path = path.split(['?', '#']).next().unwrap_or("");
    Some(path.split('/').filter(|s| !s.is_empty()).collect())
}

fn is_id(s: &str) -> bool {
    (1..=20).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit())
}

/// A post ID from a raw ID or a post URL: `/status/<id>`, `/statuses/<id>`,
/// `/i/web/status/<id>`, with or without scheme, query or trailing
/// `/photo/1`. `None` when the input isn't a post reference.
pub fn tweet_id(input: &str) -> Option<String> {
    let input = input.trim();
    if is_id(input) {
        return Some(input.to_string());
    }
    let segs = x_path(input)?;
    segs.windows(2)
        .find(|w| matches!(w[0], "status" | "statuses") && is_id(w[1]))
        .map(|w| w[1].to_string())
}

/// Paths on x.com that aren't user profiles.
const RESERVED_PATHS: [&str; 12] = [
    "i", "home", "search", "explore", "settings", "messages", "notifications", "intent", "share", "hashtag",
    "compose", "login",
];

/// A username from `@name`, `name`, or a profile/post URL. X usernames are
/// 1-15 letters, digits or underscores. `None` otherwise.
pub fn handle(input: &str) -> Option<String> {
    let input = input.trim();
    let name = match x_path(input) {
        Some(segs) => *segs.first().filter(|s| !RESERVED_PATHS.contains(&s.to_ascii_lowercase().as_str()))?,
        None => input.strip_prefix('@').unwrap_or(input),
    };
    let valid = (1..=15).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
    valid.then(|| name.to_string())
}

/// X's t.co length: every URL counts as this many characters.
const URL_WEIGHT: usize = 23;

/// Code points X counts as 1; everything else counts 2 (twitter-text v3).
fn light(c: char) -> bool {
    matches!(c as u32, 0x0000..=0x10FF | 0x2000..=0x200D | 0x2010..=0x201F | 0x2032..=0x2037)
}

fn is_regional_indicator(c: char) -> bool {
    ('\u{1F1E6}'..='\u{1F1FF}').contains(&c)
}

/// Characters that extend an emoji: variation selector, keycap, skin tones, tags.
fn is_emoji_modifier(c: char) -> bool {
    matches!(c as u32, 0xFE0F | 0x20E3 | 0x1F3FB..=0x1F3FF | 0xE0020..=0xE007F)
}

/// Pictographic code points that start an emoji sequence.
fn is_pictographic(c: char) -> bool {
    matches!(c as u32, 0x2190..=0x21FF | 0x2300..=0x23FF | 0x2460..=0x27BF | 0x2900..=0x297F | 0x2B00..=0x2BFF
        | 0x3030 | 0x303D | 0x3297 | 0x3299 | 0x1F000..=0x1FAFF)
}

const URL_TLDS: [&str; 30] = [
    "com", "net", "org", "io", "ai", "co", "dev", "app", "me", "xyz", "info", "bio", "news", "gg", "tv", "sh", "ly",
    "link", "page", "site", "tech", "blog", "us", "uk", "ca", "de", "fr", "eu", "in", "au",
];

/// Byte length of a URL starting `s`: `http(s)://…`, or a bare `domain.tld[/…]`
/// with a common TLD. Trailing punctuation isn't part of the URL.
fn url_len(s: &str) -> Option<usize> {
    let end = s.find(char::is_whitespace).unwrap_or(s.len());
    let token = s[..end].trim_end_matches(['.', ',', ';', ':', '!', '?', ')', '"', '\'', ']']);
    let lower = token.to_ascii_lowercase();
    if let Some(rest) = lower.strip_prefix("https://").or_else(|| lower.strip_prefix("http://")) {
        return rest.contains('.').then_some(token.len());
    }
    let host = lower.split(['/', '?', '#']).next().unwrap_or("");
    let labels: Vec<&str> = host.split('.').collect();
    let ok = labels.len() >= 2
        && labels.iter().all(|l| !l.is_empty() && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
        && URL_TLDS.contains(labels.last().unwrap_or(&""));
    ok.then_some(token.len())
}

/// Length of a post as X counts it against the 280 limit (twitter-text v3):
/// URLs count 23, an emoji sequence (ZWJ, skin tone, flag, keycap) counts 2,
/// CJK and other code points outside the Latin/punctuation ranges count 2,
/// the rest count 1. X also applies Unicode NFC first; this skips it, so
/// decomposed accents can over-count (never under-count).
pub fn weighted_len(text: &str) -> usize {
    let mut total = 0;
    let mut i = 0;
    let mut at_word_start = true;
    while i < text.len() {
        let rest = &text[i..];
        if at_word_start {
            if let Some(n) = url_len(rest) {
                total += URL_WEIGHT;
                i += n;
                at_word_start = false;
                continue;
            }
        }
        let mut chars = rest.chars();
        let c = chars.next().unwrap_or_default();
        let mut used = c.len_utf8();
        let next = chars.clone().next();
        let emoji = if is_regional_indicator(c) && next.is_some_and(is_regional_indicator) {
            used += next.map_or(0, char::len_utf8);
            true
        } else if is_pictographic(c) || next.is_some_and(is_emoji_modifier) {
            // Swallow modifiers and ZWJ-joined parts: the whole sequence is one emoji.
            let mut tail = rest[used..].chars().peekable();
            while let Some(&n) = tail.peek() {
                if is_emoji_modifier(n) {
                    used += n.len_utf8();
                    tail.next();
                } else if n == '\u{200D}' {
                    used += n.len_utf8();
                    tail.next();
                    if let Some(joined) = tail.next() {
                        used += joined.len_utf8();
                    }
                } else {
                    break;
                }
            }
            used > c.len_utf8() || is_pictographic(c)
        } else {
            false
        };
        total += if emoji || !light(c) { 2 } else { 1 };
        at_word_start = c.is_whitespace() || matches!(c, '(' | '[' | '"' | '\'');
        i += used;
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tweet_id_forms() {
        let id = Some("1234567890".to_string());
        assert_eq!(tweet_id("1234567890"), id);
        assert_eq!(tweet_id(" 1234567890\n"), id);
        assert_eq!(tweet_id("https://x.com/user/status/1234567890"), id);
        assert_eq!(tweet_id("https://twitter.com/user/status/1234567890?s=20&t=abc"), id);
        assert_eq!(tweet_id("https://x.com/user/status/1234567890/photo/1"), id);
        assert_eq!(tweet_id("https://mobile.twitter.com/user/statuses/1234567890"), id);
        assert_eq!(tweet_id("https://x.com/i/web/status/1234567890"), id);
        assert_eq!(tweet_id("x.com/user/status/1234567890#m"), id);
        assert_eq!(tweet_id("https://fxtwitter.com/user/status/1234567890"), id);
        assert_eq!(tweet_id("HTTPS://X.COM/user/status/1234567890"), id);
    }

    #[test]
    fn tweet_id_rejects_non_posts() {
        assert_eq!(tweet_id("https://x.com/user"), None);
        assert_eq!(tweet_id("https://dropbox.com/user/status/123"), None);
        assert_eq!(tweet_id("https://x.com/user/status/abc"), None);
        assert_eq!(tweet_id("hello"), None);
        assert_eq!(tweet_id(""), None);
        assert_eq!(tweet_id("123456789012345678901"), None);
    }

    #[test]
    fn handle_forms() {
        let h = Some("longevityboris".to_string());
        assert_eq!(handle("longevityboris"), h);
        assert_eq!(handle("@longevityboris"), h);
        assert_eq!(handle("https://x.com/longevityboris"), h);
        assert_eq!(handle("https://twitter.com/longevityboris/status/1"), h);
        assert_eq!(handle("x.com/longevityboris?lang=en"), h);
        assert_eq!(handle("https://x.com/i/web/status/1"), None);
        assert_eq!(handle("https://x.com/home"), None);
        assert_eq!(handle("two words"), None);
        assert_eq!(handle("@"), None);
        assert_eq!(handle("this_is_sixteen_"), None);
    }

    #[test]
    fn ellipsize_cuts_on_chars() {
        assert_eq!(ellipsize("short", 10), "short");
        assert_eq!(ellipsize("exactly10!", 10), "exactly10!");
        assert_eq!(ellipsize("hello world", 8), "hello...");
        assert_eq!(ellipsize("日本語のテキスト", 5), "日本...");
    }

    // Cases follow X's "Counting characters" rules and twitter-text v3 config:
    // weight 1 for U+0000-10FF, 2000-200D, 2010-201F, 2032-2037; 2 otherwise;
    // URLs 23; emoji sequences 2.
    #[test]
    fn weighted_len_latin_and_cjk() {
        assert_eq!(weighted_len(""), 0);
        assert_eq!(weighted_len("Hello world"), 11);
        assert_eq!(weighted_len(&"a".repeat(280)), 280);
        assert_eq!(weighted_len("café"), 4);
        assert_eq!(weighted_len("Привет"), 6); // Cyrillic is in the 1-weight range
        assert_eq!(weighted_len("“quoted” — dash…"), 17); // U+201C, 201D, 2014 weigh 1; U+2026 weighs 2
        assert_eq!(weighted_len("これは日本語"), 12);
        assert_eq!(weighted_len(&"漢".repeat(140)), 280);
        assert_eq!(weighted_len("한국어"), 6);
    }

    #[test]
    fn weighted_len_emoji_sequences_count_two() {
        assert_eq!(weighted_len("👾"), 2);
        assert_eq!(weighted_len("🙋🏽"), 2); // skin tone
        assert_eq!(weighted_len("👨‍🎤"), 2); // ZWJ
        assert_eq!(weighted_len("👨‍👩‍👧‍👦"), 2); // family ZWJ sequence
        assert_eq!(weighted_len("🇺🇸"), 2); // flag
        assert_eq!(weighted_len("❤️"), 2); // with variation selector
        assert_eq!(weighted_len("1️⃣"), 2); // keycap
        assert_eq!(weighted_len("🏴󠁧󠁢󠁳󠁣󠁴󠁿"), 2); // tag sequence
        assert_eq!(weighted_len("Hello 👋 world"), 14);
        assert_eq!(weighted_len("🔥🔥🔥"), 6);
    }

    #[test]
    fn weighted_len_urls_count_23() {
        assert_eq!(weighted_len("https://x.com"), 23);
        let long = format!("https://example.com/{}", "a".repeat(200));
        assert_eq!(weighted_len(&long), 23);
        assert_eq!(weighted_len("read https://example.com/a?b=1."), 5 + 23 + 1);
        assert_eq!(weighted_len("see example.com/page"), 4 + 23);
        assert_eq!(weighted_len("(https://a.io)"), 1 + 23 + 1);
        assert_eq!(weighted_len("e.g. Node.js"), 12);
        assert_eq!(weighted_len("me@example.com"), 14); // not a URL start
        assert_eq!(weighted_len("https://a.io https://b.io"), 23 + 1 + 23);
    }

    #[test]
    fn weighted_len_routes_long_posts() {
        // 270 chars + a space + a 60-char URL weigh 294; 150 CJK chars weigh 300.
        let text = format!("{} https://example.com/{}", "a".repeat(270), "p".repeat(40));
        assert_eq!(weighted_len(&text), 294);
        assert_eq!(weighted_len(&"字".repeat(150)), 300);
    }
}
