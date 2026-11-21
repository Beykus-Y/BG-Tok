//! Real TikTok [`ContentProvider`] implementation backed by [`Transport`].
use crate::{
    auth::{browser_headers, collect_bounded_body, TikTokSession},
    error::map_network_error,
    models::{
        CommentPage, DisplayUserInfo, OEmbedInfo, TikTokSourceCandidate, TikTokVideo, VideoRef,
        TIKTOK_NAMESPACE,
    },
    parser::{
        parse_comments_response, parse_display_user_info_response,
        parse_display_video_list_response, parse_explore_response, parse_oembed_response,
        parse_video_detail_html, parse_video_ref,
    },
};
use async_trait::async_trait;
use bg_tok::{
    network::{ByteRange, HttpConfig, HttpResponse, HttpTransport, Transport},
    provider::{
        Action, Capabilities, ContentId, ContentProvider, FeedKind, MediaStream, Page,
        PlaybackEvent, ProviderError, Source,
    },
};
use bytes::Bytes;
use futures_util::StreamExt;
use http::{header, HeaderMap, Method};
use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};
use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;
use url::Url;
use zeroize::Zeroizing;

const MAX_HTML_BYTES: usize = 4 * 1024 * 1024;
const MAX_JSON_BYTES: usize = 2 * 1024 * 1024;

/// Configuration for [`TikTokProvider`].
#[derive(Clone)]
pub struct TikTokConfig {
    pub http: HttpConfig,
    pub user_agent: String,
    pub max_source_handles: usize,
    pub min_request_interval: Duration,
}

impl Default for TikTokConfig {
    fn default() -> Self {
        Self {
            http: HttpConfig::default(),
            user_agent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36".to_string(),
            max_source_handles: 512,
            min_request_interval: Duration::from_millis(150),
        }
    }
}

impl std::fmt::Debug for TikTokConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TikTokConfig")
            .field("http", &self.http)
            .field("max_source_handles", &self.max_source_handles)
            .field("min_request_interval", &self.min_request_interval)
            .finish()
    }
}

struct SourceRegistry {
    max_entries: usize,
    order: VecDeque<String>,
    entries: HashMap<String, TikTokSourceCandidate>,
}

impl SourceRegistry {
    fn new(max_entries: usize) -> Self {
        Self {
            max_entries: max_entries.max(1),
            order: VecDeque::new(),
            entries: HashMap::new(),
        }
    }

    fn insert(&mut self, candidate: TikTokSourceCandidate) {
        let key = candidate.source_id.clone();
        if !self.entries.contains_key(&key) {
            if self.order.len() >= self.max_entries {
                if let Some(oldest) = self.order.pop_front() {
                    self.entries.remove(&oldest);
                }
            }
            self.order.push_back(key.clone());
        }
        self.entries.insert(key, candidate);
    }

    fn get(&self, source_id: &str) -> Option<&TikTokSourceCandidate> {
        self.entries.get(source_id)
    }

    fn clear(&mut self) {
        self.order.clear();
        self.entries.clear();
    }
}

/// Real TikTok provider implementing [`ContentProvider`].
pub struct TikTokProvider<T: Transport = HttpTransport> {
    transport: Arc<T>,
    config: TikTokConfig,
    cookies: RwLock<BTreeMap<String, Zeroizing<String>>>,
    sources: RwLock<SourceRegistry>,
    last_request: Mutex<Option<Instant>>,
}

impl TikTokProvider<HttpTransport> {
    /// Creates a production [`TikTokProvider`] backed by [`HttpTransport`].
    pub fn new(config: TikTokConfig) -> Result<Self, ProviderError> {
        let transport = HttpTransport::new(config.http.clone()).map_err(map_network_error)?;
        Ok(Self::with_transport(Arc::new(transport), config))
    }
}

impl<T: Transport> TikTokProvider<T> {
    /// Creates a [`TikTokProvider`] with a custom [`Transport`] (e.g. for offline tests).
    pub fn with_transport(transport: Arc<T>, config: TikTokConfig) -> Self {
        let max_handles = config.max_source_handles;
        Self {
            transport,
            config,
            cookies: RwLock::new(BTreeMap::new()),
            sources: RwLock::new(SourceRegistry::new(max_handles)),
            last_request: Mutex::new(None),
        }
    }

    /// Loads session cookies from a [`TikTokSession`] into this provider instance.
    pub async fn apply_session(&self, session: &TikTokSession) {
        let mut guard = self.cookies.write().await;
        guard.clear();
        for (k, v) in &session.cookies {
            guard.insert(k.clone(), v.clone());
        }
    }

