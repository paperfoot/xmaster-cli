//! OAuth 1.0a (HMAC-SHA1) request signing for the X API, per RFC 5849.
//!
//! Replaces the `reqwest-oauth1` crate (about 22 extra crates). The shim
//! (`client.oauth1(secrets).post(url).form(..).send()`) keeps its call shape.

// Not called yet on this branch: xapi.rs and rate_limits.rs switch their
// `reqwest_oauth1` import to this module when wired. Delete this line then.
#![allow(dead_code)]

use base64::Engine as _;
use hmac::{Hmac, Mac};
use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use rand::{distributions::Alphanumeric, Rng};
use reqwest::{header, Client, IntoUrl, Method, Request};
use sha1::Sha1;

/// RFC 5849 §3.6: everything but ALPHA, DIGIT, '-', '.', '_', '~' is escaped.
const RESERVED: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'.').remove(b'_').remove(b'~');

fn enc(s: &str) -> String {
    utf8_percent_encode(s, RESERVED).to_string()
}

/// Consumer key/secret plus the optional access token/secret.
#[derive(Clone, Copy)]
pub struct Secrets<'a> {
    consumer_key: &'a str,
    consumer_secret: &'a str,
    token: &'a str,
    token_secret: &'a str,
}

impl<'a> Secrets<'a> {
    pub fn new(consumer_key: &'a str, consumer_secret: &'a str) -> Self {
        Self { consumer_key, consumer_secret, token: "", token_secret: "" }
    }

    pub fn token(self, token: &'a str, token_secret: &'a str) -> Self {
        Self { token, token_secret, ..self }
    }
}

/// The `Authorization` header value for one request. Query parameters are
/// read from `url`; `form` holds the decoded x-www-form-urlencoded body.
fn authorization(s: &Secrets, method: &str, url: &url::Url, form: &[(String, String)], nonce: &str, ts: u64) -> String {
    let ts = ts.to_string();
    let mut oauth = vec![
        ("oauth_consumer_key", s.consumer_key),
        ("oauth_nonce", nonce),
        ("oauth_signature_method", "HMAC-SHA1"),
        ("oauth_timestamp", ts.as_str()),
        ("oauth_version", "1.0"),
    ];
    if !s.token.is_empty() {
        oauth.push(("oauth_token", s.token));
    }
    let mut params: Vec<(String, String)> = url
        .query_pairs()
        .chain(form.iter().map(|(k, v)| (k.into(), v.into())))
        .map(|(k, v)| (enc(&k), enc(&v)))
        .chain(oauth.iter().map(|(k, v)| (enc(k), enc(v))))
        .collect();
    params.sort();
    let param_str = params.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("&");

    let port = url.port().map(|p| format!(":{p}")).unwrap_or_default();
    let base_url = format!("{}://{}{port}{}", url.scheme(), url.host_str().unwrap_or_default(), url.path());
    let base = format!("{}&{}&{}", method.to_uppercase(), enc(&base_url), enc(&param_str));

    let key = format!("{}&{}", enc(s.consumer_secret), enc(s.token_secret));
    let mut mac = Hmac::<Sha1>::new_from_slice(key.as_bytes()).expect("HMAC takes any key length");
    mac.update(base.as_bytes());
    let sig = base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes());

    oauth.push(("oauth_signature", &sig));
    oauth.sort();
    let fields: Vec<String> = oauth.iter().map(|(k, v)| format!("{k}=\"{}\"", enc(v))).collect();
    format!("OAuth {}", fields.join(", "))
}

fn sign_with(req: &mut Request, s: &Secrets, nonce: &str, ts: u64) {
    let is_form = req
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("application/x-www-form-urlencoded"));
    // Multipart and JSON bodies are not part of the signature (RFC 5849 §3.4.1.3)
    let form: Vec<(String, String)> = match req.body().and_then(|b| b.as_bytes()) {
        Some(bytes) if is_form => url::form_urlencoded::parse(bytes).into_owned().collect(),
        _ => Vec::new(),
    };
    let value = authorization(s, req.method().as_str(), req.url(), &form, nonce, ts);
    req.headers_mut().insert(header::AUTHORIZATION, value.parse().expect("header value is ASCII"));
}

/// Sign a built request in place with a fresh nonce and timestamp.
pub fn sign(req: &mut Request, s: &Secrets) {
    let nonce: String = rand::thread_rng().sample_iter(&Alphanumeric).take(32).map(char::from).collect();
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    sign_with(req, s, &nonce, ts);
}

