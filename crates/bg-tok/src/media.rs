//! Host-initiated byte delivery; no prefetch, playback or persistent storage.
use crate::{
    network::ByteRange,
    provider::{ContentId, ContentProvider, MediaStream, ProviderError, Source},
};
use futures_util::StreamExt;
use std::{sync::Arc, time::SystemTime};
use tokio_util::sync::CancellationToken;

pub struct MediaDelivery<P: ContentProvider + ?Sized> {
    provider: Arc<P>,
    session: CancellationToken,
    max_chunk_bytes: usize,
}
impl<P: ContentProvider + ?Sized> MediaDelivery<P> {
    pub fn new(provider: Arc<P>, session: CancellationToken) -> Self {
        Self::with_chunk_limit(provider, session, 1024 * 1024)
    }
    /// Limits each chunk passed to the host. Providers must also bound their own buffers.
    pub fn with_chunk_limit(
        provider: Arc<P>,
        session: CancellationToken,
        max_chunk_bytes: usize,
    ) -> Self {
        Self {
            provider,
            session,
            max_chunk_bytes,
        }
    }
    /// Refresh an expired handle once before starting the body. The provider
    /// must return the same stable source ID; the host can reselect otherwise.
    pub async fn open(
        &self,
        id: &ContentId,
        source: Source,
        range: Option<ByteRange>,
        cancel: CancellationToken,
    ) -> Result<MediaStream, ProviderError> {
        self.open_resolved(id, source, range, cancel)
            .await
            .map(|(stream, _)| stream)
    }

    /// Like open, but returns the effective handle so range readers can reuse a refresh.
    pub async fn open_resolved(
        &self,
        id: &ContentId,
        mut source: Source,
        range: Option<ByteRange>,
        cancel: CancellationToken,
    ) -> Result<(MediaStream, Source), ProviderError> {
        let operation = cancel.child_token();
        let guard = operation.clone().drop_guard();
        let mut refreshed = false;
        loop {
            if source
                .expires_at
                .is_some_and(|expiry| expiry <= SystemTime::now())
            {
                if refreshed {
                    return Err(ProviderError::SourceExpired);
                }
                source = self.refresh(id, &source.id, &cancel, &operation).await?;
                refreshed = true;
                continue;
            }
            let result = tokio::select! { biased; _ = self.session.cancelled() => { operation.cancel(); return Err(ProviderError::SessionClosed) }, _ = cancel.cancelled() => { operation.cancel(); return Err(ProviderError::Cancelled) }, r = self.provider.read(&source, range, operation.clone()) => r };
            match result {
                Err(ProviderError::SourceExpired) if !refreshed => {
                    source = self.refresh(id, &source.id, &cancel, &operation).await?;
                    refreshed = true;
                }
                Err(error) => return Err(error),
                Ok(stream) => {
                    let session = self.session.clone();
                    let limit = self.max_chunk_bytes;
                    let output = futures_util::stream::unfold(
                        (stream, session, cancel, operation, false, guard),
                        move |(mut stream, session, cancel, operation, done, guard)| async move {
                            if done {
                                return None;
                            }
                            tokio::select! { biased;
                                _ = session.cancelled() => { operation.cancel(); Some((Err(ProviderError::SessionClosed), (stream, session, cancel, operation, true, guard))) },
                                _ = cancel.cancelled() => { operation.cancel(); Some((Err(ProviderError::Cancelled), (stream, session, cancel, operation, true, guard))) },
                                item = stream.next() => item.map(|item| { let item = item.and_then(|bytes| if bytes.len() > limit { Err(ProviderError::Capacity) } else { Ok(bytes) }); let done = item.is_err(); (item, (stream, session, cancel, operation, done, guard)) })
                            }
                        },
                    );
                    return Ok((Box::pin(output), source));
                }
            }
        }
    }
    async fn refresh(
        &self,
        id: &ContentId,
        source_id: &str,
        cancel: &CancellationToken,
        operation: &CancellationToken,
    ) -> Result<Source, ProviderError> {
        let sources = tokio::select! { biased; _ = self.session.cancelled() => { operation.cancel(); return Err(ProviderError::SessionClosed) }, _ = cancel.cancelled() => { operation.cancel(); return Err(ProviderError::Cancelled) }, r = self.provider.sources(id, operation.clone()) => r? };
        sources
            .into_iter()
            .find(|source| source.id == source_id)
            .ok_or(ProviderError::SourceExpired)
    }
}