    /// Returns the serialized `Cookie` header for the current session, if non-empty.
    pub async fn cookie_header(&self) -> Option<String> {
        let guard = self.cookies.read().await;
        if guard.is_empty() {
            return None;
        }
        let joined = guard
            .iter()
            .map(|(k, v)| format!("{k}={}", v.as_str()))
            .collect::<Vec<_>>()
            .join("; ");
        Some(joined)
    }

    /// Resolves a public TikTok video by numeric ID or URL (`@user/video/ID`, short link, etc.).
    pub async fn resolve_video(
        &self,
        id_or_url: &str,
        cancel: CancellationToken,
    ) -> Result<TikTokVideo, ProviderError> {
        let target_url = match parse_video_ref(id_or_url)? {
            VideoRef::Id(id) => format!("https://www.tiktok.com/@_/video/{id}"),
            VideoRef::Url(url) => url,
        };
        let parsed = Url::parse(&target_url).map_err(|_| ProviderError::InvalidResponse)?;
        let response = self
            .send_with_session(Method::GET, &parsed, HeaderMap::new(), None, None, cancel)
            .await?;
        let body = collect_bounded_body(response.body, MAX_HTML_BYTES).await?;
        let html = String::from_utf8(body).map_err(|_| ProviderError::InvalidResponse)?;
        let video = parse_video_detail_html(&html)?;
        self.register_sources(&video.sources).await;
        Ok(video)
    }

    /// Fetches a page of public Explore videos (NOT a personalized For You feed).
    pub async fn explore_page(
        &self,
        category: u32,
        cursor: Option<&str>,
        cancel: CancellationToken,
    ) -> Result<Page, ProviderError> {
        // Prime session cookies if we have none yet.
        if self.cookies.read().await.is_empty() {
            let explore_url = Url::parse("https://www.tiktok.com/explore")
                .map_err(|_| ProviderError::InvalidResponse)?;
            let resp = self
                .send_with_session(
                    Method::GET,
                    &explore_url,
                    HeaderMap::new(),
                    None,
                    None,
                    cancel.clone(),
                )
                .await?;
            let _ = collect_bounded_body(resp.body, MAX_HTML_BYTES).await?;
        }

        let mut url = Url::parse("https://www.tiktok.com/api/explore/item_list/")
            .map_err(|_| ProviderError::InvalidResponse)?;
        {
            let mut pairs = url.query_pairs_mut();
            pairs.append_pair("aid", "1988");
            pairs.append_pair("categoryType", &category.to_string());
            pairs.append_pair("count", "12");
            if let Some(c) = cursor {
                pairs.append_pair("cursor", c);
            }
        }
        let response = self
            .send_with_session(Method::GET, &url, HeaderMap::new(), None, None, cancel)
            .await?;
        let bytes = collect_bounded_body(response.body, MAX_JSON_BYTES).await?;
        let text = String::from_utf8(bytes).map_err(|_| ProviderError::InvalidResponse)?;
        parse_explore_response(&text, cursor)
    }

    /// Fetches public comments for a video via `/api/comment/list/`.
    pub async fn comments(
        &self,
        id: &ContentId,
        cursor: Option<&str>,
        cancel: CancellationToken,
    ) -> Result<CommentPage, ProviderError> {
        if id.namespace != TIKTOK_NAMESPACE {
            return Err(ProviderError::InvalidResponse);
        }
        let video_id = match parse_video_ref(&id.value)? {
            VideoRef::Id(vid) => vid,
            VideoRef::Url(_) => return Err(ProviderError::InvalidResponse),
        };
        let mut url = Url::parse("https://www.tiktok.com/api/comment/list/")
            .map_err(|_| ProviderError::InvalidResponse)?;
        {
            let mut pairs = url.query_pairs_mut();
            pairs.append_pair("aid", "1988");
            pairs.append_pair("aweme_id", &video_id);
            pairs.append_pair("count", "20");
            pairs.append_pair("cursor", cursor.unwrap_or("0"));
        }
        let response = self
            .send_with_session(Method::GET, &url, HeaderMap::new(), None, None, cancel)
            .await?;
        let bytes = collect_bounded_body(response.body, MAX_JSON_BYTES).await?;
        let text = String::from_utf8(bytes).map_err(|_| ProviderError::InvalidResponse)?;
        parse_comments_response(&text)
    }

