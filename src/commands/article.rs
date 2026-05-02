use crate::context::AppContext;
use crate::errors::XmasterError;
use crate::output::{self, OutputFormat, Tableable};
use crate::providers::xapi::XApi;
use chrono::Local;
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use url::Url;

#[derive(Debug, Clone)]
enum ArticleBlock {
    Heading { level: u8, text: String },
    Paragraph(String),
    Quote(String),
    Indent(String),
    UnorderedList(Vec<String>),
    OrderedList(Vec<String>),
    Figure { alt: String, src: String },
    Video { caption: String, src: String },
    Gif { caption: String, src: String },
    EmbedPost(String),
    EmbedArticle(String),
    Divider,
}

#[derive(Debug, Clone, Default, Serialize)]
struct ArticleFormatCoverage {
    headings: usize,
    subheadings: usize,
    bold: usize,
    italic: usize,
    strikethrough: usize,
    indentation: usize,
    unordered_lists: usize,
    ordered_lists: usize,
    images: usize,
    videos: usize,
    gifs: usize,
    embedded_posts: usize,
    embedded_articles: usize,
    links: usize,
}

#[derive(Serialize)]
struct ArticlePreviewResult {
    output: String,
    spec_source: &'static str,
    title: String,
    author: String,
    handle: String,
    char_count: usize,
    word_count: usize,
    reading_minutes: usize,
    block_count: usize,
    header_image: Option<String>,
    format_coverage: ArticleFormatCoverage,
    supported_article_formats: Vec<&'static str>,
}

#[derive(Debug, Clone, Serialize)]
struct ArticleDraftMedia {
    role: String,
    source: String,
    media_id: String,
    media_category: String,
}

#[derive(Serialize)]
struct ArticleDraftResult {
    article_id: String,
    draft_url: String,
    title: String,
    block_count: usize,
    uploaded_media: Vec<ArticleDraftMedia>,
    header_image: Option<String>,
    endpoint_source: &'static str,
    endpoint_status: &'static str,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<String>,
}

impl Tableable for ArticlePreviewResult {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["Field", "Value"]);
        table.add_row(vec!["Preview", &self.output]);
        table.add_row(vec!["Spec source", self.spec_source]);
        table.add_row(vec!["Title", &self.title]);
        table.add_row(vec![
            "Author",
            &format!("{} (@{})", self.author, self.handle),
        ]);
        table.add_row(vec![
            "Length",
            &format!(
                "{} chars, {} words, ~{} min",
                self.char_count, self.word_count, self.reading_minutes
            ),
        ]);
        table.add_row(vec!["Blocks", &self.block_count.to_string()]);
        if let Some(ref header) = self.header_image {
            table.add_row(vec!["Header image", header]);
        }
        table.add_row(vec![
            "Formats",
            &format!(
                "{} headings, {} subheadings, {} images, {} videos, {} GIFs, {} links, {} embedded posts/articles",
                self.format_coverage.headings,
                self.format_coverage.subheadings,
                self.format_coverage.images,
                self.format_coverage.videos,
                self.format_coverage.gifs,
                self.format_coverage.links,
                self.format_coverage.embedded_posts + self.format_coverage.embedded_articles,
            ),
        ]);
        table
    }
}

impl Tableable for ArticleDraftResult {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["Field", "Value"]);
        table.add_row(vec!["Article draft", &self.article_id]);
        table.add_row(vec!["Drafts", &self.draft_url]);
        table.add_row(vec!["Title", &self.title]);
        table.add_row(vec!["Blocks", &self.block_count.to_string()]);
        table.add_row(vec!["Endpoint", self.endpoint_status]);
        if let Some(ref header) = self.header_image {
            table.add_row(vec!["Header image", header]);
        }
        for media in &self.uploaded_media {
            table.add_row(vec![
                "Media",
                &format!("{} {} ({})", media.role, media.media_id, media.media_category),
            ]);
        }
        for warning in &self.warnings {
            table.add_row(vec!["Warning", warning]);
        }
        table
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn preview(
    format: OutputFormat,
    input: &str,
    output_path: Option<&str>,
    title_arg: Option<&str>,
    subtitle: Option<&str>,
    header_image: Option<&str>,
    author: &str,
    handle: &str,
    avatar: Option<&str>,
    audience: &str,
    open: bool,
) -> Result<(), XmasterError> {
    let (source, input_path, input_dir) = read_input(input)?;
    let output = resolve_output_path(output_path, input_path.as_deref())?;
    let output_dir = output
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();

    let parsed_blocks = parse_blocks(&source);
    let (title, mut blocks) = resolve_title(title_arg, parsed_blocks);
    let header = resolve_header_image(header_image, &mut blocks);
    let coverage = coverage_for(&blocks, &source);

    let char_count = source.chars().count();
    let word_count = source.split_whitespace().count();
    let reading_minutes = word_count.max(1).div_ceil(220);

    let html = render_html(RenderInput {
        title: &title,
        subtitle,
        author,
        handle,
        avatar,
        audience,
        header_image: header.as_deref(),
        blocks: &blocks,
        input_dir: &input_dir,
        output_dir: &output_dir,
        char_count,
        word_count,
        reading_minutes,
    });

    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&output, html)?;

    if open {
        open_file(&output);
    }

    let display = ArticlePreviewResult {
        output: output.to_string_lossy().to_string(),
        spec_source: "https://help.x.com/en/using-x/articles",
        title,
        author: author.to_string(),
        handle: handle.trim_start_matches('@').to_string(),
        char_count,
        word_count,
        reading_minutes,
        block_count: blocks.len(),
        header_image: header,
        format_coverage: coverage,
        supported_article_formats: vec![
            "header_image",
            "headings",
            "subheadings",
            "bold",
            "italics",
            "strikethrough",
            "indentation",
            "numerical_lists",
            "bulleted_lists",
            "images",
            "video",
            "GIFs",
            "embedded_posts",
            "embedded_articles",
            "links",
            "audience_controls_public_or_subscribers",
        ],
    };
    output::render(format, &display, None);
    Ok(())
}

