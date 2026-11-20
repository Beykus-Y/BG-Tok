//! Pull-based HTTP delivery. No request, URL, or response is logged or cached.
use std::{pin::Pin, time::Duration};

use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use reqwest::{header, Client, StatusCode, Url};
use thiserror::Error;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Error)]
pub enum NetworkError {
    #[error("request cancelled")]
    Cancelled,
    #[error("network timeout")]
    Timeout,
    #[error("network transport error")]
    Transport,
    #[error("invalid HTTP configuration")]
    Configuration,
    #[error("HTTP status {0}")]
    Http(u16),
    #[error("session expired or unauthorized")]
    Unauthorized,
    #[error("rate limited")]
    RateLimited,
    #[error("server did not honor the requested range")]
    InvalidRange,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ByteRange {
    pub start: u64,
    /// Inclusive end; None means through the end of the resource.
    pub end: Option<u64>,
}

impl ByteRange {
    pub fn new(start: u64, end: Option<u64>) -> Result<Self, NetworkError> {
        if end.is_some_and(|end| end < start) {
            return Err(NetworkError::InvalidRange);
        }
        Ok(Self { start, end })
    }
}

#[derive(Clone)]
pub struct HttpConfig {
    pub connect_timeout: Duration,
    /// Per-attempt deadline, including consumption of the response body.
    pub request_timeout: Duration,
    pub max_retries: u32,
    pub backoff: Duration,
    pub max_backoff: Duration,
    /// HTTP(S) or SOCKS proxy configured by the host. Never logged.
    pub proxy: Option<String>,
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(10),
            request_timeout: Duration::from_secs(60),
            max_retries: 2,
            backoff: Duration::from_millis(200),
            max_backoff: Duration::from_secs(5),
            proxy: None,
        }
    }
}

// The proxy may contain credentials.
// Do not expose it through debug formatting.
// HttpConfig uses a manual Debug implementation below.
impl std::fmt::Debug for HttpConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpConfig")
            .field("connect_timeout", &self.connect_timeout)
            .field("request_timeout", &self.request_timeout)
            .field("max_retries", &self.max_retries)
            .field("backoff", &self.backoff)
            .field("max_backoff", &self.max_backoff)
            .field("proxy", &self.proxy.as_ref().map(|_| "[redacted]"))
            .finish()
    }
}

pub type ByteStream = Pin<Box<dyn Stream<Item = Result<Bytes, NetworkError>> + Send>>;

#[derive(Clone)]
pub struct HttpTransport {
    client: Client,
    config: HttpConfig,
}

impl HttpTransport {
    pub fn new(config: HttpConfig) -> Result<Self, NetworkError> {
        if config.connect_timeout.is_zero()
            || config.request_timeout.is_zero()
            || config.max_retries > 10
        {
            return Err(NetworkError::Configuration);
        }
        // Ignore process proxy settings; routing is an explicit host decision.
        let mut builder = Client::builder()
            .no_proxy()
            .connect_timeout(config.connect_timeout)
            .timeout(config.request_timeout)
            .redirect(reqwest::redirect::Policy::limited(5));
        if let Some(proxy) = &config.proxy {
            builder =
                builder.proxy(reqwest::Proxy::all(proxy).map_err(|_| NetworkError::Configuration)?);
        }
        let client = builder.build().map_err(|_| NetworkError::Configuration)?;
        Ok(Self { client, config })
    }

    /// GET is retried only before a response body is returned to the caller.
    /// A stream is pull-driven; dropping it releases the connection. Callers
    /// must not buffer it unboundedly or interpret prefetch as playback.
    pub async fn get(
        &self,
        url: &Url,
        range: Option<ByteRange>,
        cancel: CancellationToken,
    ) -> Result<ByteStream, NetworkError> {
        if !matches!(url.scheme(), "http" | "https") {
            return Err(NetworkError::Configuration);
        }
        if let Some(r) = range {
            ByteRange::new(r.start, r.end)?;
        }
        let mut attempt = 0;
        loop {
            let mut request = self
                .client
                .get(url.clone())
                .header(header::ACCEPT_ENCODING, "identity");
            if let Some(r) = range {
                let end = r.end.map(|v| v.to_string()).unwrap_or_default();
                request = request.header(header::RANGE, format!("bytes={}-{end}", r.start));
            }
            let result = tokio::select! { biased; _ = cancel.cancelled() => return Err(NetworkError::Cancelled), result = request.send() => result };
            let retry_after;
            match result {
                Ok(response) => {
                    let status = response.status();
                    if status.is_success() {
                        if let Some(r) = range {
                            if status != StatusCode::PARTIAL_CONTENT
                                || !valid_content_range(
                                    response
                                        .headers()
                                        .get(header::CONTENT_RANGE)
                                        .and_then(|v| v.to_str().ok()),
                                    r,
                                )
                            {
                                return Err(NetworkError::InvalidRange);
                            }
                        }
                        let stream = response.bytes_stream();
                        let cancelled = cancel.clone();
                        let output = futures_util::stream::unfold(
                            (stream, cancelled, false),
                            |(mut stream, cancel, done)| async move {
                                if done {
                                    return None;
                                }
                                tokio::select! { biased;
                                    _ = cancel.cancelled() => Some((Err(NetworkError::Cancelled), (stream, cancel, true))),
                                    item = stream.next() => item.map(|item| {
                                        let item = item.map_err(|e| if e.is_timeout() { NetworkError::Timeout } else { NetworkError::Transport });
                                        let done = item.is_err();
                                        (item, (stream, cancel, done))
                                    })
                                }
                            },
                        );
                        return Ok(Box::pin(output));
                    }
                    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
                        return Err(NetworkError::Unauthorized);
                    }
                    let retryable =
                        status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error();
                    if !retryable || attempt >= self.config.max_retries {
                        return Err(if status == StatusCode::TOO_MANY_REQUESTS {
                            NetworkError::RateLimited
                        } else {
                            NetworkError::Http(status.as_u16())
                        });
                    }
                    retry_after = response
                        .headers()
                        .get(header::RETRY_AFTER)
                        .and_then(|v| v.to_str().ok())
                        .and_then(|v| v.parse::<u64>().ok())
                        .map(Duration::from_secs);
                }
                Err(e) => {
                    if attempt >= self.config.max_retries {
                        return Err(if e.is_timeout() {
                            NetworkError::Timeout
                        } else {
                            NetworkError::Transport
                        });
                    }
                    retry_after = None;
                }
            }
            let exponential = self
                .config
                .backoff
                .saturating_mul(2_u32.saturating_pow(attempt));
            let delay = retry_after
                .unwrap_or(exponential)
                .min(self.config.max_backoff);
            attempt += 1;
            tokio::select! { biased; _ = cancel.cancelled() => return Err(NetworkError::Cancelled), _ = tokio::time::sleep(delay) => {} }
        }
    }
}

fn valid_content_range(value: Option<&str>, requested: ByteRange) -> bool {
    let Some((range, total)) = value
        .and_then(|v| v.strip_prefix("bytes "))
        .and_then(|v| v.split_once('/'))
    else {
        return false;
    };
    let Some((start, end)) = range.split_once('-') else {
        return false;
    };
    let (Ok(start), Ok(end), Ok(total)) = (
        start.parse::<u64>(),
        end.parse::<u64>(),
        total.parse::<u64>(),
    ) else {
        return false;
    };
    start == requested.start
        && end >= start
        && end < total
        && requested
            .end
            .is_none_or(|wanted| end == wanted.min(total - 1))
        && (requested.end.is_some() || end == total - 1)
}
