//! Ordered metadata pagination. Presentation and prefetch queues belong to the host.
use crate::provider::{ContentId, ContentProvider, FeedKind, Metadata, ProviderError};
use std::{collections::HashSet, sync::Arc};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

struct State {
    items: Vec<Metadata>,
    seen: HashSet<ContentId>,
    cursors: HashSet<String>,
    cursor: Option<String>,
    exhausted: bool,
}

pub struct Feed<P: ContentProvider + ?Sized> {
    provider: Arc<P>,
    kind: FeedKind,
    state: Mutex<State>,
    loading: Mutex<()>,
    closed: CancellationToken,
    capacity: usize,
}
impl<P: ContentProvider + ?Sized> Feed<P> {
    pub fn new(provider: Arc<P>, kind: FeedKind, capacity: usize) -> Self {
        Self::with_session(provider, kind, capacity, CancellationToken::new())
    }

    pub fn with_session(
        provider: Arc<P>,
        kind: FeedKind,
        capacity: usize,
        session: CancellationToken,
    ) -> Self {
        Self {
            provider,
            kind,
            state: Mutex::new(State {
                items: vec![],
                seen: HashSet::new(),
                cursors: HashSet::new(),
                cursor: None,
                exhausted: false,
            }),
            loading: Mutex::new(()),
            closed: session.child_token(),
            capacity,
        }
    }
    /// Cancels pending requests and permanently closes this feed.
    pub fn close(&self) {
        self.closed.cancel();
    }
    pub async fn snapshot(&self) -> Vec<Metadata> {
        self.state.lock().await.items.clone()
    }
    /// Index lookup is metadata only; no playback or byte transfer is started.
    pub async fn get(&self, index: usize) -> Option<Metadata> {
        self.state.lock().await.items.get(index).cloned()
    }
    pub async fn load_next(&self, cancel: CancellationToken) -> Result<usize, ProviderError> {
        let _guard = tokio::select! { biased; _ = self.closed.cancelled() => return Err(ProviderError::SessionClosed), _ = cancel.cancelled() => return Err(ProviderError::Cancelled), guard = self.loading.lock() => guard };
        let cursor = {
            let state = self.state.lock().await;
            if state.exhausted {
                return Ok(0);
            }
            state.cursor.clone()
        };
        let operation = cancel.child_token();
        let page = tokio::select! { biased; _ = self.closed.cancelled() => { operation.cancel(); return Err(ProviderError::SessionClosed) }, _ = cancel.cancelled() => { operation.cancel(); return Err(ProviderError::Cancelled) }, result = self.provider.page(&self.kind, cursor.as_deref(), operation.clone()) => result? };
        let mut state = self.state.lock().await;
        if self.closed.is_cancelled() {
            return Err(ProviderError::SessionClosed);
        }
        if cancel.is_cancelled() {
            return Err(ProviderError::Cancelled);
        }
        if let Some(next) = &page.next_cursor {
            if state.cursors.contains(next) || cursor.as_ref() == Some(next) {
                return Err(ProviderError::InvalidResponse);
            }
        }
        let mut added = HashSet::new();
        for item in &page.items {
            if !state.seen.contains(&item.id) {
                added.insert(item.id.clone());
            }
        }
        if state.items.len().saturating_add(added.len()) > self.capacity {
            return Err(ProviderError::Capacity);
        }
        let mut count = 0;
        for item in page.items {
            if state.seen.insert(item.id.clone()) {
                state.items.push(item);
                count += 1;
            }
        }
        if let Some(next) = &page.next_cursor {
            state.cursors.insert(next.clone());
        }
        state.exhausted = page.next_cursor.is_none();
        state.cursor = page.next_cursor;
        Ok(count)
    }
    pub async fn exhausted(&self) -> bool {
        self.state.lock().await.exhausted
    }
}