    /// Fetches official TikTok oEmbed metadata for a public video or profile URL.
    pub async fn oembed(
        &self,
        target_url: &str,
        cancel: CancellationToken,
    ) -> Result<OEmbedInfo, ProviderError> {
        let mut url = Url::parse("https://www.tiktok.com/oembed")
            .map_err(|_| ProviderError::InvalidResponse)?;
        url.query_pairs_mut().append_pair("url", target_url);
        let response = self
            .send_with_session(Method::GET, &url, HeaderMap::new(), None, None, cancel)
            .await?;
        let bytes = collect_bounded_body(response.body, MAX_JSON_BYTES).await?;
        let text = String::from_utf8(bytes).map_err(|_| ProviderError::InvalidResponse)?;
        parse_oembed_response(&text)
    }

    /// Fetches authorized user profile via official Display API v2 (`GET /v2/user/info/`).
    pub async fn display_user_info(
        &self,
        access_token: &str,
        cancel: CancellationToken,
    ) -> Result<DisplayUserInfo, ProviderError> {
        let url = Url::parse(
            "https://open.tiktokapis.com/v2/user/info/?fields=open_id,union_id,avatar_url,display_name,username,is_verified,follower_count,following_count,likes_count,video_count",
        )
        .map_err(|_| ProviderError::InvalidResponse)?;
        let mut headers = HeaderMap::new();
        let auth_val = header::HeaderValue::from_str(&format!("Bearer {access_token}"))
            .map_err(|_| ProviderError::Unauthorized)?;
        headers.insert(header::AUTHORIZATION, auth_val);

        self.pace_request(&cancel).await?;
        let response = self
            .transport
            .send(Method::GET, &url, headers, None, None, cancel)
            .await
            .map_err(map_network_error)?;
        let bytes = collect_bounded_body(response.body, MAX_JSON_BYTES).await?;
        let text = String::from_utf8(bytes).map_err(|_| ProviderError::InvalidResponse)?;
        parse_display_user_info_response(&text)
    }

    /// Fetches authorized user's own public videos via official Display API v2 (`POST /v2/video/list/`).
    pub async fn display_video_list(
        &self,
        access_token: &str,
        cursor: Option<i64>,
        max_count: u8,
        cancel: CancellationToken,
    ) -> Result<Page, ProviderError> {
        let url = Url::parse(
            "https://open.tiktokapis.com/v2/video/list/?fields=id,title,video_description,duration,cover_image_url,view_count",
        )
        .map_err(|_| ProviderError::InvalidResponse)?;
        let mut headers = HeaderMap::new();
        let auth_val = header::HeaderValue::from_str(&format!("Bearer {access_token}"))
            .map_err(|_| ProviderError::Unauthorized)?;
        headers.insert(header::AUTHORIZATION, auth_val);
        headers.insert(
            header::CONTENT_TYPE,
            header::HeaderValue::from_static("application/json"),
        );

        let body_json = serde_json::json!({
            "max_count": max_count.clamp(1, 20),
            "cursor": cursor.unwrap_or(0),
        });
        let body = Bytes::from(body_json.to_string());

        self.pace_request(&cancel).await?;
        let response = self
            .transport
            .send(Method::POST, &url, headers, Some(body), None, cancel)
            .await
            .map_err(map_network_error)?;
        let bytes = collect_bounded_body(response.body, MAX_JSON_BYTES).await?;
        let text = String::from_utf8(bytes).map_err(|_| ProviderError::InvalidResponse)?;
        parse_display_video_list_response(&text)
    }

    async fn register_sources(&self, candidates: &[TikTokSourceCandidate]) {
        let mut guard = self.sources.write().await;
        for c in candidates {
            guard.insert(c.clone());
        }
    }

    async fn pace_request(&self, cancel: &CancellationToken) -> Result<(), ProviderError> {
        if self.config.min_request_interval.is_zero() {
            return Ok(());
        }
        let mut guard = self.last_request.lock().await;
        if let Some(last) = *guard {
            let elapsed = last.elapsed();
            if elapsed < self.config.min_request_interval {
                let wait = self.config.min_request_interval - elapsed;
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => return Err(ProviderError::Cancelled),
                    _ = tokio::time::sleep(wait) => {}
                }
            }
        }
        *guard = Some(Instant::now());
        Ok(())
    }

    async fn send_with_session(
        &self,
        method: Method,
        url: &Url,
        mut headers: HeaderMap,
        body: Option<Bytes>,
        range: Option<ByteRange>,
        cancel: CancellationToken,
    ) -> Result<HttpResponse, ProviderError> {
        self.pace_request(&cancel).await?;

        if !headers.contains_key(header::USER_AGENT) {
            if let Ok(ua) = header::HeaderValue::from_str(&self.config.user_agent) {
                headers.insert(header::USER_AGENT, ua);
            }
        }
        if !headers.contains_key(header::REFERER) {
            headers.insert(
                header::REFERER,
                header::HeaderValue::from_static("https://www.tiktok.com/"),
            );
        }
        if let Some(cookie) = self.cookie_header().await {
            if let Ok(val) = header::HeaderValue::from_str(&cookie) {
                headers.insert(header::COOKIE, val);
            }
        }

        let response = self
            .transport
            .send(method, url, headers, body, range, cancel)
            .await
            .map_err(map_network_error)?;

        self.store_set_cookies(&response.headers).await;
        Ok(response)
    }

    async fn store_set_cookies(&self, headers: &HeaderMap) {
        let mut guard = None;
        for val in headers.get_all(header::SET_COOKIE) {
            let Ok(raw) = val.to_str() else {
                continue;
            };
            let name_val = raw.split(';').next().unwrap_or(raw);
            let Some((name, value)) = name_val.split_once('=') else {
                continue;
            };
            let name = name.trim();
            if name.is_empty() {
                continue;
            }
            if guard.is_none() {
                guard = Some(self.cookies.write().await);
            }
            if let Some(map) = guard.as_mut() {
                map.insert(name.to_string(), Zeroizing::new(value.trim().to_string()));
            }
        }
    }
}