/// `client.oauth1(secrets)` → a request builder that signs on `send()`.
pub trait OAuthClientProvider {
    fn oauth1(self, secrets: Secrets<'_>) -> Signer<'_>;
}

impl OAuthClientProvider for Client {
    fn oauth1(self, secrets: Secrets<'_>) -> Signer<'_> {
        Signer { client: self, secrets }
    }
}

pub struct Signer<'a> {
    client: Client,
    secrets: Secrets<'a>,
}

impl<'a> Signer<'a> {
    fn request(self, method: Method, url: impl IntoUrl) -> SignedRequest<'a> {
        SignedRequest { inner: self.client.request(method, url), client: self.client, secrets: self.secrets }
    }
    pub fn get(self, url: impl IntoUrl) -> SignedRequest<'a> {
        self.request(Method::GET, url)
    }
    pub fn post(self, url: impl IntoUrl) -> SignedRequest<'a> {
        self.request(Method::POST, url)
    }
    pub fn put(self, url: impl IntoUrl) -> SignedRequest<'a> {
        self.request(Method::PUT, url)
    }
    pub fn delete(self, url: impl IntoUrl) -> SignedRequest<'a> {
        self.request(Method::DELETE, url)
    }
}

pub struct SignedRequest<'a> {
    inner: reqwest::RequestBuilder,
    client: Client,
    secrets: Secrets<'a>,
}

impl SignedRequest<'_> {
    pub fn header(self, key: &str, value: &str) -> Self {
        Self { inner: self.inner.header(key, value), ..self }
    }
    pub fn body(self, body: impl Into<reqwest::Body>) -> Self {
        Self { inner: self.inner.body(body), ..self }
    }
    pub fn form<T: serde::Serialize + ?Sized>(self, form: &T) -> Self {
        Self { inner: self.inner.form(form), ..self }
    }
    pub fn multipart(self, form: reqwest::multipart::Form) -> Self {
        Self { inner: self.inner.multipart(form), ..self }
    }
    pub async fn send(self) -> Result<reqwest::Response, reqwest::Error> {
        let mut req = self.inner.build()?;
        sign(&mut req, &self.secrets);
        self.client.execute(req).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // developer.x.com, "Creating a signature" (OAuth 1.0a)
    const SECRETS: Secrets<'static> = Secrets {
        consumer_key: "xvz1evFS4wEEPTGEFPHBog",
        consumer_secret: "kAcSOqF21Fu85e7zjz7ZN2U4ZRhfV3WpwPAoE3Z7kBw",
        token: "370773112-GmHxMAgYyLbNEtIKZeRNFsMKPR9EyMZeS9weJAEb",
        token_secret: "LswwdoUaIvS8ltyTt5jkRh4J50vUPVVHtR2YPi5kE",
    };
    const NONCE: &str = "kYjzVBB8Y0ZFabxSWbWovY3uYSQ2pTgmZeNu2VS4cg";
    const TS: u64 = 1318622958;
    const URL: &str = "https://api.twitter.com/1.1/statuses/update.json?include_entities=true";
    const STATUS: &str = "Hello Ladies + Gentlemen, a signed OAuth request!";
    const SIGNATURE: &str = "oauth_signature=\"hCtSmYh%2BiHYCEqBWrE7C7hYmtUk%3D\"";

    #[test]
    fn matches_x_docs_signature_example() {
        let form = [("status".to_string(), STATUS.to_string())];
        let h = authorization(&SECRETS, "post", &url::Url::parse(URL).unwrap(), &form, NONCE, TS);
        assert!(h.starts_with("OAuth oauth_consumer_key=\"xvz1evFS4wEEPTGEFPHBog\", oauth_nonce="), "{h}");
        assert!(h.contains(SIGNATURE), "{h}");
        assert!(h.contains("oauth_token=\"370773112-GmHxMAgYyLbNEtIKZeRNFsMKPR9EyMZeS9weJAEb\""));
    }

    #[test]
    fn signs_built_form_request() {
        let mut req = Client::new().post(URL).form(&[("status", STATUS)]).build().unwrap();
        sign_with(&mut req, &SECRETS, NONCE, TS);
        let h = req.headers()[header::AUTHORIZATION].to_str().unwrap();
        assert!(h.contains(SIGNATURE), "{h}");

        // A JSON body is not signed: same URL, no form params → another signature
        let mut req = Client::new().post(URL).body("{}").header("content-type", "application/json").build().unwrap();
        sign_with(&mut req, &SECRETS, NONCE, TS);
        assert!(!req.headers()[header::AUTHORIZATION].to_str().unwrap().contains(SIGNATURE));
    }

    #[test]
    fn rfc5849_encoding() {
        assert_eq!(enc("Ladies + Gentlemen!"), "Ladies%20%2B%20Gentlemen%21");
        assert_eq!(enc("a-b.c_d~e"), "a-b.c_d~e");
        assert_eq!(enc("é"), "%C3%A9");
    }
}
