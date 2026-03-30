# Discovered Posts Library — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Cache every external post xmaster fetches into a local `discovered_posts` SQLite table, building a personal viral posts library from normal usage — zero extra API calls.

**Architecture:** Add a `discovered_posts` table to the existing IntelStore (same `xmaster.db`). Implement `record_discovered_posts(&self, source, &[TweetData])` with UPSERT semantics (preserve first_discovered_at, update metrics on re-encounter). Tap into 6 commands (search, timeline, mentions, read, engage recommend, engage feed) at the exact point where `Vec<TweetData>` is materialized but before it's consumed by display logic. Skip `search-ai`/`trending` for now (they return text + citations, not TweetData — would require extra API calls to hydrate).

**Tech Stack:** Rust, rusqlite, serde_json (for JSON columns), chrono (timestamps)

---

### Task 1: Add discovered_posts table schema

**Files:**
- Modify: `src/intel/store.rs:180-254` (init_tables method)
- Modify: `src/intel/store.rs:1-6` (imports)

- [ ] **Step 1: Add the CREATE TABLE SQL to init_tables()**

In `src/intel/store.rs`, inside `init_tables()`, add after the `timing_stats` table (after line 253, before the closing `"` of `execute_batch`):

```rust
            CREATE TABLE IF NOT EXISTS discovered_posts (
                id                     INTEGER PRIMARY KEY AUTOINCREMENT,
                tweet_id               TEXT UNIQUE NOT NULL,
                text                   TEXT NOT NULL,
                author_id              TEXT,
                author_username        TEXT,
                created_at             TEXT,
                conversation_id        TEXT,
                referenced_tweets_json TEXT NOT NULL DEFAULT '[]',
                like_count             INTEGER,
                retweet_count          INTEGER,
                reply_count            INTEGER,
                impression_count       INTEGER,
                bookmark_count         INTEGER,
                author_followers       INTEGER,
                media_urls_json        TEXT NOT NULL DEFAULT '[]',
                first_source           TEXT NOT NULL,
                last_source            TEXT NOT NULL,
                first_discovered_at    INTEGER NOT NULL,
                last_seen_at           INTEGER NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_discovered_author
                ON discovered_posts(author_username);
            CREATE INDEX IF NOT EXISTS idx_discovered_last_seen
                ON discovered_posts(last_seen_at DESC);
            CREATE INDEX IF NOT EXISTS idx_discovered_impressions
                ON discovered_posts(impression_count DESC);
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check 2>&1 | head -5`
Expected: warnings only, no errors

- [ ] **Step 3: Commit**

```bash
git add src/intel/store.rs
git commit -m "Add discovered_posts table schema to IntelStore"
```

---

### Task 2: Implement record_discovered_posts() method

**Files:**
- Modify: `src/intel/store.rs` (add method after existing record methods, ~line 500+)

- [ ] **Step 1: Add the record methods to IntelStore impl**

Add these methods inside `impl IntelStore { ... }`, after the existing `record_published_post` method:

