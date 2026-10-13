//! Media upload (images, GIFs, video) for posts, threads, scheduled posts and
//! Article drafts. `XApi::upload_media` delegates here.

use crate::context::AppContext;
use crate::errors::XmasterError;
use base64::Engine as _;
use reqwest::Method;
use reqwest_oauth1::OAuthClientProvider;
use serde::Deserialize;
use serde_json::Value;
use std::path::Path;

const UPLOAD_URL: &str = "https://upload.twitter.com/1.1/media/upload.json";

#[derive(Deserialize)]
struct MediaUploadResponse {
    media_id_string: Option<String>,
    media_id: Option<u64>,
}

/// Body of an OAuth1-signed upload request.
enum Body {
    Empty,
    Form(Vec<(&'static str, String)>),
    Multipart(reqwest::multipart::Form),
}

/// Every OAuth1-signed upload request goes through here: the one place to
/// swap the signing transport.
async fn oauth1_send(
    ctx: &AppContext,
    method: Method,
    url: &str,
    body: Body,
) -> Result<reqwest::Response, XmasterError> {
    let k = &ctx.config.keys;
    let secrets = reqwest_oauth1::Secrets::new(&k.api_key, &k.api_secret)
        .token(&k.access_token, &k.access_token_secret);
    let client = ctx.client.clone().oauth1(secrets);
    let req = if method == Method::GET { client.get(url) } else { client.post(url) };
    let req = match body {
        Body::Empty => req,
        Body::Form(form) => req.form(&form),
        Body::Multipart(form) => req.multipart(form),
    };
    Ok(req.send().await?)
}

pub async fn upload(ctx: &AppContext, file_path: &str) -> Result<String, XmasterError> {
    if !ctx.config.has_x_auth() {
        return Err(XmasterError::AuthMissing {
            provider: "x",
            message: "X API credentials not configured".into(),
        });
    }

    let path = Path::new(file_path);
    if !path.exists() {
        return Err(XmasterError::Media(format!("File not found: {file_path}")));
    }

    let file_bytes = tokio::fs::read(path).await?;
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "media".into());

    let mime = match path.extension().and_then(|e| e.to_str()) {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("mp4") => "video/mp4",
        Some("mov") => "video/quicktime",
        _ => "application/octet-stream",
    };

    let is_video = mime.starts_with("video/");
    let category = if is_video {
        "tweet_video"
    } else if mime == "image/gif" {
        // Without this, animated GIFs over the simple-upload threshold
        // were chunk-uploaded as tweet_image and lost their animation.
        "tweet_gif"
    } else {
        "tweet_image"
    };

    // Validate file size against X upload limits
    let max_size = if is_video {
        512 * 1024 * 1024
    } else if mime == "image/gif" {
        15 * 1024 * 1024
    } else {
        5 * 1024 * 1024
    };
    if file_bytes.len() > max_size {
        return Err(XmasterError::Media(format!(
            "File too large: {}MB (max {}MB for {})",
            file_bytes.len() / 1024 / 1024,
            max_size / 1024 / 1024,
            if is_video { "video" } else { "image" },
        )));
    }

    // For small images, use simple upload
    if !is_video && file_bytes.len() < 5_000_000 {
        return simple_upload(ctx, &file_bytes, &file_name).await;
    }

    // Chunked upload: INIT → APPEND → FINALIZE
    chunked_upload(ctx, &file_bytes, mime, category).await
}

async fn simple_upload(
    ctx: &AppContext,
    data: &[u8],
    file_name: &str,
) -> Result<String, XmasterError> {
    let part = reqwest::multipart::Part::bytes(data.to_vec()).file_name(file_name.to_string());
    let form = reqwest::multipart::Form::new().part("media", part);

    let resp = oauth1_send(ctx, Method::POST, UPLOAD_URL, Body::Multipart(form)).await?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(XmasterError::Media(format!(
            "Upload failed (HTTP {status}): {text}"
        )));
    }

    let upload: MediaUploadResponse = resp.json().await?;
    upload
        .media_id_string
        .or_else(|| upload.media_id.map(|id| id.to_string()))
        .ok_or_else(|| XmasterError::Media("No media_id in upload response".into()))
}

