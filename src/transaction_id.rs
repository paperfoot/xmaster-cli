//! Generate X-Client-Transaction-Id for X GraphQL API requests.
//!
//! Ported from: https://github.com/iSarabjitDhiman/XClientTransaction (MIT)
//! This avoids the Python dependency by implementing the algorithm in pure Rust.

use crate::errors::XmasterError;
use base64::Engine as _;
use sha2::{Digest, Sha256};
use std::f64::consts::PI;

const ADDITIONAL_RANDOM_NUMBER: u8 = 3;
const DEFAULT_KEYWORD: &str = "obfiowerehiring";
const EPOCH_OFFSET: u64 = 1682924400;

/// Generate a transaction ID for an X GraphQL request.
/// Fetches the X homepage and ondemand.s file to extract the required keys.
pub async fn generate(
    client: &reqwest::Client,
    method: &str,
    path: &str,
    ct0: &str,
    auth_token: &str,
) -> Result<String, XmasterError> {
    let cookie = format!("ct0={ct0}; auth_token={auth_token}");
    let ua = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/146.0.0.0 Safari/537.36";

    // 1. Fetch homepage
    let html = client
        .get("https://x.com")
        .header("user-agent", ua)
        .header("cookie", &cookie)
        .header("accept", "text/html,application/xhtml+xml")
        .header("accept-language", "en-US,en;q=0.9")
        .send()
        .await
        .map_err(|e| XmasterError::Api {
            provider: "x-web",
            code: "homepage_fetch",
            message: format!("Failed to fetch X homepage: {e}"),
        })?
        .text()
        .await
        .unwrap_or_default();

    // 2. Extract verification key from <meta name="twitter-site-verification" content="...">
    let key_b64 = extract_meta_content(&html, "twitter-site-verification").ok_or_else(|| {
        XmasterError::Api {
            provider: "x-web",
            code: "no_verification_key",
            message: "No twitter-site-verification meta tag found on homepage".into(),
        }
    })?;
    let key_bytes: Vec<u8> = base64::engine::general_purpose::STANDARD
        .decode(&key_b64)
        .map_err(|e| XmasterError::Api {
            provider: "x-web",
            code: "key_decode",
            message: format!("Failed to decode verification key: {e}"),
        })?;

    // 3. Find ondemand.s file URL from homepage
    let od_index = regex_first_capture(r#",(\d+):["']ondemand\.s["']"#, &html).ok_or_else(|| {
        XmasterError::Api {
            provider: "x-web",
            code: "no_ondemand_index",
            message: "No ondemand.s index found in homepage".into(),
        }
    })?;
    let hash_pattern = format!(r#",{}:\"([0-9a-f]+)\""#, od_index);
    let od_hash = regex_first_capture(&hash_pattern, &html).ok_or_else(|| {
        XmasterError::Api {
            provider: "x-web",
            code: "no_ondemand_hash",
            message: "No ondemand.s hash found in homepage".into(),
        }
    })?;
    let od_url = format!(
        "https://abs.twimg.com/responsive-web/client-web/ondemand.s.{od_hash}a.js"
    );

    // 4. Fetch ondemand.s file
    let od_text = client
        .get(&od_url)
        .header("user-agent", ua)
        .header("cookie", &cookie)
        .send()
        .await
        .map_err(|e| XmasterError::Api {
            provider: "x-web",
            code: "ondemand_fetch",
            message: format!("Failed to fetch ondemand.s: {e}"),
        })?
        .text()
        .await
        .unwrap_or_default();

    // 5. Extract indices from ondemand file
    let indices = extract_indices(&od_text);
    if indices.is_empty() {
        return Err(XmasterError::Api {
            provider: "x-web",
            code: "no_indices",
            message: "No KEY_BYTE indices found in ondemand.s".into(),
        });
    }
    let row_index_idx = indices[0];
    let key_bytes_indices = &indices[1..];

    // 6. Extract SVG animation data from homepage
    let frame_idx = (key_bytes[5] as usize) % 4;
    let svg_paths = extract_anim_svg_paths(&html);
    if frame_idx >= svg_paths.len() {
        return Err(XmasterError::Api {
            provider: "x-web",
            code: "no_svg_frames",
            message: format!(
                "SVG frame index {frame_idx} out of range ({} frames found)",
                svg_paths.len()
            ),
        });
    }

    // Parse the SVG path data — skip first 9 chars ("M0 0 C..." prefix), split on "C"
    let d_attr = &svg_paths[frame_idx];
    let d_trimmed = if d_attr.len() > 9 { &d_attr[9..] } else { d_attr };
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

    // 7. Calculate animation key
    let ri = (key_bytes[row_index_idx] as usize) % 16;
    let frame_time: i64 = key_bytes_indices
        .iter()
        .map(|&i| (key_bytes[i] as i64) % 16)
        .product();
    let frame_time = js_round(frame_time as f64 / 10.0) * 10;

    let row_idx = ri % arr.len().max(1);
    let frame_row = &arr[row_idx];
    let target_time = frame_time as f64 / 4096.0;
    let animation_key = animate(frame_row, target_time);

    // 8. Generate transaction ID
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let time_now = ((now_ms - EPOCH_OFFSET * 1000) / 1000) as u32;
    let time_bytes: Vec<u8> = (0..4).map(|i| ((time_now >> (i * 8)) & 0xFF) as u8).collect();

    let hash_input = format!("{method}!{path}!{time_now}{DEFAULT_KEYWORD}{animation_key}");
    let hash_val = Sha256::digest(hash_input.as_bytes());
    let hash_bytes: Vec<u8> = hash_val[..16].to_vec();

    let rand_byte: u8 = rand::random();
    let mut payload: Vec<u8> = Vec::new();
    payload.extend_from_slice(&key_bytes);
    payload.extend_from_slice(&time_bytes);
    payload.extend_from_slice(&hash_bytes);
    payload.push(ADDITIONAL_RANDOM_NUMBER);

    let mut out = vec![rand_byte];
    for b in &payload {
        out.push(b ^ rand_byte);
    }

    let encoded = base64::engine::general_purpose::STANDARD.encode(&out);
    Ok(encoded.trim_end_matches('=').to_string())
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