pub async fn draft(
    ctx: Arc<AppContext>,
    format: OutputFormat,
    input: &str,
    title_arg: Option<&str>,
    header_image: Option<&str>,
) -> Result<(), XmasterError> {
    let (source, _input_path, input_dir) = read_input(input)?;
    let parsed_blocks = parse_blocks(&source);
    let (title, mut blocks) = resolve_title(title_arg, parsed_blocks);
    let header = resolve_header_image(header_image, &mut blocks);

    validate_native_article_title(&title)?;

    let api = XApi::new(ctx.clone());
    let mut uploaded_media = Vec::new();
    let mut warnings = Vec::new();
    if !ctx.config.account.premium {
        warnings.push(
            "X may reject Article creation unless this account has the Articles feature enabled"
                .to_string(),
        );
    }

    let content_state = build_native_content_state(
        &api,
        &blocks,
        &input_dir,
        &mut uploaded_media,
        &mut warnings,
    )
    .await?;

    let article = api.create_article_draft(&title, content_state).await?;

    if let Some(ref header_src) = header {
        let (media_id, media_category) = upload_article_media(&api, header_src, &input_dir).await?;
        uploaded_media.push(ArticleDraftMedia {
            role: "cover".into(),
            source: header_src.clone(),
            media_id: media_id.clone(),
            media_category: media_category.clone(),
        });
        api.update_article_cover_media(&article.id, Some(&media_id), Some(&media_category))
            .await?;
    }

    let display = ArticleDraftResult {
        article_id: article.id,
        draft_url: "https://x.com/compose/articles".into(),
        title,
        block_count: blocks.len(),
        uploaded_media,
        header_image: header,
        endpoint_source: "current X web bundle ArticleEntityDraftCreate / ArticleEntityUpdateContent",
        endpoint_status: "private X web GraphQL; not public X API v2",
        warnings,
    };
    output::render(format, &display, None);
    Ok(())
}

fn read_input(input: &str) -> Result<(String, Option<PathBuf>, PathBuf), XmasterError> {
    if input == "-" {
        let mut buf = String::new();
        io::stdin().read_to_string(&mut buf)?;
        let dir = std::env::current_dir()?;
        return Ok((buf, None, dir));
    }

    let path = PathBuf::from(input);
    let source = fs::read_to_string(&path)?;
    let input_dir = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    Ok((source, Some(path), input_dir))
}

fn resolve_output_path(
    output_path: Option<&str>,
    input_path: Option<&Path>,
) -> Result<PathBuf, XmasterError> {
    if let Some(path) = output_path {
        return Ok(PathBuf::from(path));
    }

    if let Some(input) = input_path {
        let mut out = input.to_path_buf();
        out.set_extension("preview.html");
        return Ok(out);
    }

    Ok(std::env::current_dir()?.join("article-preview.html"))
}

fn resolve_title(
    title_arg: Option<&str>,
    blocks: Vec<ArticleBlock>,
) -> (String, Vec<ArticleBlock>) {
    if let Some(title) = title_arg {
        return (title.to_string(), blocks);
    }

    let mut title = None;
    let mut rest = Vec::with_capacity(blocks.len());
    for block in blocks {
        match (&title, &block) {
            (None, ArticleBlock::Heading { level: 1, text }) => {
                title = Some(text.clone());
            }
            _ => rest.push(block),
        }
    }

    if let Some(title) = title {
        (title, rest)
    } else {
        ("Untitled Article".into(), rest)
    }
}

fn resolve_header_image(
    header_image: Option<&str>,
    blocks: &mut Vec<ArticleBlock>,
) -> Option<String> {
    if let Some(header) = header_image {
        return Some(header.to_string());
    }

    let first_content = blocks
        .iter()
        .position(|block| !matches!(block, ArticleBlock::Heading { .. } | ArticleBlock::Divider));

    if let Some(idx) = first_content {
        if let ArticleBlock::Figure { src, .. } = &blocks[idx] {
            let header = src.clone();
            blocks.remove(idx);
            return Some(header);
        }
    }

    None
}

fn validate_native_article_title(title: &str) -> Result<(), XmasterError> {
    let len = title.chars().count();
    if title.trim().is_empty() {
        return Err(XmasterError::Config("Article title cannot be empty".into()));
    }
    if len > 100 {
        return Err(XmasterError::Config(format!(
            "X Article titles are currently limited to 100 characters; got {len}"
        )));
    }
    Ok(())
}

async fn build_native_content_state(
    api: &XApi,
    blocks: &[ArticleBlock],
    input_dir: &Path,
    uploaded_media: &mut Vec<ArticleDraftMedia>,
    warnings: &mut Vec<String>,
) -> Result<Value, XmasterError> {
    let mut builder = NativeContentBuilder::default();

    for block in blocks {
        match block {
            ArticleBlock::Heading { level, text } => {
                let block_type = if *level == 1 {
                    "header-one"
                } else {
                    "header-two"
                };
                builder.push_text_block(block_type, text);
            }
            ArticleBlock::Paragraph(text) => builder.push_text_block("unstyled", text),
            ArticleBlock::Quote(text) | ArticleBlock::Indent(text) => {
                for line in text.lines() {
                    builder.push_text_block("blockquote", line);
                }
            }
            ArticleBlock::UnorderedList(items) => {
                for item in items {
                    builder.push_text_block("unordered-list-item", item);
                }
            }
            ArticleBlock::OrderedList(items) => {
                for item in items {
                    builder.push_text_block("ordered-list-item", item);
                }
            }
            ArticleBlock::Figure { alt, src } => {
                let (media_id, media_category) = upload_article_media(api, src, input_dir).await?;
                uploaded_media.push(ArticleDraftMedia {
                    role: "image".into(),
                    source: src.clone(),
                    media_id: media_id.clone(),
                    media_category: media_category.clone(),
                });
                builder.push_media_block(alt, &media_id, &media_category);
            }
            ArticleBlock::Video { caption, src } => {
                let (media_id, media_category) = upload_article_media(api, src, input_dir).await?;
                uploaded_media.push(ArticleDraftMedia {
                    role: "video".into(),
                    source: src.clone(),
                    media_id: media_id.clone(),
                    media_category: media_category.clone(),
                });
                builder.push_media_block(caption, &media_id, &media_category);
            }
            ArticleBlock::Gif { caption, src } => {
                let (media_id, media_category) = upload_article_media(api, src, input_dir).await?;
                uploaded_media.push(ArticleDraftMedia {
                    role: "gif".into(),
                    source: src.clone(),
                    media_id: media_id.clone(),
                    media_category: media_category.clone(),
                });
                builder.push_media_block(caption, &media_id, &media_category);
            }
            ArticleBlock::EmbedPost(url) => {
                let tweet_id = parse_x_post_id(url).ok_or_else(|| {
                    XmasterError::Config(format!("Could not extract post id from {url}"))
                })?;
                builder.push_tweet_block(&tweet_id);
            }
            ArticleBlock::EmbedArticle(url) => {
                warnings.push(format!(
                    "Embedded Article URLs are saved as links in native drafts: {url}"
                ));
                builder.push_text_block("unstyled", url);
            }
            ArticleBlock::Divider => builder.push_divider_block(),
        }
    }

    Ok(builder.finish())
}