```rust
    /// Cache external posts encountered during search/timeline/read commands.
    /// Uses UPSERT: first encounter preserves source/timestamp, re-encounters update metrics.
    pub fn record_discovered_posts(
        &self,
        source: &str,
        tweets: &[crate::providers::xapi::TweetData],
    ) -> Result<(), rusqlite::Error> {
        if tweets.is_empty() {
            return Ok(());
        }
        let now = chrono::Utc::now().timestamp();
        let tx = self.conn.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO discovered_posts (
                    tweet_id, text, author_id, author_username, created_at,
                    conversation_id, referenced_tweets_json,
                    like_count, retweet_count, reply_count, impression_count,
                    bookmark_count, author_followers, media_urls_json,
                    first_source, last_source, first_discovered_at, last_seen_at
                ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?15,?16,?16)
                ON CONFLICT(tweet_id) DO UPDATE SET
                    text = excluded.text,
                    author_id = COALESCE(excluded.author_id, discovered_posts.author_id),
                    author_username = COALESCE(excluded.author_username, discovered_posts.author_username),
                    created_at = COALESCE(excluded.created_at, discovered_posts.created_at),
                    conversation_id = COALESCE(excluded.conversation_id, discovered_posts.conversation_id),
                    referenced_tweets_json = CASE
                        WHEN excluded.referenced_tweets_json <> '[]' THEN excluded.referenced_tweets_json
                        ELSE discovered_posts.referenced_tweets_json END,
                    like_count = COALESCE(excluded.like_count, discovered_posts.like_count),
                    retweet_count = COALESCE(excluded.retweet_count, discovered_posts.retweet_count),
                    reply_count = COALESCE(excluded.reply_count, discovered_posts.reply_count),
                    impression_count = COALESCE(excluded.impression_count, discovered_posts.impression_count),
                    bookmark_count = COALESCE(excluded.bookmark_count, discovered_posts.bookmark_count),
                    author_followers = COALESCE(excluded.author_followers, discovered_posts.author_followers),
                    media_urls_json = CASE
                        WHEN excluded.media_urls_json <> '[]' THEN excluded.media_urls_json
                        ELSE discovered_posts.media_urls_json END,
                    last_source = excluded.last_source,
                    last_seen_at = excluded.last_seen_at",
            )?;
            for t in tweets {
                let m = t.public_metrics.as_ref();
                let refs_json = serde_json::to_string(
                    &t.referenced_tweets.as_deref().unwrap_or(&[])
                ).unwrap_or_else(|_| "[]".into());
                let media_json = serde_json::to_string(&t.media_urls)
                    .unwrap_or_else(|_| "[]".into());
                stmt.execute(rusqlite::params![
                    t.id,
                    t.text,
                    t.author_id,
                    t.author_username,
                    t.created_at,
                    t.conversation_id,
                    refs_json,
                    m.map(|m| m.like_count as i64),
                    m.map(|m| m.retweet_count as i64),
                    m.map(|m| m.reply_count as i64),
                    m.map(|m| m.impression_count as i64),
                    m.map(|m| m.bookmark_count as i64),
                    t.author_followers.map(|f| f as i64),
                    media_json,
                    source,  // binds to both first_source (?15) and last_source (?15 reused)
                    now,     // binds to both first_discovered_at (?16) and last_seen_at (?16 reused)
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Cache a single discovered post. Convenience wrapper.
    pub fn record_discovered_post(
        &self,
        source: &str,
        tweet: &crate::providers::xapi::TweetData,
    ) -> Result<(), rusqlite::Error> {
        self.record_discovered_posts(source, std::slice::from_ref(tweet))
    }
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check 2>&1 | head -5`
Expected: warnings only, no errors

- [ ] **Step 3: Write a test**

Add to the existing `#[cfg(test)] mod tests` in `store.rs` (or create one if none exists near the bottom):

```rust
    #[test]
    fn discovered_posts_upsert() {
        let store = IntelStore::open_in_memory().unwrap_or_else(|_| {
            // Fallback: use temp file
            let dir = std::env::temp_dir().join("xmaster-test");
            std::fs::create_dir_all(&dir).ok();
            let conn = rusqlite::Connection::open(dir.join("test.db")).unwrap();
            conn.pragma_update(None, "journal_mode", "wal").unwrap();
            let s = IntelStore { conn };
            s.init_tables().unwrap();
            s
        });
        let tweet = crate::providers::xapi::TweetData {
            id: "123".into(),
            text: "Hello world".into(),
            author_id: Some("user1".into()),
            author_username: Some("testuser".into()),
            created_at: Some("2026-01-01T00:00:00Z".into()),
            conversation_id: None,
            referenced_tweets: None,
            public_metrics: Some(crate::providers::xapi::TweetMetrics {
                like_count: 10,
                retweet_count: 5,
                reply_count: 2,
                impression_count: 1000,
                bookmark_count: 1,
            }),
            author_followers: Some(500),
            media_urls: vec![],
        };
        // First insert
        store.record_discovered_post("search", &tweet).unwrap();
        let count: i64 = store.conn.query_row(
            "SELECT COUNT(*) FROM discovered_posts WHERE tweet_id = '123'", [], |r| r.get(0)
        ).unwrap();
        assert_eq!(count, 1);

        // Upsert: same tweet, different source — should update last_source
        store.record_discovered_post("timeline", &tweet).unwrap();
        let last_source: String = store.conn.query_row(
            "SELECT last_source FROM discovered_posts WHERE tweet_id = '123'", [], |r| r.get(0)
        ).unwrap();
        assert_eq!(last_source, "timeline");
        let first_source: String = store.conn.query_row(
            "SELECT first_source FROM discovered_posts WHERE tweet_id = '123'", [], |r| r.get(0)
        ).unwrap();
        assert_eq!(first_source, "search");
    }
```

