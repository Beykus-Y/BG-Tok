//! Experimental Android Companion adapter for ordered For You video IDs.
//!
//! Gated behind `experimental-companion` and disabled in default production builds.
//! Receives only ordered public video IDs observed on an authorized test device
//! (never cookies, tokens, device IDs, or proprietary TikTok models) and resolves
//! them through [`TikTokProvider`].
use crate::{models::TIKTOK_NAMESPACE, provider::TikTokProvider};
use async_trait::async_trait;
use bg_tok::{
    account::AccountId,
    network::{ByteRange, HttpTransport, Transport},
    provider::{
        Action, Capabilities, ContentId, ContentProvider, FeedKind, MediaStream, Metadata, Page,
        PlaybackEvent, ProviderError, Source,
    },
};
use std::{collections::HashSet, sync::Arc};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

/// Single observed For You item (ID + order metadata only, no secrets).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompanionItem {
    pub video_id: String,
    pub position: usize,
    pub observed_at_ms: u64,
}

/// Ordered batch of For You video IDs observed on a test Android device.
#[derive(Clone)]
pub struct CompanionBatch {
    pub account_id: AccountId,
    pub batch_seq: u64,
    pub has_more: bool,
    pub items: Vec<CompanionItem>,
}

struct CompanionState {
    active_account: Option<AccountId>,
    account_cancel: CancellationToken,
    ordered_ids: Vec<String>,
    seen: HashSet<String>,
    has_more: bool,
    last_batch_seq: Option<u64>,
}

impl CompanionState {
    fn new() -> Self {
        Self {
            active_account: None,
            account_cancel: CancellationToken::new(),
            ordered_ids: Vec::new(),
            seen: HashSet::new(),
            has_more: false,
            last_batch_seq: None,
        }
    }

    fn reset(&mut self, next_account: Option<AccountId>) {
        self.account_cancel.cancel();
        self.account_cancel = CancellationToken::new();
        self.active_account = next_account;
        self.ordered_ids.clear();
        self.seen.clear();
        self.has_more = false;
        self.last_batch_seq = None;
    }
}

/// Experimental [`ContentProvider`] that serves ordered For You IDs from an
/// Android Companion observation stream and resolves media via [`TikTokProvider`].
pub struct CompanionFeedAdapter<T: Transport = HttpTransport> {
    inner: Arc<TikTokProvider<T>>,
    state: Mutex<CompanionState>,
    page_size: usize,
}

impl<T: Transport> CompanionFeedAdapter<T> {
    /// Creates a new experimental companion adapter wrapping [`TikTokProvider`].
    pub fn new(inner: Arc<TikTokProvider<T>>, page_size: usize) -> Self {
        Self {
            inner,
            state: Mutex::new(CompanionState::new()),
            page_size: page_size.max(1),
        }
    }

    /// Switches the active account, cancelling stale work and clearing buffered IDs.
    pub async fn switch_account(&self, account_id: AccountId) {
        let mut guard = self.state.lock().await;
        if guard.active_account.as_ref() != Some(&account_id) {
            guard.reset(Some(account_id));
        }
    }

    /// Appends an observed For You batch, preserving server order and deduplicating IDs.
    /// Returns the number of newly accepted unique video IDs.
    pub async fn push_batch(&self, batch: CompanionBatch) -> Result<usize, ProviderError> {
        let mut guard = self.state.lock().await;
        if guard.active_account.as_ref() != Some(&batch.account_id) {
            guard.reset(Some(batch.account_id));
        }
        if let Some(prev) = guard.last_batch_seq {
            if batch.batch_seq <= prev {
                return Err(ProviderError::InvalidResponse);
            }
        }
        let mut items = batch.items;
        items.sort_by_key(|item| item.position);

        let mut added = 0usize;
        for item in items {
            let trimmed = item.video_id.trim();
            if trimmed.is_empty() || !trimmed.bytes().all(|b| b.is_ascii_digit()) {
                continue;
            }
            if guard.seen.insert(trimmed.to_string()) {
                guard.ordered_ids.push(trimmed.to_string());
                added += 1;
            }
        }
        guard.has_more = batch.has_more;
        guard.last_batch_seq = Some(batch.batch_seq);
        Ok(added)
    }
}

#[async_trait]
impl<T: Transport> ContentProvider for CompanionFeedAdapter<T> {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            recommended: true,
            following: false,
            search: false,
            actions: false,
            playback_events: false,
            ranges: true,
        }
    }

    async fn page(
        &self,
        kind: &FeedKind,
        cursor: Option<&str>,
        cancel: CancellationToken,
    ) -> Result<Page, ProviderError> {
        if *kind != FeedKind::Recommended {
            return Err(ProviderError::Unsupported);
        }
        if cancel.is_cancelled() {
            return Err(ProviderError::Cancelled);
        }

        let (slice, next_cursor, account_cancel) = {
            let guard = self.state.lock().await;
            if guard.active_account.is_none() {
                return Err(ProviderError::Unauthorized);
            }
            let offset = match cursor {
                None => 0usize,
                Some(raw) => raw
                    .parse::<usize>()
                    .map_err(|_| ProviderError::InvalidResponse)?,
            };
            if offset > guard.ordered_ids.len() {
                return Err(ProviderError::InvalidResponse);
            }
            let end = (offset + self.page_size).min(guard.ordered_ids.len());
            let slice = guard.ordered_ids[offset..end].to_vec();
            let next_cursor =
                if end < guard.ordered_ids.len() || (guard.has_more && !slice.is_empty()) {
                    Some(end.to_string())
                } else {
                    None
                };
            (slice, next_cursor, guard.account_cancel.child_token())
        };

        let mut items: Vec<Metadata> = Vec::with_capacity(slice.len());
        for video_id in slice {
            let op_cancel = cancel.child_token();
            let video = tokio::select! {
                biased;
                _ = account_cancel.cancelled() => return Err(ProviderError::SessionClosed),
                _ = cancel.cancelled() => return Err(ProviderError::Cancelled),
                res = self.inner.resolve_video(&video_id, op_cancel) => res?,
            };
            items.push(video.to_metadata());
        }

        Ok(Page { items, next_cursor })
    }

    async fn sources(
        &self,
        id: &ContentId,
        cancel: CancellationToken,
    ) -> Result<Vec<Source>, ProviderError> {
        self.inner.sources(id, cancel).await
    }

    async fn read(
        &self,
        source: &Source,
        range: Option<ByteRange>,
        cancel: CancellationToken,
    ) -> Result<MediaStream, ProviderError> {
        self.inner.read(source, range, cancel).await
    }

    async fn action(
        &self,
        id: &ContentId,
        action: Action,
        cancel: CancellationToken,
    ) -> Result<(), ProviderError> {
        self.inner.action(id, action, cancel).await
    }

    async fn playback(
        &self,
        id: &ContentId,
        event: PlaybackEvent,
        cancel: CancellationToken,
    ) -> Result<(), ProviderError> {
        self.inner.playback(id, event, cancel).await
    }

    async fn logout(&self) -> Result<(), ProviderError> {
        self.state.lock().await.reset(None);
        self.inner.logout().await
    }
}

/// Builds a [`ContentId`] in the `tiktok` namespace from an observed video ID.
pub fn companion_content_id(video_id: &str) -> ContentId {
    ContentId {
        namespace: TIKTOK_NAMESPACE.to_string(),
        value: video_id.to_string(),
    }
}