async fn upload_article_media(
    api: &XApi,
    src: &str,
    input_dir: &Path,
) -> Result<(String, String), XmasterError> {
    let src = src.trim();
    if src.starts_with("http://") || src.starts_with("https://") || src.starts_with("file://") {
        return Err(XmasterError::Media(format!(
            "Native X Article drafts need local media files for upload, got: {src}"
        )));
    }

    let path = if Path::new(src).is_absolute() {
        PathBuf::from(src)
    } else {
        input_dir.join(src)
    };
    let path_str = path.to_string_lossy().to_string();
    let media_id = api.upload_media(&path_str).await?;
    Ok((media_id, article_media_category(&path)))
}

fn article_media_category(path: &Path) -> String {
    match path.extension().and_then(|e| e.to_str()).map(str::to_lowercase) {
        Some(ext) if ext == "gif" => "DraftTweetGif".into(),
        Some(ext) if matches!(ext.as_str(), "mp4" | "mov" | "webm") => "AmplifyVideo".into(),
        _ => "DraftTweetImage".into(),
    }
}

#[derive(Default)]
struct NativeContentBuilder {
    blocks: Vec<Value>,
    entity_map: Vec<Value>,
    next_block: usize,
}

impl NativeContentBuilder {
    fn finish(self) -> Value {
        json!({
            "blocks": self.blocks,
            "entity_map": self.entity_map,
        })
    }

    fn push_text_block(&mut self, block_type: &str, text: &str) {
        let parsed = parse_inline_native(text, &mut self.entity_map);
        let key = self.next_block_key();
        self.blocks.push(json!({
            "data": {},
            "text": parsed.text,
            "key": key,
            "type": block_type,
            "entity_ranges": parsed.entity_ranges,
            "inline_style_ranges": parsed.inline_style_ranges,
        }));
    }

    fn push_media_block(&mut self, caption: &str, media_id: &str, media_category: &str) {
        let key = self.entity_map.len();
        self.push_atomic_entity(
            "MEDIA",
            json!({
                "caption": empty_to_null(caption),
                "entity_key": key.to_string(),
                "media_items": [{
                    "local_media_id": media_id,
                    "media_category": media_category,
                    "media_id": media_id,
                }],
            }),
        );
    }

    fn push_tweet_block(&mut self, tweet_id: &str) {
        self.push_atomic_entity("TWEET", json!({ "tweet_id": tweet_id }));
    }

    fn push_divider_block(&mut self) {
        self.push_atomic_entity("DIVIDER", json!({}));
    }

    fn push_atomic_entity(&mut self, entity_type: &str, data: Value) {
        let key = self.push_entity(entity_type, "Immutable", data);
        let block_key = self.next_block_key();
        self.blocks.push(json!({
            "data": {},
            "text": " ",
            "key": block_key,
            "type": "atomic",
            "entity_ranges": [{
                "key": key,
                "offset": 0,
                "length": 1,
            }],
            "inline_style_ranges": [],
        }));
    }

    fn push_entity(&mut self, entity_type: &str, mutability: &str, data: Value) -> usize {
        let key = self.entity_map.len();
        self.entity_map.push(json!({
            "key": key.to_string(),
            "value": {
                "data": prune_null_fields(data),
                "type": entity_type,
                "mutability": mutability,
            },
        }));
        key
    }

    fn next_block_key(&mut self) -> String {
        let key = to_base36(self.next_block);
        self.next_block += 1;
        format!("{key:0>5}")
    }
}

#[derive(Default)]
struct NativeInline {
    text: String,
    inline_style_ranges: Vec<Value>,
    entity_ranges: Vec<Value>,
}

fn parse_inline_native(input: &str, entity_map: &mut Vec<Value>) -> NativeInline {
    parse_inline_native_inner(input, entity_map)
}

fn parse_inline_native_inner(input: &str, entity_map: &mut Vec<Value>) -> NativeInline {
    let mut parsed = NativeInline::default();
    let mut i = 0usize;

    while i < input.len() {
        let rest = &input[i..];

        if let Some((marker_len, style, inner, consumed)) = parse_style_marker(rest) {
            let start = utf16_len(&parsed.text);
            let child = parse_inline_native_inner(inner, entity_map);
            let child_len = utf16_len(&child.text);
            append_shifted_ranges(&mut parsed, child, start);
            if child_len > 0 {
                parsed.inline_style_ranges.push(json!({
                    "offset": start,
                    "length": child_len,
                    "style": style,
                }));
            }
            i += consumed.max(marker_len);
            continue;
        }

        if rest.starts_with('[') {
            if let Some(close) = rest.find("](") {
                if let Some(end) = rest[close + 2..].find(')') {
                    let label = &rest[1..close];
                    let href = strip_optional_title(&rest[close + 2..close + 2 + end]);
                    let start = utf16_len(&parsed.text);
                    let child = parse_inline_native_inner(label, entity_map);
                    let child_len = utf16_len(&child.text);
                    append_shifted_ranges(&mut parsed, child, start);
                    if child_len > 0 {
                        let key = entity_map.len();
                        entity_map.push(json!({
                            "key": key.to_string(),
                            "value": {
                                "data": { "url": href },
                                "type": "LINK",
                                "mutability": "Mutable",
                            },
                        }));
                        parsed.entity_ranges.push(json!({
                            "key": key,
                            "offset": start,
                            "length": child_len,
                        }));
                    }
                    i += close + 2 + end + 1;
                    continue;
                }
            }
        }

        let ch = rest.chars().next().unwrap();
        parsed.text.push(ch);
        i += ch.len_utf8();
    }

    parsed
}

