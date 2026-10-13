//! Media upload (images, GIFs, video) for posts, threads, scheduled posts and
//! Article drafts. `XApi::upload_media` and `XApi::upload_media_many` delegate
//! here.
//!
//! Routes, tried in order until one returns a media ID:
//! 1. Web session (free): `upload.x.com/i/media/upload.json` with the browser
//!    cookies, the same INIT/APPEND/FINALIZE/STATUS calls x.com makes. Used
//!    when web writes are on (`settings.web_writes` is not "never") and
//!    cookies are configured.
//! 2. API v2 chunked upload (`/2/media/upload/initialize`, `{id}/append`,
//!    `{id}/finalize`, STATUS): X's documented path since 1 Sep 2026.
//! 3. API v1.1 (`upload.twitter.com/1.1/media/upload.json`): last resort.
//!
//! A media ID is tied to the account, not the route, so a file uploaded on
//! the web can be attached to a post sent through the API and vice versa.
//! Uploads that fail leave nothing visible (unused media expire in 24 h), so
//! falling through to the next route cannot double-post.

use crate::config::AppConfig;
use crate::context::AppContext;
use crate::errors::XmasterError;
use base64::Engine as _;
use reqwest::header::HeaderValue;
use reqwest::Method;
use reqwest_oauth1::OAuthClientProvider;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tracing::warn;

use super::web::Web;

const WEB_UPLOAD: &str = "https://upload.x.com/i/media/upload.json";
/// Path the web route's transaction ID is signed for.
const WEB_UPLOAD_PATH: &str = "/i/media/upload.json";
const V2_UPLOAD: &str = "https://api.x.com/2/media/upload";
const V1_UPLOAD: &str = "https://upload.twitter.com/1.1/media/upload.json";

const MIB: usize = 1024 * 1024;
/// Files uploaded at once by `upload_many` (a post carries at most 4).
const MAX_PARALLEL: usize = 4;
/// Floor for one APPEND request: a multi-MB chunk on a slow uplink outlasts
/// the default 15 s client timeout.
const CHUNK_TIMEOUT_SECS: u64 = 120;
/// STATUS polling limits while X transcodes a video.
const MAX_POLLS: u32 = 30;
const MAX_POLL_SECS: u64 = 300;
/// Prefix of errors where X decoded the file and refused it: another route
/// would refuse it too, so there is no fallback.
const REJECTED: &str = "X rejected the media";

// ---------------------------------------------------------------------------
// The file
// ---------------------------------------------------------------------------

/// A local file, typed and size-checked, ready to upload.
#[derive(Debug)]
struct MediaFile {
    mime: &'static str,
    category: &'static str,
    bytes: Vec<u8>,
}

impl MediaFile {
    async fn load(file_path: &str) -> Result<Self, XmasterError> {
        let path = Path::new(file_path);
        if !path.exists() {
            return Err(XmasterError::Media(format!("File not found: {file_path}")));
        }
        let bytes = tokio::fs::read(path).await?;
        Self::new(path, bytes)
    }

    fn new(path: &Path, bytes: Vec<u8>) -> Result<Self, XmasterError> {
        let mime = sniff(&bytes).or_else(|| mime_from_ext(path)).ok_or_else(|| {
            XmasterError::Media(format!(
                "Unsupported media file {}: X takes PNG, JPEG, GIF, WebP, MP4 or MOV",
                path.display()
            ))
        })?;
        let (category, max_mb, kind) = match mime {
            m if m.starts_with("video/") => ("tweet_video", 512, "video"),
            // GIFs need their own category or X flattens them to a still image
            "image/gif" => ("tweet_gif", 15, "GIF"),
            _ => ("tweet_image", 5, "image"),
        };
        if bytes.len() > max_mb * MIB {
            return Err(XmasterError::Media(format!(
                "File too large: {}MB (max {max_mb}MB for {kind})",
                bytes.len() / MIB
            )));
        }
        Ok(Self { mime, category, bytes })
    }
}

