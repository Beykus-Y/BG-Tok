//! Linux-only local-file provider for integration tests and demos, not production.
//! Inputs must remain immutable for the life of the provider.
#![cfg(target_os = "linux")]
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
    fs::File,
    os::unix::fs::{FileExt, MetadataExt},
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

const MAX_READ: usize = 1024 * 1024;
struct Item {
    file: Arc<File>,
    length: u64,
    revision: String,
}
fn revision(metadata: &std::fs::Metadata) -> String {
    format!(
        "{}:{}:{}:{}:{}:{}:{}",
        metadata.dev(),
        metadata.ino(),
        metadata.len(),
        metadata.mtime(),
        metadata.mtime_nsec(),
        metadata.ctime(),
        metadata.ctime_nsec()
    )
}
fn check_file(item: &Item) -> Result<(), ProviderError> {
    let meta = item.file.metadata().map_err(|_| ProviderError::Storage)?;
    if revision(&meta) != item.revision {
        return Err(ProviderError::SourceExpired);
    }
    Ok(())
}

pub struct FileProvider {
    items: Arc<Vec<Item>>,
    slots: Arc<Semaphore>,
    closed: AtomicBool,
    calls: Arc<AtomicUsize>,
    bytes_per_second: u64,
}
impl FileProvider {
    pub fn open<P: AsRef<Path>>(paths: &[P], max_concurrent: usize) -> Result<Self, ProviderError> {
        if paths.is_empty() || max_concurrent == 0 {
            return Err(ProviderError::Capacity);
        }
        let mut items = Vec::with_capacity(paths.len());
        for path in paths {
            let file = File::open(path).map_err(|_| ProviderError::NotFound)?;
            let meta = file.metadata().map_err(|_| ProviderError::Storage)?;
            if !meta.is_file() || meta.len() == 0 || meta.len() > i64::MAX as u64 {
                return Err(ProviderError::InvalidResponse);
            }
            items.push(Item {
                file: Arc::new(file),
                length: meta.len(),
                revision: revision(&meta),
            });
        }
        Ok(Self {
            items: Arc::new(items),
            slots: Arc::new(Semaphore::new(max_concurrent)),
            closed: AtomicBool::new(false),
            calls: Arc::new(AtomicUsize::new(0)),
            bytes_per_second: 0,
        })
    }
    /// Simulate per-request transfer time in tests; zero disables throttling.
    pub fn with_rate_limit(mut self, bytes_per_second: u64) -> Self {
        self.bytes_per_second = bytes_per_second;
        self
    }
    pub fn read_calls(&self) -> usize {
        self.calls.load(Ordering::Relaxed)
    }
    fn check(&self, token: &CancellationToken) -> Result<(), ProviderError> {
        if token.is_cancelled() {
            Err(ProviderError::Cancelled)
        } else if self.closed.load(Ordering::Acquire) {
            Err(ProviderError::SessionClosed)
        } else {
            Ok(())
        }
    }
    fn index(&self, id: &ContentId) -> Result<usize, ProviderError> {
        if id.namespace != "file-fixture" {
            return Err(ProviderError::NotFound);
        }
        id.value
            .parse::<usize>()
            .ok()
            .filter(|i| *i < self.items.len())
            .ok_or(ProviderError::NotFound)
    }
}
#[async_trait]
impl ContentProvider for FileProvider {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            recommended: true,
            ranges: true,
            ..Default::default()
        }
    }
    async fn page(
        &self,
        kind: &FeedKind,
        cursor: Option<&str>,
        cancel: CancellationToken,
    ) -> Result<Page, ProviderError> {
        self.check(&cancel)?;
        if !matches!(kind, FeedKind::Recommended) || cursor.is_some() {
            return Err(ProviderError::Unsupported);
        }
        Ok(Page {
            items: (0..self.items.len())
                .map(|i| Metadata {
                    id: ContentId {
                        namespace: "file-fixture".into(),
                        value: i.to_string(),
                    },
                    author: "Local fixture".into(),
                    description: format!("Fixture {i}"),
                    duration: None,
                    thumbnail: None,
                    views: None,
                })
                .collect(),
            next_cursor: None,
        })
    }
    async fn sources(
        &self,
        id: &ContentId,
        cancel: CancellationToken,
    ) -> Result<Vec<Source>, ProviderError> {
        self.check(&cancel)?;
        let i = self.index(id)?;
        let item = &self.items[i];
        check_file(item)?;
        Ok(vec![Source {
            id: i.to_string(),
            mime_type: "video/mp4".into(),
            length: Some(item.length),
            revision: Some(item.revision.clone()),
            width: None,
            height: None,
            bitrate_bps: None,
            expires_at: None,
            cache: CachePolicy::HostManaged,
        }])
    }
    async fn read(
        &self,
        source: &Source,
        range: Option<ByteRange>,
        cancel: CancellationToken,
    ) -> Result<MediaStream, ProviderError> {
        self.check(&cancel)?;
        let i = self.index(&ContentId {
            namespace: "file-fixture".into(),
            value: source.id.clone(),
        })?;
        let item = &self.items[i];
        if source.length != Some(item.length)
            || source.revision.as_deref() != Some(item.revision.as_str())
        {
            return Err(ProviderError::SourceExpired);
        }
        let range = range.ok_or(ProviderError::Unsupported)?;
        let end = range.end.ok_or(ProviderError::Unsupported)?;
        if end < range.start || end >= item.length || end - range.start >= MAX_READ as u64 {
            return Err(ProviderError::InvalidResponse);
        }
        self.calls.fetch_add(1, Ordering::Relaxed);
        let permit = tokio::select! { biased; _ = cancel.cancelled() => return Err(ProviderError::Cancelled), p = self.slots.clone().acquire_owned() => p.map_err(|_| ProviderError::SessionClosed)? };
        if self.bytes_per_second != 0 {
            let seconds = (end - range.start + 1) as f64 / self.bytes_per_second as f64;
            tokio::select! { biased; _ = cancel.cancelled() => return Err(ProviderError::Cancelled), _ = tokio::time::sleep(std::time::Duration::from_secs_f64(seconds)) => {} }
        }
        let items = self.items.clone();
        let token = cancel.clone();
        let task = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            if token.is_cancelled() {
                return Err(ProviderError::Cancelled);
            }
            let item = &items[i];
            check_file(item)?;
            let mut data = vec![0; (end - range.start + 1) as usize];
            let mut read = 0;
            while read < data.len() {
                if token.is_cancelled() {
                    return Err(ProviderError::Cancelled);
                }
                let n = item
                    .file
                    .read_at(&mut data[read..], range.start + read as u64)
                    .map_err(|_| ProviderError::Storage)?;
                if n == 0 {
                    return Err(ProviderError::InvalidResponse);
                }
                read += n;
            }
            check_file(item)?;
            Ok(Bytes::from(data))
        });
        let data = tokio::select! { biased; _ = cancel.cancelled() => return Err(ProviderError::Cancelled), result = task => result.map_err(|_| ProviderError::Storage)?? };
        Ok(Box::pin(futures_util::stream::once(
            async move { Ok(data) },
        )))
    }
    async fn action(
        &self,
        _: &ContentId,
        _: Action,
        cancel: CancellationToken,
    ) -> Result<(), ProviderError> {
        self.check(&cancel)?;
        Err(ProviderError::Unsupported)
    }
    async fn playback(
        &self,
        _: &ContentId,
        _: PlaybackEvent,
        cancel: CancellationToken,
    ) -> Result<(), ProviderError> {
        self.check(&cancel)?;
        Err(ProviderError::Unsupported)
    }
    async fn logout(&self) -> Result<(), ProviderError> {
        self.closed.store(true, Ordering::Release);
        Ok(())
    }
}