fn parse_style_marker(rest: &str) -> Option<(usize, &'static str, &str, usize)> {
    if let Some(stripped) = rest.strip_prefix("**") {
        if let Some(end) = stripped.find("**") {
            return Some((2, "Bold", &stripped[..end], 2 + end + 2));
        }
    }
    if let Some(stripped) = rest.strip_prefix("~~") {
        if let Some(end) = stripped.find("~~") {
            return Some((2, "Strikethrough", &stripped[..end], 2 + end + 2));
        }
    }
    if rest.starts_with('*') && !rest.starts_with("**") {
        if let Some(end) = rest[1..].find('*') {
            let inner = &rest[1..1 + end];
            if !inner.trim().is_empty() {
                return Some((1, "Italic", inner, 1 + end + 1));
            }
        }
    }
    None
}

fn append_shifted_ranges(target: &mut NativeInline, child: NativeInline, offset: usize) {
    target.text.push_str(&child.text);
    for mut range in child.inline_style_ranges {
        shift_range(&mut range, offset);
        target.inline_style_ranges.push(range);
    }
    for mut range in child.entity_ranges {
        shift_range(&mut range, offset);
        target.entity_ranges.push(range);
    }
}

fn shift_range(range: &mut Value, offset: usize) {
    if let Some(current) = range.get("offset").and_then(|v| v.as_u64()) {
        range["offset"] = json!(current as usize + offset);
    }
}

fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

fn empty_to_null(s: &str) -> Value {
    if s.trim().is_empty() {
        Value::Null
    } else {
        json!(s)
    }
}

fn prune_null_fields(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut out = Map::new();
            for (key, value) in map {
                let value = prune_null_fields(value);
                if !value.is_null() {
                    out.insert(key, value);
                }
            }
            Value::Object(out)
        }
        Value::Array(values) => Value::Array(values.into_iter().map(prune_null_fields).collect()),
        other => other,
    }
}

fn to_base36(mut n: usize) -> String {
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if n == 0 {
        return "0".into();
    }
    let mut out = Vec::new();
    while n > 0 {
        out.push(DIGITS[n % 36] as char);
        n /= 36;
    }
    out.iter().rev().collect()
}

fn parse_x_post_id(url: &str) -> Option<String> {
    let marker = if url.contains("/status/") {
        "/status/"
    } else if url.contains("/statuses/") {
        "/statuses/"
    } else {
        return None;
    };
    let id = url.split(marker).nth(1)?.split(['?', '/', '#']).next()?;
    if id.chars().all(|c| c.is_ascii_digit()) {
        Some(id.to_string())
    } else {
        None
    }
}

fn parse_blocks(source: &str) -> Vec<ArticleBlock> {
    let lines: Vec<&str> = source.lines().collect();
    let mut blocks = Vec::new();
    let mut i = 0usize;

    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();

        if trimmed.is_empty() {
            i += 1;
            continue;
        }

        if trimmed == "---" || trimmed == "***" {
            blocks.push(ArticleBlock::Divider);
            i += 1;
            continue;
        }

        if let Some((level, text)) = parse_heading(trimmed) {
            blocks.push(ArticleBlock::Heading { level, text });
            i += 1;
            continue;
        }

        if let Some((alt, src)) = parse_image(trimmed) {
            blocks.push(ArticleBlock::Figure { alt, src });
            i += 1;
            continue;
        }

        if let Some((caption, src)) = parse_media_directive(trimmed, "video") {
            blocks.push(ArticleBlock::Video { caption, src });
            i += 1;
            continue;
        }

        if let Some((caption, src)) = parse_media_directive(trimmed, "gif") {
            blocks.push(ArticleBlock::Gif { caption, src });
            i += 1;
            continue;
        }

        if let Some(url) =
            parse_embed_directive(trimmed, "post").or_else(|| single_post_url(trimmed))
        {
            blocks.push(ArticleBlock::EmbedPost(url));
            i += 1;
            continue;
        }

        if let Some(url) =
            parse_embed_directive(trimmed, "article").or_else(|| single_article_url(trimmed))
        {
            blocks.push(ArticleBlock::EmbedArticle(url));
            i += 1;
            continue;
        }

        if trimmed.starts_with('>') {
            let mut parts = Vec::new();
            while i < lines.len() && lines[i].trim_start().starts_with('>') {
                parts.push(
                    lines[i]
                        .trim_start()
                        .trim_start_matches('>')
                        .trim()
                        .to_string(),
                );
                i += 1;
            }
            blocks.push(ArticleBlock::Quote(parts.join("\n")));
            continue;
        }

        if line.starts_with("    ") || line.starts_with('\t') {
            let mut parts = Vec::new();
            while i < lines.len() && (lines[i].starts_with("    ") || lines[i].starts_with('\t')) {
                parts.push(lines[i].trim().to_string());
                i += 1;
            }
            blocks.push(ArticleBlock::Indent(parts.join("\n")));
            continue;
        }

        if let Some(item) = parse_unordered_item(trimmed) {
            let mut items = vec![item.to_string()];
            i += 1;
            while i < lines.len() {
                if let Some(item) = parse_unordered_item(lines[i].trim()) {
                    items.push(item.to_string());
                    i += 1;
                } else {
                    break;
                }
            }
            blocks.push(ArticleBlock::UnorderedList(items));
            continue;
        }

        if let Some(item) = parse_ordered_item(trimmed) {
            let mut items = vec![item.to_string()];
            i += 1;
            while i < lines.len() {
                if let Some(item) = parse_ordered_item(lines[i].trim()) {
                    items.push(item.to_string());
                    i += 1;
                } else {
                    break;
                }
            }
            blocks.push(ArticleBlock::OrderedList(items));
            continue;
        }

        let mut paragraph = vec![trimmed.to_string()];
        i += 1;
        while i < lines.len() {
            let next = lines[i];
            let next_trimmed = next.trim();
            if next_trimmed.is_empty() || starts_block(next, next_trimmed) {
                break;
            }
            paragraph.push(next_trimmed.to_string());
            i += 1;
        }
        blocks.push(ArticleBlock::Paragraph(paragraph.join(" ")));
    }

    blocks
}

