//! Service-neutral Rust interface. This is not a stable C ABI.
use crate::network::ByteRange;
use async_trait::async_trait;
use bytes::Bytes;
use futures_util::Stream;
use std::{
    pin::Pin,
    time::{Duration, SystemTime},
};
use thiserror::Error;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ContentId {
    pub namespace: String,
    pub value: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Metadata {
    pub id: ContentId,
    pub author: String,
    pub description: String,
    pub duration: Option<Duration>,
    pub thumbnail: Option<String>,
    pub views: Option<u64>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Page {
    pub items: Vec<Metadata>,
    pub next_cursor: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FeedKind {
    Recommended,
    Following,
    Search(String),
}
/// Source is an opaque handle, not a URL or credential. The provider owns its
/// mapping and must re-resolve expired handles when requested by the host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    pub id: String,
    pub mime_type: String,
    pub expires_at: Option<SystemTime>,
    pub cache: CachePolicy,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CachePolicy {
    NoStore,
    HostManaged,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Like(bool),
    Follow(bool),
    Comment(String),
    Share,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlaybackEvent {
    Started,
    Visibility(bool),
    Paused,
    Resumed,
    Progress(Duration),
    Completed,
    Skipped,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Capabilities {
    pub recommended: bool,
    pub following: bool,
    pub search: bool,
    pub actions: bool,
    pub playback_events: bool,
    pub ranges: bool,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ProviderError {
    #[error("operation cancelled")]
    Cancelled,
    #[error("session closed")]
    SessionClosed,
    #[error("authentication required")]
    Unauthorized,
    #[error("rate limited")]
    RateLimited,
    #[error("source expired")]
    SourceExpired,
    #[error("operation unsupported")]
    Unsupported,
    #[error("invalid provider response")]
    InvalidResponse,
    #[error("resource not found")]
    NotFound,
    #[error("transport error")]
    Transport,
    #[error("storage error")]
    Storage,
    #[error("capacity exceeded")]
    Capacity,
}
pub type MediaStream = Pin<Box<dyn Stream<Item = Result<Bytes, ProviderError>> + Send>>;

/// Implementations must respect cancellation, avoid logging secrets and keep
/// account credentials internal. Fetches are initiated solely by the host.
#[async_trait]
pub trait ContentProvider: Send + Sync {
    fn capabilities(&self) -> Capabilities;
    async fn page(
        &self,
        kind: &FeedKind,
        cursor: Option<&str>,
        cancel: CancellationToken,
    ) -> Result<Page, ProviderError>;
    async fn sources(
        &self,
        id: &ContentId,
        cancel: CancellationToken,
    ) -> Result<Vec<Source>, ProviderError>;
    async fn read(
        &self,
        source: &Source,
        range: Option<ByteRange>,
        cancel: CancellationToken,
    ) -> Result<MediaStream, ProviderError>;
    async fn action(
        &self,
        id: &ContentId,
        action: Action,
        cancel: CancellationToken,
    ) -> Result<(), ProviderError>;
    /// Called only with actual host playback events, never on preloading.
    async fn playback(
        &self,
        id: &ContentId,
        event: PlaybackEvent,
        cancel: CancellationToken,
    ) -> Result<(), ProviderError>;
    async fn logout(&self) -> Result<(), ProviderError>;
}