Note: If `open_in_memory()` doesn't exist, we need a test helper. Check if the test module already has a way to create an in-memory store. If not, add a `#[cfg(test)]` constructor:

```rust
    #[cfg(test)]
    pub fn open_test() -> Result<Self, rusqlite::Error> {
        let conn = Connection::open_in_memory()?;
        let store = Self { conn };
        store.init_tables()?;
        Ok(store)
    }
```

- [ ] **Step 4: Run the test**

Run: `cargo test discovered_posts_upsert 2>&1 | tail -10`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/intel/store.rs
git commit -m "Implement record_discovered_posts() with UPSERT semantics"
```

---

### Task 3: Wire search.rs

**Files:**
- Modify: `src/commands/search.rs:1-6` (add import)
- Modify: `src/commands/search.rs:89` (add cache call after API result)

- [ ] **Step 1: Add import**

Add after existing imports at top of `search.rs`:
```rust
use crate::intel::store::IntelStore;
```

- [ ] **Step 2: Add cache call**

The `tweets` variable at line 89 is `Vec<TweetData>`. It's consumed by `into_iter()` at line 92. We need to cache BEFORE consumption. Change:

```rust
    let tweets = api.search_tweets_paginated(query, mode, count, start_time.as_deref(), end_time.as_deref()).await?;
    let display = SearchResults {
```

To:

```rust
    let tweets = api.search_tweets_paginated(query, mode, count, start_time.as_deref(), end_time.as_deref()).await?;
    if let Ok(store) = IntelStore::open() {
        let _ = store.record_discovered_posts("search", &tweets);
    }
    let display = SearchResults {
```

- [ ] **Step 3: Verify compilation**

Run: `cargo check 2>&1 | head -5`

- [ ] **Step 4: Commit**

```bash
git add src/commands/search.rs
git commit -m "Cache search results into discovered_posts library"
```

---

### Task 4: Wire timeline.rs (timeline + mentions)

**Files:**
- Modify: `src/commands/timeline.rs:1-6` (add import)
- Modify: `src/commands/timeline.rs:136` (cache after timeline fetch)
- Modify: `src/commands/timeline.rs:162` (cache after mentions fetch)

- [ ] **Step 1: Add import**

```rust
use crate::intel::store::IntelStore;
```

- [ ] **Step 2: Cache timeline results**

After line 136 (`None => api.get_home_timeline(count).await?,` — the end of the `let tweets = match` block), add before `let mut list = tweets_to_list(tweets);`:

```rust
    if let Ok(store) = IntelStore::open() {
        let _ = store.record_discovered_posts("timeline", &tweets);
    }
```

- [ ] **Step 3: Cache mentions results**

In `mentions()`, after line 162 (`let tweets = api.get_user_mentions_since(...).await?;`), add before the render call:

```rust
    if let Ok(store) = IntelStore::open() {
        let _ = store.record_discovered_posts("mentions", &tweets);
    }
```

- [ ] **Step 4: Verify compilation**

Run: `cargo check 2>&1 | head -5`

- [ ] **Step 5: Commit**

```bash
git add src/commands/timeline.rs
git commit -m "Cache timeline and mentions into discovered_posts library"
```

---

### Task 5: Wire read_post.rs

**Files:**
- Modify: `src/commands/read_post.rs:1-7` (add import)
- Modify: `src/commands/read_post.rs:54` (cache after single tweet fetch)

- [ ] **Step 1: Add import**

```rust
use crate::intel::store::IntelStore;
```

- [ ] **Step 2: Cache the read post**

After line 54 (`let tweet = api.get_tweet(&tweet_id).await?;`), add before the `let metrics` line:

```rust
    if let Ok(store) = IntelStore::open() {
        let _ = store.record_discovered_post("read", &tweet);
    }
```

- [ ] **Step 3: Verify compilation**

Run: `cargo check 2>&1 | head -5`

- [ ] **Step 4: Commit**

```bash
git add src/commands/read_post.rs
git commit -m "Cache read posts into discovered_posts library"
```

---

### Task 6: Wire engage_recommend.rs (recommend mentions + feed)

**Files:**
- Modify: `src/commands/engage_recommend.rs:128` (cache mentions in recommend)
- Modify: `src/commands/engage_recommend.rs:462` (cache feed results)

IntelStore is already imported in this file.

- [ ] **Step 1: Cache mentions in recommend()**

After line 128 (`if let Ok(mentions) = xapi.get_user_mentions(&user_id, 20).await {`), the mentions are iterated starting at line 129. Cache them inside the Ok branch, before the for loop:

Change:
```rust
        if let Ok(mentions) = xapi.get_user_mentions(&user_id, 20).await {
            for tweet in &mentions {
```

To:
```rust
        if let Ok(mentions) = xapi.get_user_mentions(&user_id, 20).await {
            if let Ok(store) = IntelStore::open() {
                let _ = store.record_discovered_posts("recommend_mentions", &mentions);
            }
            for tweet in &mentions {
```

- [ ] **Step 2: Cache feed results**

In `feed()`, after line 462 (end of the dedup loop `}`), add before `let now = chrono::Utc::now();` at line 464:

```rust
    if let Ok(store) = IntelStore::open() {
        let _ = store.record_discovered_posts("engage_feed", &tweets);
    }
```

Note: `tweets` is `Vec<TweetData>` at this point (deduped, not yet consumed — the consuming `for t in tweets` loop is at line 469). We need to pass `&tweets` here, but `tweets` is consumed at 469. Change `for t in tweets` at line 469 to `for t in &tweets` and adjust the field accesses (they already use `t.` — since TweetData fields are accessed by reference, check if this needs `.clone()` calls).

Actually, the simpler approach: just pass `&tweets` before the consuming loop. The `for t in tweets` at 469 takes ownership — that's fine, the store call borrows first.

```rust
    // Cache discovered posts before consuming
    if let Ok(store) = IntelStore::open() {
        let _ = store.record_discovered_posts("engage_feed", &tweets);
    }

    let now = chrono::Utc::now();
```

- [ ] **Step 3: Verify compilation**

Run: `cargo check 2>&1 | head -5`

- [ ] **Step 4: Commit**

```bash
git add src/commands/engage_recommend.rs
git commit -m "Cache engage recommend and feed results into discovered_posts library"
```

---

### Task 7: Add inspire command (query the library)

**Files:**
- Create: `src/commands/inspire.rs`
- Modify: `src/commands/mod.rs` (add module + dispatch)
- Modify: `src/cli.rs` (add CLI subcommand)

- [ ] **Step 1: Create inspire.rs**

```rust
use crate::context::AppContext;
use crate::errors::XmasterError;
use crate::intel::store::IntelStore;
use crate::output::{self, CsvRenderable, OutputFormat, Tableable};
use serde::Serialize;
use std::sync::Arc;

#[derive(Serialize)]
struct InspireResults {
    query: String,
    posts: Vec<InspireRow>,
}

#[derive(Serialize)]
struct InspireRow {
    id: String,
    author: String,
    text: String,
    likes: i64,
    impressions: i64,
    source: String,
    discovered: String,
}

impl Tableable for InspireResults {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["ID", "Author", "Text", "Likes", "Views", "Source"]);
        for p in &self.posts {
            table.add_row(vec![
                &p.id,
                &p.author,
                &if p.text.len() > 120 { format!("{}...", &p.text[..117]) } else { p.text.clone() },
                &p.likes.to_string(),
                &p.impressions.to_string(),
                &p.source,
            ]);
        }
        table
    }
}

impl CsvRenderable for InspireResults {
    fn csv_headers(&self) -> Vec<&str> {
        vec!["id", "author", "text", "likes", "impressions", "source", "discovered"]
    }
    fn csv_rows(&self) -> Vec<Vec<String>> {
        self.posts.iter().map(|p| vec![
            p.id.clone(), p.author.clone(), p.text.clone(),
            p.likes.to_string(), p.impressions.to_string(),
            p.source.clone(), p.discovered.clone(),
        ]).collect()
    }
}

pub async fn execute(
    _ctx: Arc<AppContext>,
    format: OutputFormat,
    topic: Option<&str>,
    author: Option<&str>,
    min_likes: Option<i64>,
    count: usize,
) -> Result<(), XmasterError> {
    let store = IntelStore::open()
        .map_err(|e| XmasterError::Config(format!("DB error: {e}")))?;

    let mut conditions = vec!["1=1".to_string()];
    let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

    if let Some(t) = topic {
        conditions.push(format!("(text LIKE ?{} OR author_username LIKE ?{})",
            param_values.len() + 1, param_values.len() + 2));
        param_values.push(Box::new(format!("%{t}%")));
        param_values.push(Box::new(format!("%{t}%")));
    }
    if let Some(a) = author {
        conditions.push(format!("author_username LIKE ?{}", param_values.len() + 1));
        param_values.push(Box::new(format!("%{a}%")));
    }
    if let Some(ml) = min_likes {
        conditions.push(format!("like_count >= ?{}", param_values.len() + 1));
        param_values.push(Box::new(ml));
    }

    let sql = format!(
        "SELECT tweet_id, author_username, text, COALESCE(like_count,0),
                COALESCE(impression_count,0), last_source, first_discovered_at
         FROM discovered_posts
         WHERE {}
         ORDER BY COALESCE(impression_count,0) DESC
         LIMIT ?{}",
        conditions.join(" AND "),
        param_values.len() + 1
    );
    param_values.push(Box::new(count as i64));

    let params_refs: Vec<&dyn rusqlite::types::ToSql> = param_values.iter().map(|b| b.as_ref()).collect();
    let mut stmt = store.query_raw(&sql, &params_refs)
        .map_err(|e| XmasterError::Config(format!("Query error: {e}")))?;

    // Actually, we need a simpler approach. Let's use conn directly via a public method.
    // Add a query helper to IntelStore instead.

    let display = InspireResults {
        query: topic.unwrap_or("all").to_string(),
        posts: stmt,
    };
    output::render_csv(format, &display, None);
    Ok(())
}
```

Actually, this approach with dynamic SQL params is getting complex. Simpler: add a dedicated `query_discovered_posts()` method to IntelStore that returns `Vec<InspireRow>` directly, with optional filters. This keeps SQLite access in the store layer.

**Revised step 1: Add query method to IntelStore**

In `src/intel/store.rs`, add:

```rust
    /// Query the discovered posts library with optional filters.
    pub fn query_discovered_posts(
        &self,
        topic: Option<&str>,
        author: Option<&str>,
        min_likes: Option<i64>,
        limit: usize,
    ) -> Result<Vec<DiscoveredPostRow>, rusqlite::Error> {
        let mut sql = String::from(
            "SELECT tweet_id, COALESCE(author_username,''), text,
                    COALESCE(like_count,0), COALESCE(impression_count,0),
                    last_source, first_discovered_at
             FROM discovered_posts WHERE 1=1"
        );
        if topic.is_some() { sql.push_str(" AND text LIKE '%' || ?1 || '%'"); }
        if author.is_some() { sql.push_str(&format!(" AND author_username LIKE '%' || ?{} || '%'", if topic.is_some() { 2 } else { 1 })); }
        if min_likes.is_some() {
            let idx = 1 + topic.is_some() as usize + author.is_some() as usize;
            sql.push_str(&format!(" AND like_count >= ?{idx}"));
        }
        let limit_idx = 1 + topic.is_some() as usize + author.is_some() as usize + min_likes.is_some() as usize;
        sql.push_str(&format!(" ORDER BY COALESCE(impression_count,0) DESC LIMIT ?{limit_idx}"));

        let mut stmt = self.conn.prepare(&sql)?;
        let mut idx = 1usize;
        if let Some(t) = topic { stmt.raw_bind_parameter(idx, t)?; idx += 1; }
        if let Some(a) = author { stmt.raw_bind_parameter(idx, a)?; idx += 1; }
        if let Some(ml) = min_likes { stmt.raw_bind_parameter(idx, ml)?; idx += 1; }
        stmt.raw_bind_parameter(idx, limit as i64)?;

        let rows = stmt.raw_query().mapped(|row| {
            Ok(DiscoveredPostRow {
                tweet_id: row.get(0)?,
                author_username: row.get(1)?,
                text: row.get(2)?,
                like_count: row.get(3)?,
                impression_count: row.get(4)?,
                last_source: row.get(5)?,
                first_discovered_at: row.get(6)?,
            })
        }).collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }
```

And add the row struct near the other types:

```rust
#[derive(Debug, Clone, Serialize)]
pub struct DiscoveredPostRow {
    pub tweet_id: String,
    pub author_username: String,
    pub text: String,
    pub like_count: i64,
    pub impression_count: i64,
    pub last_source: String,
    pub first_discovered_at: i64,
}
```

- [ ] **Step 2: Create the inspire command file**

Create `src/commands/inspire.rs`:

```rust
use crate::context::AppContext;
use crate::errors::XmasterError;
use crate::intel::store::IntelStore;
use crate::output::{self, CsvRenderable, OutputFormat, Tableable};
use serde::Serialize;
use std::sync::Arc;

#[derive(Serialize)]
struct InspireResults {
    query: String,
    count: usize,
    posts: Vec<InspireRow>,
}

#[derive(Serialize)]
struct InspireRow {
    id: String,
    author: String,
    text: String,
    likes: i64,
    impressions: i64,
    source: String,
}

impl Tableable for InspireResults {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["ID", "Author", "Text", "Likes", "Views", "Via"]);
        for p in &self.posts {
            let truncated = if p.text.len() > 120 {
                format!("{}...", &p.text[..p.text.floor_char_boundary(117)])
            } else {
                p.text.clone()
            };
            table.add_row(vec![
                &p.id, &p.author, &truncated,
                &p.likes.to_string(), &p.impressions.to_string(), &p.source,
            ]);
        }
        table
    }
}

impl CsvRenderable for InspireResults {
    fn csv_headers(&self) -> Vec<&str> {
        vec!["id", "author", "text", "likes", "impressions", "source"]
    }
    fn csv_rows(&self) -> Vec<Vec<String>> {
        self.posts.iter().map(|p| vec![
            p.id.clone(), p.author.clone(), p.text.clone(),
            p.likes.to_string(), p.impressions.to_string(), p.source.clone(),
        ]).collect()
    }
}

pub async fn execute(
    _ctx: Arc<AppContext>,
    format: OutputFormat,
    topic: Option<&str>,
    author: Option<&str>,
    min_likes: Option<i64>,
    count: usize,
) -> Result<(), XmasterError> {
    let store = IntelStore::open()
        .map_err(|e| XmasterError::Config(format!("DB error: {e}")))?;
    let rows = store.query_discovered_posts(topic, author, min_likes, count)
        .map_err(|e| XmasterError::Config(format!("Query error: {e}")))?;

    if rows.is_empty() {
        return Err(XmasterError::NotFound(
            "No posts in library yet. Run `xmaster search`, `xmaster timeline`, or `xmaster read` to build your library.".into()
        ));
    }

    let display = InspireResults {
        query: topic.unwrap_or("all").to_string(),
        count: rows.len(),
        posts: rows.into_iter().map(|r| InspireRow {
            id: r.tweet_id,
            author: if r.author_username.is_empty() { "?".into() } else { format!("@{}", r.author_username) },
            text: r.text,
            likes: r.like_count,
            impressions: r.impression_count,
            source: r.last_source,
        }).collect(),
    };
    output::render_csv(format, &display, None);
    Ok(())
}
```

- [ ] **Step 3: Register in mod.rs and cli.rs**

In `src/commands/mod.rs`, add `pub mod inspire;` and dispatch:
```rust
Commands::Inspire { topic, author, min_likes, count } =>
    inspire::execute(ctx, format, topic.as_deref(), author.as_deref(), *min_likes, *count).await,
```

In `src/cli.rs`, add the subcommand:
```rust
    /// Browse your discovered posts library for inspiration
    Inspire {
        /// Filter by topic (searches post text)
        #[arg(long)]
        topic: Option<String>,
        /// Filter by author username
        #[arg(long)]
        author: Option<String>,
        /// Minimum like count
        #[arg(long)]
        min_likes: Option<i64>,
        /// Number of results
        #[arg(long, default_value = "20")]
        count: usize,
    },
```

- [ ] **Step 4: Update agent-info**

In `src/commands/agent_info.rs`, add `"inspire"` to the commands list and add a usage hint:
```rust
"inspire".into(),
```
```rust
"Use 'xmaster inspire --topic \"your niche\" --min-likes 50' to browse your discovered posts library for content inspiration".into(),
```

- [ ] **Step 5: Verify everything compiles**

Run: `cargo check 2>&1 | head -10`

- [ ] **Step 6: Run all tests**

Run: `cargo test -- -q 2>&1 | tail -5`
Expected: all pass

- [ ] **Step 7: Commit**

```bash
git add src/commands/inspire.rs src/commands/mod.rs src/cli.rs src/commands/agent_info.rs src/intel/store.rs
git commit -m "Add inspire command for querying discovered posts library"
```

---

### Task 8: Update README, bump version, publish

**Files:**
- Modify: `README.md`
- Modify: `Cargo.toml` (version bump to 1.5.0)
- Modify: Homebrew formula

- [ ] **Step 1: Update README**

Add `inspire` to the commands table in the Content section:
```markdown
| `inspire` | Browse your discovered posts library | `xmaster inspire --topic "AI" --min-likes 50` |
```

Add a section about the library:
```markdown
### Discovered Posts Library

Every search, timeline view, and post read automatically caches posts into a local library.
Browse it with:

\```bash
xmaster inspire --topic "longevity" --min-likes 100
xmaster inspire --author "naval" --count 10
xmaster inspire --json  # pipe to jq for analysis
\```
```

- [ ] **Step 2: Bump version**

Change `Cargo.toml` version to `"1.5.0"`.

- [ ] **Step 3: Run final tests**

Run: `cargo test -- -q 2>&1 | tail -5`

- [ ] **Step 4: Commit, tag, push**

```bash
git add -A
git commit -m "v1.5.0: discovered posts library — automatic caching + inspire command"
git tag v1.5.0
git push origin main v1.5.0
```

- [ ] **Step 5: Publish crates.io**

Run: `cargo publish`

- [ ] **Step 6: Update Homebrew**

Update `/tmp/homebrew-tap/Formula/xmaster.rb` tag to `v1.5.0`, commit and push.