fn starts_block(line: &str, trimmed: &str) -> bool {
    trimmed == "---"
        || trimmed == "***"
        || parse_heading(trimmed).is_some()
        || parse_image(trimmed).is_some()
        || parse_media_directive(trimmed, "video").is_some()
        || parse_media_directive(trimmed, "gif").is_some()
        || parse_embed_directive(trimmed, "post").is_some()
        || parse_embed_directive(trimmed, "article").is_some()
        || single_post_url(trimmed).is_some()
        || single_article_url(trimmed).is_some()
        || trimmed.starts_with('>')
        || line.starts_with("    ")
        || line.starts_with('\t')
        || parse_unordered_item(trimmed).is_some()
        || parse_ordered_item(trimmed).is_some()
}

fn parse_heading(line: &str) -> Option<(u8, String)> {
    let level = line.chars().take_while(|c| *c == '#').count();
    if !(1..=3).contains(&level) {
        return None;
    }
    let rest = line[level..].trim_start();
    if rest.is_empty() {
        return None;
    }
    Some((level as u8, rest.to_string()))
}

fn parse_image(line: &str) -> Option<(String, String)> {
    if !line.starts_with("![") {
        return None;
    }
    let close_alt = line.find("](")?;
    let end = line.rfind(')')?;
    if end <= close_alt + 2 {
        return None;
    }
    let alt = &line[2..close_alt];
    let src = &line[close_alt + 2..end];
    Some((
        alt.trim().to_string(),
        strip_optional_title(src).to_string(),
    ))
}

fn parse_media_directive(line: &str, kind: &str) -> Option<(String, String)> {
    let prefix = format!("::{kind}[");
    if line.starts_with(&prefix) {
        let close_caption = line.find("](")?;
        let end = line.rfind(')')?;
        let caption = &line[prefix.len()..close_caption];
        let src = &line[close_caption + 2..end];
        return Some((
            caption.trim().to_string(),
            strip_optional_title(src).to_string(),
        ));
    }

    let bare_prefix = format!("::{kind}(");
    if line.starts_with(&bare_prefix) && line.ends_with(')') {
        let src = &line[bare_prefix.len()..line.len() - 1];
        return Some(("".into(), src.trim().to_string()));
    }

    None
}

fn parse_embed_directive(line: &str, kind: &str) -> Option<String> {
    let prefix = format!("::{kind}(");
    if line.starts_with(&prefix) && line.ends_with(')') {
        return Some(line[prefix.len()..line.len() - 1].trim().to_string());
    }
    None
}

fn strip_optional_title(src: &str) -> &str {
    src.split(" \"").next().unwrap_or(src).trim()
}

fn parse_unordered_item(line: &str) -> Option<&str> {
    line.strip_prefix("- ")
        .or_else(|| line.strip_prefix("* "))
        .or_else(|| line.strip_prefix("+ "))
}

fn parse_ordered_item(line: &str) -> Option<&str> {
    let dot = line.find(". ")?;
    if dot == 0 || !line[..dot].chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(&line[dot + 2..])
}

fn single_post_url(line: &str) -> Option<String> {
    if is_x_post_url(line) {
        Some(line.to_string())
    } else {
        None
    }
}

fn single_article_url(line: &str) -> Option<String> {
    if is_x_article_url(line) {
        Some(line.to_string())
    } else {
        None
    }
}

fn is_x_post_url(s: &str) -> bool {
    (s.starts_with("https://x.com/") || s.starts_with("https://twitter.com/"))
        && (s.contains("/status/") || s.contains("/statuses/"))
        && !s.contains(' ')
}

fn is_x_article_url(s: &str) -> bool {
    (s.starts_with("https://x.com/i/article/") || s.starts_with("https://twitter.com/i/article/"))
        && !s.contains(' ')
}

fn coverage_for(blocks: &[ArticleBlock], source: &str) -> ArticleFormatCoverage {
    let mut coverage = ArticleFormatCoverage::default();

    for block in blocks {
        match block {
            ArticleBlock::Heading { level: 1, .. } => coverage.headings += 1,
            ArticleBlock::Heading { .. } => coverage.subheadings += 1,
            ArticleBlock::Indent(_) => coverage.indentation += 1,
            ArticleBlock::UnorderedList(_) => coverage.unordered_lists += 1,
            ArticleBlock::OrderedList(_) => coverage.ordered_lists += 1,
            ArticleBlock::Figure { .. } => coverage.images += 1,
            ArticleBlock::Video { .. } => coverage.videos += 1,
            ArticleBlock::Gif { .. } => coverage.gifs += 1,
            ArticleBlock::EmbedPost(_) => coverage.embedded_posts += 1,
            ArticleBlock::EmbedArticle(_) => coverage.embedded_articles += 1,
            _ => {}
        }
    }

    coverage.bold = source.matches("**").count() / 2;
    coverage.italic = source
        .matches('*')
        .count()
        .saturating_sub(coverage.bold * 4)
        / 2;
    coverage.strikethrough = source.matches("~~").count() / 2;
    coverage.links = source.matches("](").count().saturating_sub(coverage.images);
    coverage
}

struct RenderInput<'a> {
    title: &'a str,
    subtitle: Option<&'a str>,
    author: &'a str,
    handle: &'a str,
    avatar: Option<&'a str>,
    audience: &'a str,
    header_image: Option<&'a str>,
    blocks: &'a [ArticleBlock],
    input_dir: &'a Path,
    output_dir: &'a Path,
    char_count: usize,
    word_count: usize,
    reading_minutes: usize,
}