#[async_trait]
impl<T: Transport> ContentProvider for TikTokProvider<T> {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            // Personalized For You is NOT available via permitted unsigned/official endpoints.
            recommended: false,
            following: false,
            search: false,
            actions: false,
            playback_events: false,
            ranges: true,
        }
    }

    async fn page(
        &self,
        _kind: &FeedKind,
        _cursor: Option<&str>,
        cancel: CancellationToken,
    ) -> Result<Page, ProviderError> {
        if cancel.is_cancelled() {
            return Err(ProviderError::Cancelled);
        }
        // Never masquerade public Explore or user posts as personalized For You / Following / Search.
        Err(ProviderError::Unsupported)
    }

    async fn sources(
        &self,
        id: &ContentId,
        cancel: CancellationToken,
    ) -> Result<Vec<Source>, ProviderError> {
        if id.namespace != TIKTOK_NAMESPACE {
            return Err(ProviderError::InvalidResponse);
        }
        let video = self.resolve_video(&id.value, cancel).await?;
        if video.sources.is_empty() {
            return Err(ProviderError::NotFound);
        }
        Ok(video
            .sources
            .iter()
            .map(TikTokSourceCandidate::to_source)
            .collect())
    }

    async fn read(
        &self,
        source: &Source,
        range: Option<ByteRange>,
        cancel: CancellationToken,
    ) -> Result<MediaStream, ProviderError> {
        if cancel.is_cancelled() {
            return Err(ProviderError::Cancelled);
        }
        let candidate = {
            let guard = self.sources.read().await;
            guard.get(&source.id).cloned()
        }
        .ok_or(ProviderError::SourceExpired)?;

        if candidate
            .expires_at
            .is_some_and(|exp| exp <= SystemTime::now())
        {
            return Err(ProviderError::SourceExpired);
        }

        let mut last_err = ProviderError::SourceExpired;
        for raw_url in &candidate.urls {
            if cancel.is_cancelled() {
                return Err(ProviderError::Cancelled);
            }
            let Ok(url) = Url::parse(raw_url) else {
                continue;
            };
            match self
                .send_with_session(
                    Method::GET,
                    &url,
                    browser_headers(),
                    None,
                    range,
                    cancel.clone(),
                )
                .await
            {
                Ok(resp) => {
                    let mapped = resp.body.map(|item| item.map_err(map_network_error));
                    return Ok(Box::pin(mapped));
                }
                Err(ProviderError::Cancelled) => return Err(ProviderError::Cancelled),
                Err(ProviderError::Unauthorized) => {
                    // Signed CDN URL or chain cookie may have expired; allow MediaDelivery to refresh once.
                    last_err = ProviderError::SourceExpired;
                }
                Err(e) => {
                    last_err = e;
                }
            }
        }
        Err(last_err)
    }

    async fn action(
        &self,
        _id: &ContentId,
        _action: Action,
        cancel: CancellationToken,
    ) -> Result<(), ProviderError> {
        if cancel.is_cancelled() {
            return Err(ProviderError::Cancelled);
        }
        Err(ProviderError::Unsupported)
    }

    async fn playback(
        &self,
        _id: &ContentId,
        _event: PlaybackEvent,
        cancel: CancellationToken,
    ) -> Result<(), ProviderError> {
        if cancel.is_cancelled() {
            return Err(ProviderError::Cancelled);
        }
        Err(ProviderError::Unsupported)
    }

    async fn logout(&self) -> Result<(), ProviderError> {
        self.cookies.write().await.clear();
        self.sources.write().await.clear();
        Ok(())
    }
}
