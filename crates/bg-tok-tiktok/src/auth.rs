//! TikTok authorization helpers: official Login Kit v2 (OAuth 2.0) and Web QR login.
//!
//! Passwords are never collected, third-party app cookies are never extracted, and
//! no CAPTCHA or anti-bot checks are bypassed. All secrets are stored in zeroized
//! containers and omitted from `Debug` output.
use crate::error::map_network_error;
use bg_tok::{account::Secret, network::Transport, provider::ProviderError};
use bytes::Bytes;
use futures_util::StreamExt;
use http::{header, HeaderMap, Method};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use tokio_util::sync::CancellationToken;
use url::Url;
use zeroize::Zeroizing;

const DEFAULT_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

/// Configuration for TikTok Login Kit v2 OAuth 2.0.
#[derive(Clone, Debug)]
pub struct LoginKitConfig {
    pub client_key: String,
    pub redirect_uri: String,
    pub scopes: Vec<String>,
}

/// OAuth 2.0 tokens returned by TikTok Login Kit v2.
#[derive(Clone)]
pub struct OAuthTokens {
    pub open_id: String,
    pub access_token: Zeroizing<String>,
    pub refresh_token: Option<Zeroizing<String>>,
    pub expires_in: u64,
    pub refresh_expires_in: Option<u64>,
    pub scope: String,
    pub token_type: String,
}

impl std::fmt::Debug for OAuthTokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuthTokens")
            .field("open_id", &self.open_id)
            .field("access_token", &"[redacted]")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "[redacted]"),
            )
            .field("expires_in", &self.expires_in)
            .field("scope", &self.scope)
            .finish()
    }
}

/// Account session state serialized into [`bg_tok::account::Secret`].
#[derive(Clone, Default)]
pub struct TikTokSession {
    pub oauth: Option<OAuthTokens>,
    pub cookies: BTreeMap<String, Zeroizing<String>>,
}

impl std::fmt::Debug for TikTokSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TikTokSession")
            .field("has_oauth", &self.oauth.is_some())
            .field("cookie_count", &self.cookies.len())
            .finish()
    }
}

#[derive(Serialize, Deserialize)]
struct RawOAuthTokens {
    open_id: String,
    access_token: String,
    refresh_token: Option<String>,
    expires_in: u64,
    refresh_expires_in: Option<u64>,
    scope: String,
    token_type: String,
}

#[derive(Serialize, Deserialize)]
struct RawSession {
    oauth: Option<RawOAuthTokens>,
    cookies: BTreeMap<String, String>,
}

impl TikTokSession {
    /// Serializes this session into a [`Secret`] for host [`bg_tok::account::SecureStore`].
    pub fn to_secret(&self) -> Result<Secret, ProviderError> {
        let raw = RawSession {
            oauth: self.oauth.as_ref().map(|o| RawOAuthTokens {
                open_id: o.open_id.clone(),
                access_token: o.access_token.as_str().to_string(),
                refresh_token: o.refresh_token.as_ref().map(|r| r.as_str().to_string()),
                expires_in: o.expires_in,
                refresh_expires_in: o.refresh_expires_in,
                scope: o.scope.clone(),
                token_type: o.token_type.clone(),
            }),
            cookies: self
                .cookies
                .iter()
                .map(|(k, v)| (k.clone(), v.as_str().to_string()))
                .collect(),
        };
        let json = serde_json::to_vec(&raw).map_err(|_| ProviderError::Storage)?;
        Ok(Secret::new(json))
    }

    /// Deserializes a session from a [`Secret`].
    pub fn from_secret(secret: &Secret) -> Result<Self, ProviderError> {
        let raw: RawSession =
            serde_json::from_slice(secret.expose()).map_err(|_| ProviderError::Storage)?;
        Ok(Self {
            oauth: raw.oauth.map(|o| OAuthTokens {
                open_id: o.open_id,
                access_token: Zeroizing::new(o.access_token),
                refresh_token: o.refresh_token.map(Zeroizing::new),
                expires_in: o.expires_in,
                refresh_expires_in: o.refresh_expires_in,
                scope: o.scope,
                token_type: o.token_type,
            }),
            cookies: raw
                .cookies
                .into_iter()
                .map(|(k, v)| (k, Zeroizing::new(v)))
                .collect(),
        })
    }
}

/// QR challenge returned by TikTok Web QR login.
#[derive(Clone)]
pub struct QrChallenge {
    pub token: String,
    pub qrcode_base64_png: String,
    pub qrcode_index_url: Option<String>,
    pub expire_time: Option<u64>,
}

impl std::fmt::Debug for QrChallenge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QrChallenge")
            .field("token", &"[redacted]")
            .field("has_png", &!self.qrcode_base64_png.is_empty())
            .field("expire_time", &self.expire_time)
            .finish()
    }
}