fn render_html(input: RenderInput<'_>) -> String {
    let handle = input.handle.trim_start_matches('@');
    let date = Local::now().format("%b %-d, %Y").to_string();
    let avatar_html = render_avatar(
        input.avatar,
        input.author,
        input.input_dir,
        input.output_dir,
    );
    let header_html = input
        .header_image
        .map(|src| {
            format!(
                "<figure class=\"article-cover\"><img src=\"{}\" alt=\"{}\"></figure>",
                escape_attr(&asset_url(src, input.input_dir, input.output_dir)),
                escape_attr(input.title),
            )
        })
        .unwrap_or_default();
    let subtitle_html = input
        .subtitle
        .filter(|s| !s.trim().is_empty())
        .map(|s| format!("<p class=\"article-subtitle\">{}</p>", render_inline(s)))
        .unwrap_or_default();
    let body_html = input
        .blocks
        .iter()
        .map(|block| render_block(block, input.input_dir, input.output_dir))
        .collect::<Vec<_>>()
        .join("\n");
    let feed_card_image = input
        .header_image
        .map(|src| {
            format!(
                "<img class=\"card-image\" src=\"{}\" alt=\"\">",
                escape_attr(&asset_url(src, input.input_dir, input.output_dir)),
            )
        })
        .unwrap_or_default();
    let audience = if input.audience.eq_ignore_ascii_case("subscribers") {
        "Subscribers"
    } else {
        "Public"
    };

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{title}</title>
<style>
  :root {{
    --bg: #000000;
    --panel: #16181c;
    --border: #2f3336;
    --border-soft: #202327;
    --text: #e7e9ea;
    --muted: #71767b;
    --link: #1d9bf0;
    --hover: #080808;
  }}
  * {{ box-sizing: border-box; }}
  body {{
    margin: 0;
    background: var(--bg);
    color: var(--text);
    font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
    line-height: 1.45;
  }}
  a {{ color: var(--link); text-decoration: none; }}
  a:hover {{ text-decoration: underline; }}
  .shell {{
    width: min(100%, 640px);
    margin: 0 auto;
    min-height: 100vh;
    border-left: 1px solid var(--border);
    border-right: 1px solid var(--border);
  }}
  .topbar {{
    position: sticky;
    top: 0;
    z-index: 5;
    height: 54px;
    display: flex;
    align-items: center;
    gap: 20px;
    padding: 0 16px;
    background: rgba(0, 0, 0, 0.82);
    backdrop-filter: blur(16px);
    border-bottom: 1px solid var(--border);
    font-weight: 700;
    font-size: 20px;
  }}
  .back {{ font-size: 26px; line-height: 1; color: var(--text); }}
  .article, .share-preview {{
    border-bottom: 1px solid var(--border);
  }}
  .share-preview {{
    padding: 16px;
  }}
  .post-row {{
    display: grid;
    grid-template-columns: 40px minmax(0, 1fr);
    gap: 12px;
  }}
  .avatar {{
    width: 40px;
    height: 40px;
    border-radius: 50%;
    background: linear-gradient(135deg, #f3d36b, #9f7b29);
    color: #050505;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    font-weight: 800;
    overflow: hidden;
    flex: 0 0 auto;
  }}
  .avatar img {{ width: 100%; height: 100%; object-fit: cover; display: block; }}
  .meta-line {{
    display: flex;
    gap: 4px;
    align-items: center;
    flex-wrap: wrap;
    min-width: 0;
    font-size: 15px;
  }}
  .name {{ font-weight: 700; color: var(--text); }}
  .handle, .dot, .time, .audience {{ color: var(--muted); }}
  .tweet-copy {{ margin: 4px 0 12px; font-size: 15px; }}
  .article-card {{
    border: 1px solid var(--border);
    border-radius: 16px;
    overflow: hidden;
    background: var(--bg);
  }}
  .card-image {{
    width: 100%;
    aspect-ratio: 3 / 1.55;
    object-fit: cover;
    display: block;
    border-bottom: 1px solid var(--border);
  }}
  .card-body {{ padding: 12px 14px 14px; }}
  .card-kicker {{ color: var(--muted); font-size: 13px; margin-bottom: 3px; }}
  .card-title {{ font-size: 18px; line-height: 1.2; font-weight: 800; color: var(--text); }}
  .card-subtitle {{ margin-top: 6px; color: var(--muted); font-size: 14px; line-height: 1.35; }}
  .article {{
    padding: 0 0 48px;
  }}
  .article-cover {{
    margin: 0;
    border-bottom: 1px solid var(--border);
    background: var(--panel);
  }}
  .article-cover img {{
    width: 100%;
    aspect-ratio: 3 / 1.45;
    object-fit: cover;
    display: block;
  }}
  .article-head {{
    padding: 28px 28px 18px;
  }}
  .article-title {{
    margin: 0;
    color: var(--text);
    font-size: clamp(34px, 7.2vw, 48px);
    line-height: 1.04;
    letter-spacing: 0;
    font-weight: 800;
  }}
  .article-subtitle {{
    margin: 12px 0 0;
    color: var(--muted);
    font-size: 20px;
    line-height: 1.35;
  }}
  .byline {{
    display: flex;
    gap: 12px;
    align-items: center;
    margin-top: 20px;
    color: var(--muted);
    font-size: 15px;
  }}
  .article-body {{
    padding: 0 28px;
    font-size: 19px;
    line-height: 1.58;
  }}
  .article-body p {{ margin: 0 0 22px; }}
  .article-body h2 {{
    margin: 38px 0 14px;
    font-size: 28px;
    line-height: 1.15;
    color: var(--text);
  }}
  .article-body h3 {{
    margin: 30px 0 10px;
    font-size: 22px;
    line-height: 1.2;
    color: var(--text);
  }}
  .article-body ul, .article-body ol {{
    margin: 0 0 24px;
    padding-left: 28px;
  }}
  .article-body li {{ margin: 8px 0; padding-left: 3px; }}
  .quote {{
    margin: 26px 0;
    padding: 4px 0 4px 18px;
    border-left: 4px solid var(--border);
    color: #d5d9dc;
    font-size: 20px;
  }}
  .indent {{
    margin: 22px 0;
    padding: 16px 18px;
    border: 1px solid var(--border-soft);
    border-radius: 12px;
    background: var(--panel);
    color: #d5d9dc;
  }}
  .article-figure {{
    margin: 30px 0;
  }}
  .article-figure img, .article-figure video {{
    width: 100%;
    max-height: 680px;
    object-fit: contain;
    display: block;
    border: 1px solid var(--border);
    border-radius: 16px;
    background: var(--panel);
  }}
  figcaption {{
    margin-top: 8px;
    color: var(--muted);
    font-size: 14px;
    line-height: 1.35;
  }}
  .embed-card {{
    margin: 24px 0;
    border: 1px solid var(--border);
    border-radius: 16px;
    padding: 14px;
    background: var(--bg);
  }}
  .embed-label {{ color: var(--muted); font-size: 13px; margin-bottom: 4px; }}
  .embed-url {{
    color: var(--text);
    font-size: 15px;
    overflow-wrap: anywhere;
  }}
  .divider {{
    border: 0;
    border-top: 1px solid var(--border);
    margin: 34px 0;
  }}
  .article-foot {{
    margin-top: 34px;
    padding-top: 18px;
    border-top: 1px solid var(--border);
    color: var(--muted);
    font-size: 14px;
  }}
  @media (max-width: 520px) {{
    .shell {{ border-left: 0; border-right: 0; }}
    .article-head {{ padding: 24px 18px 16px; }}
    .article-body {{ padding: 0 18px; font-size: 18px; }}
    .article-title {{ font-size: 36px; }}
    .article-subtitle {{ font-size: 18px; }}
  }}
</style>
</head>
<body>
<main class="shell">
  <div class="topbar"><span class="back">‹</span><span>Article</span></div>
  <article class="article">
    {header}
    <header class="article-head">
      <h1 class="article-title">{title}</h1>
      {subtitle}
      <div class="byline">
        {avatar}
        <div>
          <div class="meta-line"><span class="name">{author}</span><span class="handle">@{handle}</span></div>
          <div>{date} <span class="dot">.</span> {minutes} min read <span class="dot">.</span> <span class="audience">{audience}</span></div>
        </div>
      </div>
    </header>
    <div class="article-body">
{body}
      <footer class="article-foot">{chars} chars <span class="dot">.</span> {words} words</footer>
    </div>
  </article>
  <section class="share-preview">
    <div class="post-row">
      {avatar}
      <div>
        <div class="meta-line"><span class="name">{author}</span><span class="handle">@{handle}</span><span class="dot">.</span><span class="time">now</span></div>
        <p class="tweet-copy">New Article</p>
        <div class="article-card">
          {feed_card_image}
          <div class="card-body">
            <div class="card-kicker">x.com</div>
            <div class="card-title">{title}</div>
            {feed_subtitle}
          </div>
        </div>
      </div>
    </div>
  </section>
</main>
</body>
</html>
"#,
        title = escape_html(input.title),
        author = escape_html(input.author),
        handle = escape_html(handle),
        avatar = avatar_html,
        feed_card_image = feed_card_image,
        feed_subtitle = input
            .subtitle
            .filter(|s| !s.trim().is_empty())
            .map(|s| format!("<div class=\"card-subtitle\">{}</div>", render_inline(s)))
            .unwrap_or_default(),
        header = header_html,
        subtitle = subtitle_html,
        date = escape_html(&date),
        minutes = input.reading_minutes,
        audience = escape_html(audience),
        body = body_html,
        chars = input.char_count,
        words = input.word_count,
    )
}

