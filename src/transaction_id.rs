//! X-Client-Transaction-Id for X web requests.
//!
//! Ported from: https://github.com/iSarabjitDhiman/XClientTransaction (MIT)
//! The signing keys come from the logged-in x.com page and its ondemand.s
//! chunk. They are derived once (`derive`) and cached by `providers::web`;
//! each request is then signed locally (`sign`) with no network round trip.

use crate::errors::XmasterError;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::f64::consts::PI;

const ADDITIONAL_RANDOM_NUMBER: u8 = 3;
const DEFAULT_KEYWORD: &str = "obfiowerehiring";
const EPOCH_OFFSET: u64 = 1682924400;

/// Per-page signing material: the site-verification key and the animation key.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TxKeys {
    pub key_bytes: Vec<u8>,
    pub animation_key: String,
}

fn err(code: &'static str, message: String) -> XmasterError {
    XmasterError::Api { provider: "x-web", code, message }
}

/// Script URLs of the newer x-web (Vite) build linked from the page.
pub fn x_web_scripts(html: &str) -> Vec<String> {
    let re = regex::Regex::new(r#"https://[\w.-]+/x-web/[\w./-]+\.js"#).unwrap();
    let mut urls: Vec<String> = re.find_iter(html).map(|m| m.as_str().to_string()).collect();
    urls.dedup();
    urls
}

/// A reference to the signing-indices script inside page or bundle text:
/// `ondemand.s.*.js` (legacy webpack build) or `sign.o-*.js` (x-web build,
/// dynamically imported from a chunk). Resolved against `base` when relative.
pub fn indices_script_ref(text: &str, base: &str) -> Option<String> {
    let re = regex::Regex::new(r#"(?:\.{0,2}/)?[\w./-]*?\b(?:ondemand\.s|sign\.o)[\w.-]*\.js"#).unwrap();
    let found = re.find(text)?.as_str();
    url::Url::parse(base).ok()?.join(found).ok().map(String::from)
}

/// Scripts a bundle imports by relative path (`"./assets/x-1a2b.js"`),
/// resolved against the bundle's own URL.
pub fn relative_imports(text: &str, base: &str) -> Vec<String> {
    let Ok(base) = url::Url::parse(base) else { return Vec::new() };
    let re = regex::Regex::new(r#"["'`](\.{1,2}/[\w./-]+\.js)["'`]"#).unwrap();
    let mut urls: Vec<String> = re
        .captures_iter(text)
        .filter_map(|c| base.join(&c[1]).ok().map(String::from))
        .collect();
    urls.sort();
    urls.dedup();
    urls
}

/// The page is a Cloudflare interstitial rather than the X web app.
pub fn is_challenge_page(html: &str) -> bool {
    html.contains("/cdn-cgi/challenge-platform/") && !html.contains("abs.twimg.com")
}

/// URL of the ondemand.s chunk referenced by the x.com page (legacy build).
pub fn ondemand_url(html: &str) -> Result<String, XmasterError> {
    let od_index = regex_first_capture(r#",(\d+):["']ondemand\.s["']"#, html)
        .ok_or_else(|| err("no_ondemand_index", "No ondemand.s index found in homepage".into()))?;
    let od_hash = regex_first_capture(&format!(r#",{od_index}:\"([0-9a-f]+)\""#), html)
        .ok_or_else(|| err("no_ondemand_hash", "No ondemand.s hash found in homepage".into()))?;
    Ok(format!("https://abs.twimg.com/responsive-web/client-web/ondemand.s.{od_hash}a.js"))
}

/// Derive the signing keys from the x.com page and its ondemand.s chunk.
pub fn derive(html: &str, ondemand_js: &str) -> Result<TxKeys, XmasterError> {
    let key_b64 = extract_meta_content(html, "twitter-site-verification").ok_or_else(|| {
        err("no_verification_key", "No twitter-site-verification meta tag found on homepage".into())
    })?;
    let key_bytes: Vec<u8> = base64::engine::general_purpose::STANDARD
        .decode(&key_b64)
        .map_err(|e| err("key_decode", format!("Failed to decode verification key: {e}")))?;
    if key_bytes.len() < 6 {
        return Err(err("key_decode", "Verification key too short".into()));
    }

    let indices = extract_indices(ondemand_js);
    if indices.is_empty() {
        return Err(err("no_indices", "No KEY_BYTE indices found in ondemand.s".into()));
    }
    let row_index_idx = indices[0];
    let key_bytes_indices = &indices[1..];
    if indices.iter().any(|&i| i >= key_bytes.len()) {
        return Err(err("no_indices", "ondemand.s indices out of range for verification key".into()));
    }

    // SVG animation frame selected by the key
    let frame_idx = (key_bytes[5] as usize) % 4;
    let svg_paths = extract_anim_svg_paths(html);
    let d_attr = svg_paths.get(frame_idx).ok_or_else(|| {
        err(
            "no_svg_frames",
            format!("SVG frame index {frame_idx} out of range ({} frames found)", svg_paths.len()),
        )
    })?;

    // Parse the SVG path data — skip the 9-char "M0 0 C..." prefix, split on "C"
    let d_trimmed = d_attr.get(9..).unwrap_or(d_attr);
    let arr: Vec<Vec<i64>> = d_trimmed
        .split('C')
        .filter(|s| !s.trim().is_empty())
        .map(|part| {
            part.split(|c: char| !c.is_ascii_digit())
                .filter(|s| !s.is_empty())
                .filter_map(|s| s.parse::<i64>().ok())
                .collect()
        })
        .collect();
    if arr.is_empty() {
        return Err(err("no_svg_frames", "Empty SVG animation frame".into()));
    }

    let ri = (key_bytes[row_index_idx] as usize) % 16;
    let frame_time: i64 = key_bytes_indices
        .iter()
        .map(|&i| (key_bytes[i] as i64) % 16)
        .product();
    let frame_time = js_round(frame_time as f64 / 10.0) * 10;
    let frame_row = &arr[ri % arr.len()];
    let animation_key = animate(frame_row, frame_time as f64 / 4096.0);

    Ok(TxKeys { key_bytes, animation_key })
}

/// Sign one request: `method` and `path` (e.g. "/i/api/graphql/ID/CreateTweet").
pub fn sign(keys: &TxKeys, method: &str, path: &str) -> String {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let time_now = ((now_ms - EPOCH_OFFSET * 1000) / 1000) as u32;

    let hash_input = format!("{method}!{path}!{time_now}{DEFAULT_KEYWORD}{}", keys.animation_key);
    let hash_val = Sha256::digest(hash_input.as_bytes());

    let mut payload: Vec<u8> = keys.key_bytes.clone();
    payload.extend_from_slice(&time_now.to_le_bytes());
    payload.extend_from_slice(&hash_val[..16]);
    payload.push(ADDITIONAL_RANDOM_NUMBER);

    let rand_byte: u8 = rand::random();
    let out: Vec<u8> = std::iter::once(rand_byte)
        .chain(payload.iter().map(|b| b ^ rand_byte))
        .collect();

    let encoded = base64::engine::general_purpose::STANDARD.encode(&out);
    encoded.trim_end_matches('=').to_string()
}

// --- Helpers ---

fn extract_meta_content(html: &str, name: &str) -> Option<String> {
    // Match: <meta name="twitter-site-verification" content="VALUE">
    let pattern = format!(
        r#"<meta[^>]*name=["']{name}["'][^>]*content=["']([^"']+)["']"#
    );
    regex_first_capture(&pattern, html)
        .or_else(|| {
            // Also try content before name (HTML attribute order varies)
            let pattern2 = format!(
                r#"<meta[^>]*content=["']([^"']+)["'][^>]*name=["']{name}["']"#
            );
            regex_first_capture(&pattern2, html)
        })
}

fn extract_anim_svg_paths(html: &str) -> Vec<String> {
    // Find SVGs with id="loading-x-anim-N", extract the d attribute from their paths
    let mut paths = Vec::new();
    // Match each loading-x-anim SVG block and extract path d attributes
    let svg_re = regex::Regex::new(
        r#"id=["']loading-x-anim-\d+["'][^>]*>.*?</svg>"#
    ).unwrap();

    let path_d_re = regex::Regex::new(r#"<path[^>]*\sd=["']([^"']+)["']"#).unwrap();

    for svg_match in svg_re.find_iter(html) {
        let svg_text = svg_match.as_str();
        let d_values: Vec<String> = path_d_re
            .captures_iter(svg_text)
            .map(|c| c[1].to_string())
            .collect();
        // Use the second path (index 1) if available, otherwise first
        if d_values.len() >= 2 {
            paths.push(d_values[1].clone());
        } else if !d_values.is_empty() {
            paths.push(d_values[0].clone());
        }
    }
    paths
}

fn extract_indices(text: &str) -> Vec<usize> {
    let re = regex::Regex::new(r#"\(\w\[(\d{1,2})\],\s*16\)"#).unwrap();
    re.captures_iter(text)
        .filter_map(|c| c[1].parse::<usize>().ok())
        .collect()
}

fn regex_first_capture(pattern: &str, text: &str) -> Option<String> {
    regex::Regex::new(pattern)
        .ok()?
        .captures(text)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
}

fn js_round(num: f64) -> i64 {
    let x = num.floor() as i64;
    if (num - x as f64) >= 0.5 {
        num.ceil() as i64
    } else {
        x
    }
}

fn solve(value: f64, min_val: f64, max_val: f64, rounding: bool) -> f64 {
    let result = value * (max_val - min_val) / 255.0 + min_val;
    if rounding {
        result.floor()
    } else {
        (result * 100.0).round() / 100.0
    }
}

fn is_odd_val(num: usize) -> f64 {
    if !num.is_multiple_of(2) { -1.0 } else { 0.0 }
}

fn interpolate(from: &[f64], to: &[f64], f: f64) -> Vec<f64> {
    from.iter()
        .zip(to.iter())
        .map(|(a, b)| a * (1.0 - f) + b * f)
        .collect()
}

fn rotation_matrix(degrees: f64) -> Vec<f64> {
    let rad = degrees * PI / 180.0;
    vec![rad.cos(), -rad.sin(), rad.sin(), rad.cos()]
}

fn float_to_hex(x: f64) -> String {
    let mut result = Vec::new();
    let mut quotient = x as u64;
    let fraction = x - quotient as f64;

    if quotient == 0 && fraction == 0.0 {
        return "0".to_string();
    }

    let mut val = x;
    while quotient > 0 {
        let q = (val / 16.0) as u64;
        let remainder = (val - q as f64 * 16.0) as u8;
        if remainder > 9 {
            result.insert(0, (remainder + 55) as char);
        } else {
            result.insert(0, (remainder + b'0') as char);
        }
        val = q as f64;
        quotient = q;
    }

    if fraction == 0.0 {
        return result.iter().collect();
    }

    result.push('.');
    let mut frac = fraction;
    let mut iterations = 0;
    while frac > 0.0 && iterations < 16 {
        frac *= 16.0;
        let integer = frac as u8;
        frac -= integer as f64;
        if integer > 9 {
            result.push((integer + 55) as char);
        } else {
            result.push((integer + b'0') as char);
        }
        iterations += 1;
    }

    result.iter().collect()
}

struct Cubic {
    curves: Vec<f64>,
}

impl Cubic {
    fn get_value(&self, t: f64) -> f64 {
        if t <= 0.0 {
            if self.curves[0] > 0.0 {
                return (self.curves[1] / self.curves[0]) * t;
            }
            if self.curves[1] == 0.0 && self.curves[2] > 0.0 {
                return (self.curves[3] / self.curves[2]) * t;
            }
            return 0.0;
        }
        if t >= 1.0 {
            if self.curves[2] < 1.0 {
                return 1.0 + ((self.curves[3] - 1.0) / (self.curves[2] - 1.0)) * (t - 1.0);
            }
            if self.curves[2] == 1.0 && self.curves[0] < 1.0 {
                return 1.0 + ((self.curves[1] - 1.0) / (self.curves[0] - 1.0)) * (t - 1.0);
            }
            return 1.0;
        }
        let (mut lo, mut hi) = (0.0_f64, 1.0_f64);
        let mut mid = 0.5;
        while lo < hi {
            mid = (lo + hi) / 2.0;
            let x_est = Self::calc(self.curves[0], self.curves[2], mid);
            if (t - x_est).abs() < 0.00001 {
                return Self::calc(self.curves[1], self.curves[3], mid);
            }
            if x_est < t {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        Self::calc(self.curves[1], self.curves[3], mid)
    }

    fn calc(a: f64, b: f64, m: f64) -> f64 {
        3.0 * a * (1.0 - m) * (1.0 - m) * m + 3.0 * b * (1.0 - m) * m * m + m * m * m
    }
}

fn animate(frames: &[i64], target_time: f64) -> String {
    if frames.len() < 7 {
        return String::new();
    }

    let from_color: Vec<f64> = frames[..3].iter().map(|&x| x as f64).chain(std::iter::once(1.0)).collect();
    let to_color: Vec<f64> = frames[3..6].iter().map(|&x| x as f64).chain(std::iter::once(1.0)).collect();
    let to_rotation = vec![solve(frames[6] as f64, 60.0, 360.0, true)];

    let curves: Vec<f64> = frames[7..]
        .iter()
        .enumerate()
        .map(|(i, &v)| solve(v as f64, is_odd_val(i), 1.0, false))
        .collect();

    if curves.len() < 4 {
        return String::new();
    }

    let cubic = Cubic { curves };
    let val = cubic.get_value(target_time);

    let color: Vec<f64> = interpolate(&from_color, &to_color, val)
        .into_iter()
        .map(|v| v.clamp(0.0, 255.0))
        .collect();
    let rotation = interpolate(&[0.0], &to_rotation, val);
    let matrix = rotation_matrix(rotation[0]);

    let mut str_arr: Vec<String> = color[..color.len() - 1]
        .iter()
        .map(|v| format!("{:x}", v.round() as i64))
        .collect();

    for v in &matrix {
        let rv = (v.abs() * 100.0).round() / 100.0;
        let hx = float_to_hex(rv);
        if hx.starts_with('.') {
            str_arr.push(format!("0{}", hx.to_lowercase()));
        } else if hx.is_empty() {
            str_arr.push("0".to_string());
        } else {
            str_arr.push(hx);
        }
    }
    str_arr.extend_from_slice(&["0".to_string(), "0".to_string()]);

    let joined: String = str_arr.join("");
    joined.replace(['.', '-'], "")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_legacy_ondemand_chunk() {
        let html = r#"{59862:"ondemand.LottieWeb",59924:"ondemand.s",60041:"i18n/emoji"},{1:"0f0f0f0",59924:"18fd6887aa235d97"}"#;
        assert_eq!(
            ondemand_url(html).unwrap(),
            "https://abs.twimg.com/responsive-web/client-web/ondemand.s.18fd6887aa235d97a.js"
        );
    }

    #[test]
    fn finds_x_web_signing_script() {
        let html = r#"<script type="module" src="https://abs.twimg.com/x-web/x-web/entry-client-DmnmR8WH.js"></script>"#;
        assert_eq!(x_web_scripts(html), ["https://abs.twimg.com/x-web/x-web/entry-client-DmnmR8WH.js"]);
        let chunk = r#"import("./sign.o-Bq3xYz12.js").then(m=>m.d)"#;
        assert_eq!(
            indices_script_ref(chunk, "https://abs.twimg.com/x-web/x-web/assets/app-1.js").as_deref(),
            Some("https://abs.twimg.com/x-web/x-web/assets/sign.o-Bq3xYz12.js")
        );
        assert!(indices_script_ref(r#"import("./design.o-AB.js")"#, "https://a.b/c.js").is_none());
        let entry = r#"import"./assets/client-transaction-id-plugin-l8.js";const a=()=>import(`./assets/app-1.js`),b="../x.js";"#;
        assert_eq!(
            relative_imports(entry, "https://abs.twimg.com/x-web/x-web/entry-client-D1.js"),
            [
                "https://abs.twimg.com/x-web/x-web/assets/app-1.js",
                "https://abs.twimg.com/x-web/x-web/assets/client-transaction-id-plugin-l8.js",
                "https://abs.twimg.com/x-web/x.js",
            ]
        );
    }

    #[test]
    fn derive_and_sign_round_trip() {
        let key = base64::engine::general_purpose::STANDARD.encode((0u8..48).collect::<Vec<_>>());
        let path = "M 10,30 C 20,40 30,50 40,60 70,80 90,100 110,120 130,140 150,160 170,180 190,200 210,220";
        let svgs: String = (0..4)
            .map(|i| format!(r#"<svg id="loading-x-anim-{i}"><path d="M0 0"/><path d="{path}"/></svg>"#))
            .collect();
        let html = format!(r#"<meta name="twitter-site-verification" content="{key}"/>{svgs}"#);
        let od = "(a[2], 16),(a[7], 16),(a[11], 16),(a[30], 16)";
        let keys = derive(&html, od).unwrap();
        assert_eq!(keys.key_bytes.len(), 48);
        let tx = sign(&keys, "POST", "/i/api/graphql/x/CreateTweet");
        // 1 random byte + 48 key + 4 time + 16 hash + 1 constant = 70 bytes
        assert_eq!(base64::engine::general_purpose::STANDARD_NO_PAD.decode(&tx).unwrap().len(), 70);
    }
}
