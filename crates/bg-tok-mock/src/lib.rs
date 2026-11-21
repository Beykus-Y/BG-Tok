//! Synthetic provider for tests and the desktop CLI. Never contacts TikTok.
#![forbid(unsafe_code)]
use async_trait::async_trait;
use bg_tok::{
    network::ByteRange,
    provider::{
        Action, CachePolicy, Capabilities, ContentId, ContentProvider, FeedKind, MediaStream,
        Metadata, Page, PlaybackEvent, ProviderError, Source,
    },
};
use bytes::Bytes;
use std::{
    sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    time::{Duration, SystemTime},
};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

pub fn id(value: &str) -> ContentId {
    ContentId {
        namespace: "mock".into(),
        value: value.into(),
    }
}
fn metadata(value: &str) -> Metadata {
    Metadata {
        id: id(value),
        author: "Synthetic author".into(),
        description: format!("Synthetic video {value}"),
        duration: Some(Duration::from_secs(10)),
        thumbnail: None,
        views: None,
    }
}

#[derive(Default)]
pub struct MockProvider {
    media: Option<Bytes>,
    closed: AtomicBool,
    calls: AtomicUsize,
    delay_ms: AtomicU64,
    read_delay_ms: AtomicU64,
    cycle_cursor: AtomicBool,
    next_page_error: Mutex<Option<ProviderError>>,
    events: Mutex<Vec<(ContentId, PlaybackEvent)>>,
}
impl MockProvider {
    pub fn with_media(media: Bytes) -> Self {
        Self {
            media: Some(media),
            ..Self::default()
        }
    }

    pub fn page_calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
    /// Inject one synthetic page failure, useful for testing recovery.
    pub async fn fail_next_page(&self, error: ProviderError) {
        *self.next_page_error.lock().await = Some(error);
    }
    pub fn set_cursor_cycle(&self, enabled: bool) {
        self.cycle_cursor.store(enabled, Ordering::SeqCst);
    }
    pub fn set_page_delay(&self, delay: Duration) {
        self.delay_ms.store(
            delay.as_millis().min(u64::MAX as u128) as u64,
            Ordering::SeqCst,
        );
    }
    pub fn set_read_delay(&self, delay: Duration) {
        self.read_delay_ms.store(
            delay.as_millis().min(u64::MAX as u128) as u64,
            Ordering::SeqCst,
        );
    }
    pub async fn events(&self) -> Vec<(ContentId, PlaybackEvent)> {
        self.events.lock().await.clone()
    }
    fn check(&self, cancel: &CancellationToken) -> Result<(), ProviderError> {
        if cancel.is_cancelled() {
            Err(ProviderError::Cancelled)
        } else if self.closed.load(Ordering::SeqCst) {
            Err(ProviderError::SessionClosed)
        } else {
            Ok(())
        }
    }
}
#[async_trait]
impl ContentProvider for MockProvider {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            recommended: true,
            following: true,
            search: true,
            actions: true,
            playback_events: true,
            ranges: true,
        }
    }
    async fn page(
        &self,
        kind: &FeedKind,
        cursor: Option<&str>,
        cancel: CancellationToken,
    ) -> Result<Page, ProviderError> {
        self.check(&cancel)?;
        self.calls.fetch_add(1, Ordering::SeqCst);
        let delay = Duration::from_millis(self.delay_ms.load(Ordering::SeqCst));
        tokio::select! { biased; _ = cancel.cancelled() => return Err(ProviderError::Cancelled), _ = tokio::time::sleep(delay) => {} }
        self.check(&cancel)?;
        if let Some(error) = self.next_page_error.lock().await.take() {
            return Err(error);
        }
        if self.cycle_cursor.load(Ordering::SeqCst) {
            return Ok(Page {
                items: vec![],
                next_cursor: Some("cycle".into()),
            });
        }
        if let FeedKind::Search(query) = kind {
            return Ok(Page {
                items: ["one", "two", "three"]
                    .into_iter()
                    .filter(|s| s.contains(query))
                    .map(metadata)
                    .collect(),
                next_cursor: None,
            });
        }
        match cursor {
            None => Ok(Page {
                items: vec![metadata("one"), metadata("two")],
                next_cursor: Some("page-2".into()),
            }),
            Some("page-2") => Ok(Page {
                items: vec![metadata("two"), metadata("three")],
                next_cursor: None,
            }),
            _ => Err(ProviderError::InvalidResponse),
        }
    }
    async fn sources(
        &self,
        id: &ContentId,
        cancel: CancellationToken,
    ) -> Result<Vec<Source>, ProviderError> {
        self.check(&cancel)?;
        if id.namespace != "mock" || !["one", "two", "three"].contains(&id.value.as_str()) {
            return Err(ProviderError::NotFound);
        }
        Ok(vec![Source {
            id: id.value.clone(),
            mime_type: "application/octet-stream".into(),
            width: None,
            height: None,
            bitrate_bps: None,
            expires_at: Some(SystemTime::now() + Duration::from_secs(60)),
            cache: CachePolicy::NoStore,
        }])
    }
    async fn read(
        &self,
        source: &Source,
        range: Option<ByteRange>,
        cancel: CancellationToken,
    ) -> Result<MediaStream, ProviderError> {
        self.check(&cancel)?;
        if source.expires_at.is_some_and(|e| e <= SystemTime::now()) {
            return Err(ProviderError::SourceExpired);
        }
        if !["one", "two", "three"].contains(&source.id.as_str()) {
            return Err(ProviderError::NotFound);
        }
        let delay = Duration::from_millis(self.read_delay_ms.load(Ordering::SeqCst));
        if !delay.is_zero() {
            tokio::select! { biased; _ = cancel.cancelled() => return Err(ProviderError::Cancelled), _ = tokio::time::sleep(delay) => {} }
        }
        let data = self
            .media
            .clone()
            .unwrap_or_else(|| Bytes::from_static(b"synthetic-media"));
        let (start, end) = match range {
            Some(r) if r.start < data.len() as u64 && r.end.is_none_or(|e| e >= r.start) => (
                r.start as usize,
                r.end
                    .unwrap_or(data.len() as u64 - 1)
                    .min(data.len() as u64 - 1) as usize
                    + 1,
            ),
            Some(_) => return Err(ProviderError::InvalidResponse),
            None => (0, data.len()),
        };
        Ok(Box::pin(futures_util::stream::once(async move {
            Ok(data.slice(start..end))
        })))
    }
    async fn action(
        &self,
        _id: &ContentId,
        _action: Action,
        cancel: CancellationToken,
    ) -> Result<(), ProviderError> {
        self.check(&cancel)
    }
    async fn playback(
        &self,
        id: &ContentId,
        event: PlaybackEvent,
        cancel: CancellationToken,
    ) -> Result<(), ProviderError> {
        self.check(&cancel)?;
        self.events.lock().await.push((id.clone(), event));
        Ok(())
    }
    async fn logout(&self) -> Result<(), ProviderError> {
        self.closed.store(true, Ordering::SeqCst);
        Ok(())
    }
}