/// MIME type from the file's magic bytes.
fn sniff(b: &[u8]) -> Option<&'static str> {
    if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if b.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if b.len() >= 12 && &b[..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        Some("image/webp")
    } else if b.len() >= 12 && &b[4..8] == b"ftyp" {
        // ISO media: MP4 and QuickTime, but not HEIC/AVIF stills
        match &b[8..12] {
            b"qt  " => Some("video/quicktime"),
            b"heic" | b"heix" | b"mif1" | b"msf1" | b"avif" => None,
            _ => Some("video/mp4"),
        }
    } else {
        None
    }
}

fn mime_from_ext(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// Routes and their requests
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Route {
    Web,
    V2,
    V1,
}

/// One upload request before auth and chunk bytes are attached.
#[derive(Debug, PartialEq)]
struct Call {
    method: Method,
    url: String,
    body: CallBody,
}

#[derive(Debug, PartialEq)]
enum CallBody {
    Empty,
    Form(Vec<(&'static str, String)>),
    Json(Value),
    /// Multipart: these text fields, then the chunk as the `media` part.
    Chunk(Vec<(&'static str, String)>),
    /// Form: these fields, then the chunk base64-encoded as `media_data`.
    Chunk64(Vec<(&'static str, String)>),
}

fn with_query(base: &str, pairs: &[(&str, &str)]) -> String {
    url::Url::parse_with_params(base, pairs)
        .map(String::from)
        .unwrap_or_else(|_| base.to_string())
}

impl Route {
    fn name(self) -> &'static str {
        match self {
            Route::Web => "web session",
            Route::V2 => "API v2",
            Route::V1 => "API v1.1",
        }
    }

    /// Bytes per APPEND: x.com's own 1 MiB on the web; v2 recommends at most
    /// 5 MB (8 MB hard cap); v1.1 keeps its proven 1 MiB.
    fn chunk_size(self) -> usize {
        match self {
            Route::V2 => 4 * MIB,
            Route::Web | Route::V1 => MIB,
        }
    }

    fn init(self, f: &MediaFile) -> Call {
        let total = f.bytes.len().to_string();
        let (method, url, body) = match self {
            Route::Web => (
                Method::POST,
                with_query(WEB_UPLOAD, &[
                    ("command", "INIT"),
                    ("total_bytes", &total),
                    ("media_type", f.mime),
                    ("media_category", f.category),
                ]),
                CallBody::Empty,
            ),
            Route::V2 => (
                Method::POST,
                format!("{V2_UPLOAD}/initialize"),
                CallBody::Json(json!({
                    "media_type": f.mime,
                    "total_bytes": f.bytes.len(),
                    "media_category": f.category,
                })),
            ),
            Route::V1 => (
                Method::POST,
                V1_UPLOAD.to_string(),
                CallBody::Form(vec![
                    ("command", "INIT".into()),
                    ("media_type", f.mime.into()),
                    ("total_bytes", total),
                    ("media_category", f.category.into()),
                ]),
            ),
        };
        Call { method, url, body }
    }

    fn append(self, id: &str, segment: usize) -> Call {
        let seg = segment.to_string();
        let (url, body) = match self {
            Route::Web => (
                with_query(WEB_UPLOAD, &[("command", "APPEND"), ("media_id", id), ("segment_index", &seg)]),
                CallBody::Chunk(vec![]),
            ),
            Route::V2 => (
                format!("{V2_UPLOAD}/{id}/append"),
                CallBody::Chunk(vec![("segment_index", seg)]),
            ),
            Route::V1 => (
                V1_UPLOAD.to_string(),
                CallBody::Chunk64(vec![
                    ("command", "APPEND".into()),
                    ("media_id", id.into()),
                    ("segment_index", seg),
                ]),
            ),
        };
        Call { method: Method::POST, url, body }
    }

    fn finalize(self, id: &str) -> Call {
        let (url, body) = match self {
            Route::Web => (with_query(WEB_UPLOAD, &[("command", "FINALIZE"), ("media_id", id)]), CallBody::Empty),
            Route::V2 => (format!("{V2_UPLOAD}/{id}/finalize"), CallBody::Empty),
            Route::V1 => (
                V1_UPLOAD.to_string(),
                CallBody::Form(vec![("command", "FINALIZE".into()), ("media_id", id.into())]),
            ),
        };
        Call { method: Method::POST, url, body }
    }

    fn status(self, id: &str) -> Call {
        let base = match self {
            Route::Web => WEB_UPLOAD,
            Route::V2 => V2_UPLOAD,
            Route::V1 => V1_UPLOAD,
        };
        Call {
            method: Method::GET,
            url: with_query(base, &[("command", "STATUS"), ("media_id", id)]),
            body: CallBody::Empty,
        }
    }
}

/// Routes to try, in order, for this config.
fn routes(cfg: &AppConfig) -> Vec<Route> {
    let mut routes = Vec::with_capacity(3);
    if cfg.web_first(true) {
        routes.push(Route::Web);
    }
    if cfg.has_x_auth() {
        routes.extend([Route::V2, Route::V1]);
    }
    routes
}

// ---------------------------------------------------------------------------
// Response parsing
// ---------------------------------------------------------------------------

/// Media ID from an INIT/FINALIZE response: v2 nests it as `data.id`; v1.1
/// and the web return `media_id_string` (and a numeric `media_id`).
fn media_id(v: &Value) -> Option<String> {
    v.pointer("/data/id")
        .or_else(|| v.get("media_id_string"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| v.get("media_id").and_then(Value::as_u64).map(|id| id.to_string()))
        .filter(|id| !id.is_empty())
}

fn processing_info(v: &Value) -> Option<&Value> {
    v.pointer("/data/processing_info").or_else(|| v.get("processing_info"))
}

#[derive(Debug, PartialEq)]
enum Processing {
    Done,
    Wait(u64),
    Failed(String),
}

fn processing(info: Option<&Value>) -> Processing {
    let Some(info) = info else { return Processing::Done };
    match info.get("state").and_then(Value::as_str).unwrap_or("succeeded") {
        "succeeded" => Processing::Done,
        "failed" => Processing::Failed(
            info.pointer("/error/message")
                .or_else(|| info.pointer("/error/name"))
                .and_then(Value::as_str)
                .unwrap_or("unknown processing error")
                .to_string(),
        ),
        _ => Processing::Wait(info.get("check_after_secs").and_then(Value::as_u64).unwrap_or(5).clamp(1, 60)),
    }
}

fn rejected(e: &XmasterError) -> bool {
    matches!(e, XmasterError::Media(m) if m.starts_with(REJECTED))
}

// ---------------------------------------------------------------------------
// Sending
// ---------------------------------------------------------------------------

/// Body of an OAuth1-signed upload request.
enum Body {
    Empty,
    Form(Vec<(&'static str, String)>),
    Json(Value),
    Multipart(reqwest::multipart::Form),
}

/// Every OAuth1-signed upload request goes through here: the one place to
/// swap the signing transport.
async fn oauth1_send(
    ctx: &AppContext,
    method: Method,
    url: &str,
    body: Body,
    timeout: Duration,
) -> Result<reqwest::Response, XmasterError> {
    let k = &ctx.config.keys;
    let secrets = reqwest_oauth1::Secrets::new(&k.api_key, &k.api_secret)
        .token(&k.access_token, &k.access_token_secret);
    let client = ctx.client.clone().oauth1(secrets);
    let req = if method == Method::GET { client.get(url) } else { client.post(url) };
    let req = match body {
        Body::Empty => req,
        Body::Form(form) => req.form(&form),
        Body::Json(v) => req.header("content-type", "application/json").body(v.to_string()),
        Body::Multipart(form) => req.multipart(form),
    };
    Ok(req.timeout(timeout).send().await?)
}

fn multipart(fields: Vec<(&'static str, String)>, chunk: &[u8]) -> reqwest::multipart::Form {
    let part = reqwest::multipart::Part::bytes(chunk.to_vec())
        .file_name("blob")
        .mime_str("application/octet-stream")
        .expect("static MIME type parses");
    fields
        .into_iter()
        .fold(reqwest::multipart::Form::new(), |form, (k, v)| form.text(k, v))
        .part("media", part)
}

struct Uploader<'a> {
    ctx: &'a AppContext,
    route: Route,
    /// The browser session, for the web route.
    web: Option<Web<'a>>,
}

impl<'a> Uploader<'a> {
    fn new(ctx: &'a AppContext, route: Route) -> Result<Self, XmasterError> {
        let web = if route == Route::Web { Some(Web::new(ctx)?) } else { None };
        Ok(Self { ctx, route, web })
    }

    fn err(&self, step: &str, msg: impl std::fmt::Display) -> XmasterError {
        XmasterError::Media(format!("{} {step} failed: {msg}", self.route.name()))
    }

    /// Send one call (with `chunk` for APPEND) and return its JSON body
    /// (`Null` when X answers 204/empty, as APPEND does).
    async fn send(&self, step: &str, call: Call, chunk: Option<&[u8]>) -> Result<Value, XmasterError> {
        let base = self.ctx.config.settings.timeout;
        let timeout = Duration::from_secs(if chunk.is_some() { base.max(CHUNK_TIMEOUT_SECS) } else { base });
        let chunk = chunk.unwrap_or_default();
        let resp = match &self.web {
            Some(web) => {
                let mut req = self.ctx.client.request(call.method.clone(), &call.url).timeout(timeout);
                match call.body {
                    CallBody::Empty => {}
                    CallBody::Chunk(fields) => req = req.multipart(multipart(fields, chunk)),
                    other => return Err(self.err(step, format!("unexpected web request body {other:?}"))),
                }
                let mut req = web
                    .upload_headers(req, call.method.as_str(), WEB_UPLOAD_PATH)
                    .await?
                    .build()?;
                // upload.x.com is a sibling of the x.com page that calls it
                req.headers_mut().insert("sec-fetch-site", HeaderValue::from_static("same-site"));
                self.ctx.client.execute(req).await?
            }
            None => {
                let body = match call.body {
                    CallBody::Empty => Body::Empty,
                    CallBody::Form(f) => Body::Form(f),
                    CallBody::Json(v) => Body::Json(v),
                    CallBody::Chunk(fields) => Body::Multipart(multipart(fields, chunk)),
                    CallBody::Chunk64(mut fields) => {
                        fields.push(("media_data", base64::engine::general_purpose::STANDARD.encode(chunk)));
                        Body::Form(fields)
                    }
                };
                oauth1_send(self.ctx, call.method, &call.url, body, timeout).await?
            }
        };

        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if text.trim_start().starts_with('<') {
            return Err(self.err(step, format!("X served a web page (HTTP {status}) instead of data")));
        }
        if !status.is_success() {
            return Err(self.err(step, format!("HTTP {status}: {}", crate::utils::safe_truncate(&text, 300))));
        }
        Ok(serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    /// INIT → APPEND each chunk → FINALIZE → poll STATUS while X processes.
    async fn run(&self, f: &MediaFile) -> Result<String, XmasterError> {
        let init = self.send("INIT", self.route.init(f), None).await?;
        let id = media_id(&init)
            .ok_or_else(|| self.err("INIT", format!("no media ID in {}", crate::utils::safe_truncate(&init.to_string(), 200))))?;

        let size = self.route.chunk_size();
        let total = f.bytes.len().div_ceil(size);
        for (i, chunk) in f.bytes.chunks(size).enumerate() {
            if f.bytes.len() > 5 * MIB {
                eprintln!("  Uploading chunk {}/{total} ...", i + 1);
            }
            self.send("APPEND", self.route.append(&id, i), Some(chunk)).await?;
        }

        let fin = self.send("FINALIZE", self.route.finalize(&id), None).await?;
        let mut state = processing(processing_info(&fin));
        let (mut polls, mut waited) = (0u32, 0u64);
        loop {
            match state {
                Processing::Done => return Ok(id),
                Processing::Failed(msg) => return Err(XmasterError::Media(format!("{REJECTED}: {msg}"))),
                Processing::Wait(secs) => {
                    polls += 1;
                    waited += secs;
                    if polls > MAX_POLLS || waited > MAX_POLL_SECS {
                        return Err(self.err("processing", "timed out waiting for X to process the media"));
                    }
                    tokio::time::sleep(Duration::from_secs(secs)).await;
                    let st = self.send("STATUS", self.route.status(&id), None).await?;
                    state = processing(processing_info(&st));
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Entry points
// ---------------------------------------------------------------------------

/// Upload one file; returns its media ID.
pub async fn upload(ctx: &AppContext, file_path: &str) -> Result<String, XmasterError> {
    let routes = routes(&ctx.config);
    if routes.is_empty() {
        return Err(XmasterError::AuthMissing {
            provider: "x",
            message: "Media upload needs X web-session cookies (xmaster config web-login) \
                      or X API credentials"
                .into(),
        });
    }
    let file = MediaFile::load(file_path).await?;

    let mut failures: Vec<(Route, XmasterError)> = Vec::new();
    for route in routes {
        let result = match Uploader::new(ctx, route) {
            Ok(up) => up.run(&file).await,
            Err(e) => Err(e),
        };
        match result {
            Ok(id) => return Ok(id),
            Err(e) if rejected(&e) => return Err(e),
            Err(e) => {
                warn!("Media upload via {} failed ({e}); trying the next route", route.name());
                failures.push((route, e));
            }
        }
    }
    if failures.len() == 1 {
        return Err(failures.remove(0).1);
    }
    Err(XmasterError::Media(
        failures
            .iter()
            .map(|(r, e)| format!("{}: {e}", r.name()))
            .collect::<Vec<_>>()
            .join("; "),
    ))
}

/// Upload several files, up to `MAX_PARALLEL` at once; IDs come back in the
/// order of `paths`. The first failure aborts the rest.
pub async fn upload_many(ctx: &Arc<AppContext>, paths: &[String]) -> Result<Vec<String>, XmasterError> {
    let gate = Arc::new(tokio::sync::Semaphore::new(MAX_PARALLEL));
    let mut tasks = tokio::task::JoinSet::new();
    for (i, path) in paths.iter().enumerate() {
        let (ctx, gate, path) = (Arc::clone(ctx), Arc::clone(&gate), path.clone());
        tasks.spawn(async move {
            let _permit = gate.acquire_owned().await;
            (i, upload(&ctx, &path).await)
        });
    }
    let mut ids = vec![String::new(); paths.len()];
    while let Some(done) = tasks.join_next().await {
        let (i, result) = done.map_err(|e| XmasterError::Media(format!("Upload task failed: {e}")))?;
        ids[i] = result?;
    }
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(mime: &'static str, category: &'static str, len: usize) -> MediaFile {
        MediaFile { mime, category, bytes: vec![0; len] }
    }

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";
    const MP4: &[u8] = b"\0\0\0\x20ftypisom\0\0\x02\0";
    const MOV: &[u8] = b"\0\0\0\x14ftypqt  \0\0\0\0";
    const HEIC: &[u8] = b"\0\0\0\x18ftypheic\0\0\0\0";

    #[test]
    fn sniffs_types_from_magic_bytes() {
        assert_eq!(sniff(PNG), Some("image/png"));
        assert_eq!(sniff(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("image/jpeg"));
        assert_eq!(sniff(b"GIF89a\x01\0"), Some("image/gif"));
        assert_eq!(sniff(b"RIFF\0\0\0\0WEBPVP8 "), Some("image/webp"));
        assert_eq!(sniff(MP4), Some("video/mp4"));
        assert_eq!(sniff(MOV), Some("video/quicktime"));
        assert_eq!(sniff(HEIC), None);
        assert_eq!(sniff(b"hello"), None);
    }

    #[test]
    fn content_beats_extension_and_extension_is_case_blind() {
        let f = MediaFile::new(Path::new("shot.jpg"), PNG.to_vec()).unwrap();
        assert_eq!((f.mime, f.category), ("image/png", "tweet_image"));
        // Unknown bytes fall back to the extension, whatever its case
        let f = MediaFile::new(Path::new("CLIP.MOV"), vec![1, 2, 3]).unwrap();
        assert_eq!((f.mime, f.category), ("video/quicktime", "tweet_video"));
        assert!(MediaFile::new(Path::new("photo.heic"), HEIC.to_vec()).is_err());
        assert!(MediaFile::new(Path::new("notes.txt"), b"hi".to_vec()).is_err());
    }

    #[test]
    fn category_and_size_caps() {
        let gif = |len: usize| {
            let mut b = b"GIF89a".to_vec();
            b.resize(len, 0);
            MediaFile::new(Path::new("a.gif"), b)
        };
        // GIFs are tweet_gif at any size, up to 15 MB
        assert_eq!(gif(100).unwrap().category, "tweet_gif");
        assert_eq!(gif(6 * MIB).unwrap().category, "tweet_gif");
        assert!(gif(16 * MIB).is_err());

        let mut png = PNG.to_vec();
        png.resize(6 * MIB, 0);
        let err = MediaFile::new(Path::new("a.png"), png).unwrap_err().to_string();
        assert!(err.contains("max 5MB for image"), "{err}");

        let mut mp4 = MP4.to_vec();
        mp4.resize(20 * MIB, 0);
        assert_eq!(MediaFile::new(Path::new("a.mp4"), mp4).unwrap().category, "tweet_video");
    }

    #[test]
    fn web_requests_match_x_com() {
        let f = file("image/png", "tweet_image", 1234);
        let init = Route::Web.init(&f);
        assert_eq!(init.method, Method::POST);
        assert_eq!(
            init.url,
            "https://upload.x.com/i/media/upload.json?command=INIT&total_bytes=1234\
             &media_type=image%2Fpng&media_category=tweet_image"
        );
        assert_eq!(init.body, CallBody::Empty);

        let append = Route::Web.append("99", 2);
        assert_eq!(append.url, "https://upload.x.com/i/media/upload.json?command=APPEND&media_id=99&segment_index=2");
        assert_eq!(append.body, CallBody::Chunk(vec![]));

        let fin = Route::Web.finalize("99");
        assert_eq!(fin.url, "https://upload.x.com/i/media/upload.json?command=FINALIZE&media_id=99");
        let st = Route::Web.status("99");
        assert_eq!(st.method, Method::GET);
        assert_eq!(st.url, "https://upload.x.com/i/media/upload.json?command=STATUS&media_id=99");
    }

    #[test]
    fn v2_requests_match_docs() {
        let f = file("video/mp4", "tweet_video", 1_048_576);
        let init = Route::V2.init(&f);
        assert_eq!(init.url, "https://api.x.com/2/media/upload/initialize");
        assert_eq!(
            init.body,
            CallBody::Json(json!({ "media_type": "video/mp4", "total_bytes": 1_048_576, "media_category": "tweet_video" }))
        );
        let append = Route::V2.append("188", 0);
        assert_eq!(append.url, "https://api.x.com/2/media/upload/188/append");
        assert_eq!(append.body, CallBody::Chunk(vec![("segment_index", "0".into())]));
        assert_eq!(Route::V2.finalize("188").url, "https://api.x.com/2/media/upload/188/finalize");
        assert_eq!(Route::V2.finalize("188").body, CallBody::Empty);
        let st = Route::V2.status("188");
        assert_eq!((st.method, st.url.as_str()), (Method::GET, "https://api.x.com/2/media/upload?command=STATUS&media_id=188"));
    }

    #[test]
    fn v1_requests_keep_the_legacy_form() {
        let f = file("image/gif", "tweet_gif", 10);
        let init = Route::V1.init(&f);
        assert_eq!(init.url, V1_UPLOAD);
        assert_eq!(
            init.body,
            CallBody::Form(vec![
                ("command", "INIT".into()),
                ("media_type", "image/gif".into()),
                ("total_bytes", "10".into()),
                ("media_category", "tweet_gif".into()),
            ])
        );
        assert_eq!(
            Route::V1.append("7", 1).body,
            CallBody::Chunk64(vec![("command", "APPEND".into()), ("media_id", "7".into()), ("segment_index", "1".into())])
        );
        assert_eq!(
            Route::V1.status("7").url,
            "https://upload.twitter.com/1.1/media/upload.json?command=STATUS&media_id=7"
        );
    }

    #[test]
    fn segment_sizing() {
        let segments = |route: Route, len: usize| vec![0u8; len].chunks(route.chunk_size()).count();
        assert_eq!(segments(Route::Web, 100), 1);
        assert_eq!(segments(Route::Web, 5 * MIB + 1), 6);
        assert_eq!(segments(Route::V2, 4 * MIB), 1);
        assert_eq!(segments(Route::V2, 4 * MIB + 1), 2);
        assert!(Route::V2.chunk_size() <= 5 * 1000 * 1000, "v2 recommends at most 5 MB per segment");
    }

    #[test]
    fn route_order_follows_config() {
        let mut cfg = AppConfig::default();
        assert!(routes(&cfg).is_empty());

        cfg.keys.api_key = "k".into();
        cfg.keys.api_secret = "s".into();
        cfg.keys.access_token = "t".into();
        cfg.keys.access_token_secret = "ts".into();
        assert_eq!(routes(&cfg), [Route::V2, Route::V1]);

        cfg.keys.web_ct0 = "c".into();
        cfg.keys.web_auth_token = "a".into();
        for mode in ["always", "auto"] {
            cfg.settings.web_writes = mode.into();
            assert_eq!(routes(&cfg), [Route::Web, Route::V2, Route::V1], "{mode}");
        }
        cfg.settings.web_writes = "never".into();
        assert_eq!(routes(&cfg), [Route::V2, Route::V1]);

        // Cookies alone are enough on the web route
        cfg.settings.web_writes = "always".into();
        cfg.keys.api_key.clear();
        assert_eq!(routes(&cfg), [Route::Web]);
    }

    #[test]
    fn parses_media_ids() {
        // v1.1 / web INIT
        let v1 = json!({ "media_id": 710511363345354753u64, "media_id_string": "710511363345354753", "expires_after_secs": 86400 });
        assert_eq!(media_id(&v1).as_deref(), Some("710511363345354753"));
        // Numeric only
        assert_eq!(media_id(&json!({ "media_id": 42 })).as_deref(), Some("42"));
        // v2 initialize
        let v2 = json!({ "data": { "id": "1880028106020515840", "media_key": "13_1880028106020515840", "expires_after_secs": 86400 } });
        assert_eq!(media_id(&v2).as_deref(), Some("1880028106020515840"));
        assert_eq!(media_id(&json!({ "errors": [] })), None);
        assert_eq!(media_id(&Value::Null), None);
    }

    #[test]
    fn processing_states() {
        // Images: FINALIZE carries no processing_info
        let image = json!({ "media_id_string": "1", "size": 1234 });
        assert_eq!(processing(processing_info(&image)), Processing::Done);

        // v2 FINALIZE nests it under data
        let v2 = json!({ "data": { "id": "1", "processing_info": { "state": "pending", "check_after_secs": 1 } } });
        assert_eq!(processing(processing_info(&v2)), Processing::Wait(1));

        // v1.1 / web STATUS at the top level
        let busy = json!({ "processing_info": { "state": "in_progress", "check_after_secs": 10, "progress_percent": 8 } });
        assert_eq!(processing(processing_info(&busy)), Processing::Wait(10));
        let done = json!({ "processing_info": { "state": "succeeded", "progress_percent": 100 } });
        assert_eq!(processing(processing_info(&done)), Processing::Done);
        let failed = json!({ "processing_info": { "state": "failed", "error": { "code": 1, "name": "InvalidMedia", "message": "Unsupported video format" } } });
        assert_eq!(processing(processing_info(&failed)), Processing::Failed("Unsupported video format".into()));

        // A zero or huge wait is clamped
        let zero = json!({ "processing_info": { "state": "pending", "check_after_secs": 0 } });
        assert_eq!(processing(processing_info(&zero)), Processing::Wait(1));
    }

    #[test]
    fn only_decoded_rejections_stop_the_fallback() {
        assert!(rejected(&XmasterError::Media(format!("{REJECTED}: Unsupported video format"))));
        assert!(!rejected(&XmasterError::Media("API v2 INIT failed: HTTP 403 Forbidden".into())));
        assert!(!rejected(&XmasterError::Config("x".into())));
    }

    #[test]
    fn web_chunk_is_a_blob_part() {
        // Builds without panicking (the MIME literal parses)
        let _ = multipart(vec![("segment_index", "0".into())], b"abc");
    }
}