/// Polling state for TikTok Web QR login.
#[derive(Clone, Debug)]
pub enum QrStatus {
    New,
    Scanned,
    Confirmed(TikTokSession),
    Expired,
    Cancelled,
}

/// Builds the official TikTok Login Kit v2 authorization URL for a system browser / ASWebAuthenticationSession / Custom Tab.
pub fn build_authorize_url(
    config: &LoginKitConfig,
    state: &str,
    code_challenge: Option<&str>,
) -> Result<Url, ProviderError> {
    let mut url = Url::parse("https://www.tiktok.com/v2/auth/authorize/")
        .map_err(|_| ProviderError::InvalidResponse)?;
    let scope = config.scopes.join(",");
    {
        let mut pairs = url.query_pairs_mut();
        pairs.append_pair("client_key", &config.client_key);
        pairs.append_pair("response_type", "code");
        pairs.append_pair("scope", &scope);
        pairs.append_pair("redirect_uri", &config.redirect_uri);
        pairs.append_pair("state", state);
        if let Some(challenge) = code_challenge {
            pairs.append_pair("code_challenge", challenge);
            pairs.append_pair("code_challenge_method", "S256");
        }
    }
    Ok(url)
}

/// Exchanges an OAuth 2.0 authorization code for tokens via `POST https://open.tiktokapis.com/v2/oauth/token/`.
pub async fn exchange_code<T: Transport + ?Sized>(
    transport: &T,
    config: &LoginKitConfig,
    client_secret: &str,
    code: &str,
    code_verifier: Option<&str>,
    cancel: CancellationToken,
) -> Result<OAuthTokens, ProviderError> {
    let mut form = vec![
        ("client_key", config.client_key.as_str()),
        ("client_secret", client_secret),
        ("code", code),
        ("grant_type", "authorization_code"),
        ("redirect_uri", config.redirect_uri.as_str()),
    ];
    if let Some(verifier) = code_verifier {
        form.push(("code_verifier", verifier));
    }
    post_oauth_token(transport, &form, cancel).await
}

/// Refreshes an OAuth 2.0 access token via `POST https://open.tiktokapis.com/v2/oauth/token/`.
pub async fn refresh_oauth_token<T: Transport + ?Sized>(
    transport: &T,
    config: &LoginKitConfig,
    client_secret: &str,
    refresh_token: &str,
    cancel: CancellationToken,
) -> Result<OAuthTokens, ProviderError> {
    let form = [
        ("client_key", config.client_key.as_str()),
        ("client_secret", client_secret),
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
    ];
    post_oauth_token(transport, &form, cancel).await
}

async fn post_oauth_token<T: Transport + ?Sized>(
    transport: &T,
    form: &[(&str, &str)],
    cancel: CancellationToken,
) -> Result<OAuthTokens, ProviderError> {
    let url = Url::parse("https://open.tiktokapis.com/v2/oauth/token/")
        .map_err(|_| ProviderError::InvalidResponse)?;
    let encoded = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(form.iter().copied())
        .finish();
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/x-www-form-urlencoded"),
    );
    let response = transport
        .send(
            Method::POST,
            &url,
            headers,
            Some(Bytes::from(encoded)),
            None,
            cancel,
        )
        .await
        .map_err(map_network_error)?;
    let bytes = collect_bounded_body(response.body, 256 * 1024).await?;
    parse_oauth_token_response(&bytes)
}

/// Parses a TikTok Login Kit v2 token response JSON.
pub fn parse_oauth_token_response(bytes: &[u8]) -> Result<OAuthTokens, ProviderError> {
    let root: Value = serde_json::from_slice(bytes).map_err(|_| ProviderError::InvalidResponse)?;
    if let Some(err) = root.get("error").and_then(Value::as_str) {
        if !err.is_empty() && err != "ok" {
            return Err(ProviderError::Unauthorized);
        }
    }
    let access_token = root
        .get("access_token")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or(ProviderError::InvalidResponse)?;
    let open_id = root
        .get("open_id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    Ok(OAuthTokens {
        open_id,
        access_token: Zeroizing::new(access_token.to_string()),
        refresh_token: root
            .get("refresh_token")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(|s| Zeroizing::new(s.to_string())),
        expires_in: root.get("expires_in").and_then(Value::as_u64).unwrap_or(0),
        refresh_expires_in: root.get("refresh_expires_in").and_then(Value::as_u64),
        scope: root
            .get("scope")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        token_type: root
            .get("token_type")
            .and_then(Value::as_str)
            .unwrap_or("Bearer")
            .to_string(),
    })
}