async fn chunked_upload(
    ctx: &AppContext,
    data: &[u8],
    mime: &str,
    category: &str,
) -> Result<String, XmasterError> {
    // INIT
    let form = vec![
        ("command", "INIT".to_string()),
        ("media_type", mime.to_string()),
        ("total_bytes", data.len().to_string()),
        ("media_category", category.to_string()),
    ];
    let resp = oauth1_send(ctx, Method::POST, UPLOAD_URL, Body::Form(form)).await?;

    if !resp.status().is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(XmasterError::Media(format!("INIT failed: {text}")));
    }

    let init_resp: MediaUploadResponse = resp.json().await?;
    let media_id = init_resp
        .media_id_string
        .or_else(|| init_resp.media_id.map(|id| id.to_string()))
        .ok_or_else(|| XmasterError::Media("No media_id from INIT".into()))?;

    // APPEND in 1MB chunks
    let chunk_size = 1024 * 1024;
    let total_chunks = data.len().div_ceil(chunk_size);
    for (i, chunk) in data.chunks(chunk_size).enumerate() {
        if data.len() > 5_000_000 {
            eprintln!("  Uploading chunk {}/{} ...", i + 1, total_chunks);
        }
        let form = vec![
            ("command", "APPEND".to_string()),
            ("media_id", media_id.clone()),
            ("segment_index", i.to_string()),
            ("media_data", base64::engine::general_purpose::STANDARD.encode(chunk)),
        ];
        let resp = oauth1_send(ctx, Method::POST, UPLOAD_URL, Body::Form(form)).await?;

        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(XmasterError::Media(format!(
                "APPEND segment {i} failed: {text}"
            )));
        }
    }

    // FINALIZE
    let form = vec![("command", "FINALIZE".to_string()), ("media_id", media_id.clone())];
    let resp = oauth1_send(ctx, Method::POST, UPLOAD_URL, Body::Form(form)).await?;

    if !resp.status().is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(XmasterError::Media(format!("FINALIZE failed: {text}")));
    }

    // For video, poll processing_info until complete
    let finalize: Value = resp.json().await?;
    if let Some(info) = finalize.get("processing_info") {
        wait_for_processing(ctx, &media_id, info).await?;
    }

    Ok(media_id)
}

async fn wait_for_processing(
    ctx: &AppContext,
    media_id: &str,
    initial_info: &Value,
) -> Result<(), XmasterError> {
    let mut check_after = initial_info
        .get("check_after_secs")
        .and_then(|v| v.as_u64())
        .unwrap_or(5);

    const MAX_RETRIES: u32 = 30;
    const MAX_TOTAL_SECS: u64 = 300; // 5 minutes
    let mut attempts = 0u32;
    let mut elapsed_secs = 0u64;

    loop {
        tokio::time::sleep(std::time::Duration::from_secs(check_after)).await;
        elapsed_secs += check_after;
        attempts += 1;

        if attempts > MAX_RETRIES || elapsed_secs > MAX_TOTAL_SECS {
            return Err(XmasterError::Media("Upload processing timed out".into()));
        }

        let url = format!("{UPLOAD_URL}?command=STATUS&media_id={media_id}");

        let resp = oauth1_send(ctx, Method::GET, &url, Body::Empty).await?;

        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(XmasterError::Media(format!("STATUS check failed: {text}")));
        }

        let status: Value = resp.json().await?;
        let state = status
            .get("processing_info")
            .and_then(|p| p.get("state"))
            .and_then(|s| s.as_str())
            .unwrap_or("succeeded");

        match state {
            "succeeded" => return Ok(()),
            "failed" => {
                let error = status
                    .get("processing_info")
                    .and_then(|p| p.get("error"))
                    .and_then(|e| e.get("message"))
                    .and_then(|m| m.as_str())
                    .unwrap_or("Unknown processing error");
                return Err(XmasterError::Media(format!(
                    "Media processing failed: {error}"
                )));
            }
            _ => {
                check_after = status
                    .get("processing_info")
                    .and_then(|p| p.get("check_after_secs"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(5);
            }
        }
    }
}