fn render_avatar(
    avatar: Option<&str>,
    author: &str,
    input_dir: &Path,
    output_dir: &Path,
) -> String {
    if let Some(src) = avatar.filter(|s| !s.trim().is_empty()) {
        return format!(
            "<span class=\"avatar\"><img src=\"{}\" alt=\"{}\"></span>",
            escape_attr(&asset_url(src, input_dir, output_dir)),
            escape_attr(author),
        );
    }
    let initial = author
        .chars()
        .find(|c| c.is_ascii_alphanumeric())
        .unwrap_or('A')
        .to_ascii_uppercase();
    format!("<span class=\"avatar\">{initial}</span>")
}

fn render_block(block: &ArticleBlock, input_dir: &Path, output_dir: &Path) -> String {
    match block {
        ArticleBlock::Heading { level: 1, text } => {
            format!("<h2>{}</h2>", render_inline(text))
        }
        ArticleBlock::Heading { level: 2, text } => {
            format!("<h2>{}</h2>", render_inline(text))
        }
        ArticleBlock::Heading { text, .. } => {
            format!("<h3>{}</h3>", render_inline(text))
        }
        ArticleBlock::Paragraph(text) => format!("<p>{}</p>", render_inline(text)),
        ArticleBlock::Quote(text) => format!(
            "<blockquote class=\"quote\">{}</blockquote>",
            render_lines(text)
        ),
        ArticleBlock::Indent(text) => format!("<div class=\"indent\">{}</div>", render_lines(text)),
        ArticleBlock::UnorderedList(items) => {
            let items = items
                .iter()
                .map(|item| format!("<li>{}</li>", render_inline(item)))
                .collect::<Vec<_>>()
                .join("");
            format!("<ul>{items}</ul>")
        }
        ArticleBlock::OrderedList(items) => {
            let items = items
                .iter()
                .map(|item| format!("<li>{}</li>", render_inline(item)))
                .collect::<Vec<_>>()
                .join("");
            format!("<ol>{items}</ol>")
        }
        ArticleBlock::Figure { alt, src } => render_figure("img", src, alt, input_dir, output_dir),
        ArticleBlock::Video { caption, src } => {
            render_figure("video", src, caption, input_dir, output_dir)
        }
        ArticleBlock::Gif { caption, src } => {
            render_figure("img", src, caption, input_dir, output_dir)
        }
        ArticleBlock::EmbedPost(url) => render_embed("Embedded post", url),
        ArticleBlock::EmbedArticle(url) => render_embed("Embedded article", url),
        ArticleBlock::Divider => "<hr class=\"divider\">".into(),
    }
}

fn render_lines(text: &str) -> String {
    text.lines()
        .map(render_inline)
        .collect::<Vec<_>>()
        .join("<br>")
}

fn render_figure(
    kind: &str,
    src: &str,
    caption: &str,
    input_dir: &Path,
    output_dir: &Path,
) -> String {
    let src = escape_attr(&asset_url(src, input_dir, output_dir));
    let caption_html = if caption.trim().is_empty() {
        String::new()
    } else {
        format!("<figcaption>{}</figcaption>", render_inline(caption))
    };

    if kind == "video" {
        format!(
            "<figure class=\"article-figure\"><video src=\"{src}\" controls playsinline></video>{caption_html}</figure>"
        )
    } else {
        format!(
            "<figure class=\"article-figure\"><img src=\"{src}\" alt=\"{}\">{caption_html}</figure>",
            escape_attr(caption),
        )
    }
}

fn render_embed(label: &str, url: &str) -> String {
    format!(
        "<div class=\"embed-card\"><div class=\"embed-label\">{}</div><a class=\"embed-url\" href=\"{}\">{}</a></div>",
        escape_html(label),
        escape_attr(url),
        escape_html(url),
    )
}