/// Initiates a TikTok Web QR login challenge (`GET /passport/web/get_qrcode/`).
pub async fn start_qr_login<T: Transport + ?Sized>(
    transport: &T,
    cookie_header: Option<&str>,
    cancel: CancellationToken,
) -> Result<QrChallenge, ProviderError> {
    let url = Url::parse("https://www.tiktok.com/passport/web/get_qrcode/?aid=1459&next=https%3A%2F%2Fwww.tiktok.com%2F")
        .map_err(|_| ProviderError::InvalidResponse)?;
    let mut headers = browser_headers();
    if let Some(cookie) = cookie_header {
        if let Ok(val) = header::HeaderValue::from_str(cookie) {
            headers.insert(header::COOKIE, val);
        }
    }
    let response = transport
        .send(Method::GET, &url, headers, None, None, cancel)
        .await
        .map_err(map_network_error)?;
    let bytes = collect_bounded_body(response.body, 512 * 1024).await?;
    parse_qr_challenge(&bytes)
}

/// Parses `/passport/web/get_qrcode/` JSON.
pub fn parse_qr_challenge(bytes: &[u8]) -> Result<QrChallenge, ProviderError> {
    let root: Value = serde_json::from_slice(bytes).map_err(|_| ProviderError::InvalidResponse)?;
    let data = root.get("data").ok_or(ProviderError::InvalidResponse)?;
    let token = data
        .get("token")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or(ProviderError::InvalidResponse)?
        .to_string();
    let qrcode_base64_png = data
        .get("qrcode")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let qrcode_index_url = data
        .get("qrcode_index_url")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let expire_time = data.get("expire_time").and_then(Value::as_u64);
    Ok(QrChallenge {
        token,
        qrcode_base64_png,
        qrcode_index_url,
        expire_time,
    })
}

/// Polls TikTok Web QR login status (`GET /passport/web/check_qrconnect/`).
pub async fn poll_qr_login<T: Transport + ?Sized>(
    transport: &T,
    cookie_header: Option<&str>,
    token: &str,
    cancel: CancellationToken,
) -> Result<QrStatus, ProviderError> {
    let mut url = Url::parse("https://www.tiktok.com/passport/web/check_qrconnect/")
        .map_err(|_| ProviderError::InvalidResponse)?;
    {
        let mut pairs = url.query_pairs_mut();
        pairs.append_pair("aid", "1459");
        pairs.append_pair("token", token);
        pairs.append_pair("next", "https://www.tiktok.com/");
    }
    let mut headers = browser_headers();
    if let Some(cookie) = cookie_header {
        if let Ok(val) = header::HeaderValue::from_str(cookie) {
            headers.insert(header::COOKIE, val);
        }
    }
    let response = transport
        .send(Method::GET, &url, headers, None, None, cancel)
        .await
        .map_err(map_network_error)?;
    let resp_headers = response.headers;
    let bytes = collect_bounded_body(response.body, 256 * 1024).await?;
    parse_qr_poll_response(&bytes, &resp_headers)
}

/// Parses `/passport/web/check_qrconnect/` JSON and extracts `Set-Cookie` on confirmation.
pub fn parse_qr_poll_response(
    bytes: &[u8],
    headers: &HeaderMap,
) -> Result<QrStatus, ProviderError> {
    let root: Value = serde_json::from_slice(bytes).map_err(|_| ProviderError::InvalidResponse)?;
    let status = root
        .get("data")
        .and_then(|d| d.get("status"))
        .and_then(Value::as_str)
        .ok_or(ProviderError::InvalidResponse)?;
    match status {
        "1" | "new" => Ok(QrStatus::New),
        "2" | "scanned" => Ok(QrStatus::Scanned),
        "3" | "confirmed" => {
            let mut session = TikTokSession::default();
            for val in headers.get_all(header::SET_COOKIE) {
                if let Ok(raw) = val.to_str() {
                    if let Some((name_val, _)) = raw.split_once(';').or(Some((raw, ""))) {
                        if let Some((name, value)) = name_val.split_once('=') {
                            session.cookies.insert(
                                name.trim().to_string(),
                                Zeroizing::new(value.trim().to_string()),
                            );
                        }
                    }
                }
            }
            Ok(QrStatus::Confirmed(session))
        }
        "4" | "expired" => Ok(QrStatus::Expired),
        "5" | "cancelled" => Ok(QrStatus::Cancelled),
        _ => Err(ProviderError::InvalidResponse),
    }
}

pub(crate) fn browser_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::USER_AGENT,
        header::HeaderValue::from_static(DEFAULT_USER_AGENT),
    );
    headers.insert(
        header::REFERER,
        header::HeaderValue::from_static("https://www.tiktok.com/"),
    );
    headers
}

pub(crate) async fn collect_bounded_body(
    mut stream: bg_tok::network::ByteStream,
    max_bytes: usize,
) -> Result<Vec<u8>, ProviderError> {
    let mut out = Vec::new();
    while let Some(chunk) = stream.next().await {
        let bytes = chunk.map_err(map_network_error)?;
        if out.len().saturating_add(bytes.len()) > max_bytes {
            return Err(ProviderError::Capacity);
        }
        out.extend_from_slice(&bytes);
    }
    Ok(out)
}