fn render_inline(input: &str) -> String {
    let mut out = String::new();
    let mut i = 0usize;

    while i < input.len() {
        let rest = &input[i..];

        if let Some(stripped) = rest.strip_prefix("**") {
            if let Some(end) = stripped.find("**") {
                let inner = &stripped[..end];
                out.push_str("<strong>");
                out.push_str(&render_inline(inner));
                out.push_str("</strong>");
                i += 2 + end + 2;
                continue;
            }
        }

        if let Some(stripped) = rest.strip_prefix("~~") {
            if let Some(end) = stripped.find("~~") {
                let inner = &stripped[..end];
                out.push_str("<s>");
                out.push_str(&render_inline(inner));
                out.push_str("</s>");
                i += 2 + end + 2;
                continue;
            }
        }

        if rest.starts_with('*') && !rest.starts_with("**") {
            if let Some(end) = rest[1..].find('*') {
                let inner = &rest[1..1 + end];
                if !inner.trim().is_empty() {
                    out.push_str("<em>");
                    out.push_str(&render_inline(inner));
                    out.push_str("</em>");
                    i += 1 + end + 1;
                    continue;
                }
            }
        }

        if rest.starts_with('[') {
            if let Some(close) = rest.find("](") {
                if let Some(end) = rest[close + 2..].find(')') {
                    let label = &rest[1..close];
                    let href = &rest[close + 2..close + 2 + end];
                    out.push_str("<a href=\"");
                    out.push_str(&escape_attr(strip_optional_title(href)));
                    out.push_str("\">");
                    out.push_str(&render_inline(label));
                    out.push_str("</a>");
                    i += close + 2 + end + 1;
                    continue;
                }
            }
        }

        let ch = rest.chars().next().unwrap();
        out.push_str(&escape_html(&ch.to_string()));
        i += ch.len_utf8();
    }

    out
}

fn asset_url(src: &str, input_dir: &Path, output_dir: &Path) -> String {
    let src = src.trim();
    if src.starts_with("http://")
        || src.starts_with("https://")
        || src.starts_with("data:")
        || src.starts_with("file://")
        || src.starts_with('#')
    {
        return src.to_string();
    }

    let path = Path::new(src);
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        input_dir.join(path)
    };

    if let Ok(rel) = abs.strip_prefix(output_dir) {
        if let Some(rel_str) = rel.to_str() {
            if !rel_str.starts_with("..") {
                return rel_str.to_string();
            }
        }
    }

    Url::from_file_path(&abs)
        .map(|url| url.to_string())
        .unwrap_or_else(|_| src.to_string())
}

fn escape_html(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn escape_attr(input: &str) -> String {
    escape_html(input).replace('"', "&quot;")
}

fn open_file(path: &Path) {
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(path).status();
    }
    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("xdg-open").arg(path).status();
    }
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("cmd")
            .args(["/C", "start"])
            .arg(path)
            .status();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_article_blocks() {
        let md = "# Title\n\n## Section\n\nText with **bold** and [link](https://x.com).\n\n- one\n- two\n\n![Alt](cover.png)\n\n::post(https://x.com/a/status/123)";
        let blocks = parse_blocks(md);
        assert!(matches!(blocks[0], ArticleBlock::Heading { level: 1, .. }));
        assert!(blocks
            .iter()
            .any(|b| matches!(b, ArticleBlock::UnorderedList(_))));
        assert!(blocks
            .iter()
            .any(|b| matches!(b, ArticleBlock::Figure { .. })));
        assert!(blocks
            .iter()
            .any(|b| matches!(b, ArticleBlock::EmbedPost(_))));
        let coverage = coverage_for(&blocks, md);
        assert_eq!(coverage.images, 1);
        assert_eq!(coverage.embedded_posts, 1);
        assert_eq!(coverage.bold, 1);
    }

    #[test]
    fn covers_official_article_format_surface() {
        let md = "# Title\n\n### Subheading\n\n    Indented text\n\n1. first\n2. second\n\n::video[Demo](clip.mp4)\n\n::gif[Loop](loop.gif)\n\n::article(https://x.com/i/article/1234567890)";
        let blocks = parse_blocks(md);
        let coverage = coverage_for(&blocks, md);
        assert_eq!(coverage.subheadings, 1);
        assert_eq!(coverage.indentation, 1);
        assert_eq!(coverage.ordered_lists, 1);
        assert_eq!(coverage.videos, 1);
        assert_eq!(coverage.gifs, 1);
        assert_eq!(coverage.embedded_articles, 1);
    }

    #[test]
    fn renders_inline_formatting() {
        let html = render_inline("This is **bold**, *italic*, ~~gone~~, and [X](https://x.com).");
        assert!(html.contains("<strong>bold</strong>"));
        assert!(html.contains("<em>italic</em>"));
        assert!(html.contains("<s>gone</s>"));
        assert!(html.contains("<a href=\"https://x.com\">X</a>"));
    }

    #[test]
    fn builds_native_inline_ranges_for_x_articles() {
        let mut entities = Vec::new();
        let parsed = parse_inline_native(
            "This is **bold**, *italic*, ~~gone~~, and [X](https://x.com).",
            &mut entities,
        );

        assert_eq!(parsed.text, "This is bold, italic, gone, and X.");
        assert_eq!(entities.len(), 1);
        assert_eq!(entities[0]["value"]["type"], "LINK");
        assert!(parsed
            .inline_style_ranges
            .iter()
            .any(|r| r["style"] == "Bold"));
        assert!(parsed
            .inline_style_ranges
            .iter()
            .any(|r| r["style"] == "Italic"));
        assert!(parsed
            .inline_style_ranges
            .iter()
            .any(|r| r["style"] == "Strikethrough"));
        assert_eq!(parsed.entity_ranges.len(), 1);
    }

    #[test]
    fn builds_native_atomic_post_block() {
        let mut builder = NativeContentBuilder::default();
        builder.push_text_block("header-two", "Section");
        builder.push_tweet_block("1234567890");
        builder.push_divider_block();
        let state = builder.finish();

        assert_eq!(state["blocks"].as_array().unwrap().len(), 3);
        assert_eq!(state["entity_map"].as_array().unwrap().len(), 2);
        assert_eq!(state["entity_map"][0]["value"]["type"], "TWEET");
        assert_eq!(state["entity_map"][0]["value"]["data"]["tweet_id"], "1234567890");
        assert_eq!(state["entity_map"][1]["value"]["type"], "DIVIDER");
    }
}
