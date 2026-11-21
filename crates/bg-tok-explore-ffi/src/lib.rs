//! Experimental async Explore host. Live access is separate from offline CI.
use bg_tok::{
    media::MediaDelivery,
    network::ByteRange,
    provider::{CachePolicy, ContentProvider, Metadata, ProviderError, Source},
};
use bg_tok_range::{RangeSource, RangeV1};
use bg_tok_tiktok::{TikTokConfig, TikTokProvider};
use futures_util::{FutureExt, StreamExt};
use std::{
    ffi::{c_char, CStr},
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::runtime::Runtime;
use tokio_util::sync::CancellationToken;

struct Item {
    metadata: Metadata,
    source: Source,
}
enum State {
    Loading,
    Ready(Vec<Item>),
    Failed,
    Cancelled,
}
pub struct Host {
    runtime: Arc<Runtime>,
    provider: Arc<TikTokProvider>,
    session: CancellationToken,
    state: Arc<Mutex<State>>,
}

async fn verify<P: ContentProvider + ?Sized>(
    provider: Arc<P>,
    id: &bg_tok::provider::ContentId,
    source: Source,
    session: CancellationToken,
) -> Result<Source, ProviderError> {
    if source.cache != CachePolicy::NoStore
        || source.length.is_none_or(|n| n == 0 || n > i64::MAX as u64)
        || source.revision.as_ref().is_none_or(String::is_empty)
    {
        return Err(ProviderError::InvalidResponse);
    }
    let delivery = MediaDelivery::new(provider, session.clone());
    let (mut stream, resolved) = delivery
        .open_resolved(
            id,
            source.clone(),
            Some(ByteRange {
                start: 0,
                end: Some(0),
            }),
            session,
        )
        .await?;
    if resolved.length != source.length
        || resolved.revision != source.revision
        || resolved.cache != CachePolicy::NoStore
    {
        return Err(ProviderError::SourceExpired);
    }
    let mut total = 0usize;
    while let Some(chunk) = stream.next().await {
        total = total
            .checked_add(chunk?.len())
            .ok_or(ProviderError::InvalidResponse)?;
        if total > 1 {
            return Err(ProviderError::InvalidResponse);
        }
    }
    if total != 1 {
        return Err(ProviderError::InvalidResponse);
    }
    Ok(resolved)
}
async fn prepare(
    provider: Arc<TikTokProvider>,
    session: CancellationToken,
    category: u32,
) -> Result<Vec<Item>, ProviderError> {
    let page = provider
        .explore_page(category, None, session.clone())
        .await?;
    let mut items = Vec::new();
    for metadata in page.items {
        if session.is_cancelled() {
            return Err(ProviderError::Cancelled);
        }
        let Ok(sources) = provider.sources(&metadata.id, session.clone()).await else {
            continue;
        };
        for source in sources {
            if let Ok(source) =
                verify(provider.clone(), &metadata.id, source, session.clone()).await
            {
                items.push(Item {
                    metadata: metadata.clone(),
                    source,
                });
                break;
            }
        }
        if items.len() == 3 {
            break;
        }
    }
    if items.len() != 3 {
        return Err(ProviderError::NotFound);
    }
    Ok(items)
}

/// Start asynchronous Explore discovery.
/// # Safety
/// `proxy` must be null or a valid NUL-terminated UTF-8 string for this call.
/// All host calls must be serialized, and creation must run outside Tokio.
#[no_mangle]
pub unsafe extern "C" fn bg_tok_explore_create(proxy: *const c_char, category: u32) -> *mut Host {
    catch_unwind(AssertUnwindSafe(|| {
        let proxy = if proxy.is_null() {
            None
        } else {
            Some(unsafe { CStr::from_ptr(proxy) }.to_str().ok()?.to_owned())
        };
        let mut config = TikTokConfig::default();
        config.http.proxy = proxy;
        config.http.connect_timeout = Duration::from_secs(3);
        config.http.request_timeout = Duration::from_secs(8);
        config.http.max_retries = 0;
        let provider = Arc::new(TikTokProvider::new(config).ok()?);
        let runtime = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .ok()?,
        );
        let session = CancellationToken::new();
        let state = Arc::new(Mutex::new(State::Loading));
        let (p, token, result) = (provider.clone(), session.clone(), state.clone());
        runtime.spawn(async move {
            let work = AssertUnwindSafe(tokio::time::timeout(
                Duration::from_secs(30),
                prepare(p, token.clone(), category),
            ))
            .catch_unwind()
            .await;
            let next = if token.is_cancelled() {
                State::Cancelled
            } else {
                match work {
                    Ok(Ok(Ok(items))) => State::Ready(items),
                    _ => State::Failed,
                }
            };
            if let Ok(mut state) = result.lock() {
                *state = next;
            }
        });
        Some(Box::into_raw(Box::new(Host {
            runtime,
            provider,
            session,
            state,
        })))
    }))
    .ok()
    .flatten()
    .unwrap_or(std::ptr::null_mut())
}
/// # Safety
/// `host` must be null or a live host. Host calls must be serialized.
#[no_mangle]
pub unsafe extern "C" fn bg_tok_explore_status(host: *const Host) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(host) = (unsafe { host.as_ref() }) else {
            return 2;
        };
        if host.session.is_cancelled() {
            return 3;
        }
        match host.state.lock().as_deref() {
            Ok(State::Loading) => 0,
            Ok(State::Ready(_)) => 1,
            Ok(State::Cancelled) => 3,
            _ => 2,
        }
    }))
    .unwrap_or(2)
}
/// # Safety
/// `host` must be null or a live host. Host calls must be serialized.
#[no_mangle]
pub unsafe extern "C" fn bg_tok_explore_count(host: *const Host) -> usize {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(host) = (unsafe { host.as_ref() }) else {
            return 0;
        };
        if host.session.is_cancelled() {
            return 0;
        }
        match host.state.lock().as_deref() {
            Ok(State::Ready(items)) => items.len(),
            _ => 0,
        }
    }))
    .unwrap_or(0)
}
/// # Safety
/// `host` must be null or live, and calls must be serialized. If `out` is
/// non-null and capacity is sufficient, it must be writable for `capacity` bytes.
#[no_mangle]
pub unsafe extern "C" fn bg_tok_explore_metadata(
    host: *const Host,
    index: usize,
    field: i32,
    out: *mut c_char,
    capacity: usize,
) -> usize {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(host) = (unsafe { host.as_ref() }) else {
            return 0;
        };
        if host.session.is_cancelled() {
            return 0;
        }
        let Ok(state) = host.state.lock() else {
            return 0;
        };
        let State::Ready(items) = &*state else {
            return 0;
        };
        let Some(item) = items.get(index) else {
            return 0;
        };
        let value = match field {
            0 => &item.metadata.id.value,
            1 => &item.metadata.author,
            2 => &item.metadata.description,
            _ => return 0,
        };
        if value.as_bytes().contains(&0) {
            return 0;
        }
        let Some(required) = value.len().checked_add(1) else {
            return 0;
        };
        if !out.is_null() && capacity >= required {
            unsafe {
                std::ptr::copy_nonoverlapping(value.as_ptr(), out.cast::<u8>(), value.len());
                out.add(value.len()).write(0);
            }
        }
        required
    }))
    .unwrap_or(0)
}
/// # Safety
/// `host` must be null or live, and calls must be serialized. `out` must be
/// null or valid and aligned for one range. On success release it once after
/// all reads finish; callbacks and release must run outside Tokio workers.
#[no_mangle]
pub unsafe extern "C" fn bg_tok_explore_open(
    host: *mut Host,
    index: usize,
    out: *mut RangeV1,
) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(host) = (unsafe { host.as_mut() }) else {
            return 4;
        };
        if out.is_null() || host.session.is_cancelled() {
            return 4;
        }
        let Ok(state) = host.state.lock() else {
            return 4;
        };
        let State::Ready(items) = &*state else {
            return 4;
        };
        let Some(item) = items.get(index) else {
            return 4;
        };
        let Some(length) = item.source.length else {
            return 4;
        };
        let Ok(range) = RangeSource::new_owned_runtime(
            host.runtime.clone(),
            host.provider.clone(),
            host.session.clone(),
            item.metadata.id.clone(),
            item.source.clone(),
            length,
        ) else {
            return 4;
        };
        unsafe { out.write(range.into_raw()) };
        0
    }))
    .unwrap_or(4)
}
/// # Safety
/// `host` must be null or a live host. Host calls must be serialized.
#[no_mangle]
pub unsafe extern "C" fn bg_tok_explore_cancel(host: *mut Host) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if let Some(host) = unsafe { host.as_mut() } {
            host.session.cancel();
        }
    }));
}
/// # Safety
/// `host` must be null or a live host not previously destroyed. Serialize host
/// calls and destroy outside Tokio worker threads. Active ranges are cancelled.
#[no_mangle]
pub unsafe extern "C" fn bg_tok_explore_destroy(host: *mut Host) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if !host.is_null() {
            let host = unsafe { Box::from_raw(host) };
            host.session.cancel();
            drop(host);
        }
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use bg_tok_mock::{id, MockProvider};

    #[tokio::test]
    async fn verifies_exact_byte_and_rejects_unsafe_sources() {
        let provider = Arc::new(MockProvider::default());
        let id = id("one");
        let source = provider
            .sources(&id, CancellationToken::new())
            .await
            .unwrap()
            .remove(0);
        assert!(verify(
            provider.clone(),
            &id,
            source.clone(),
            CancellationToken::new()
        )
        .await
        .is_ok());
        let mut unsafe_source = source.clone();
        unsafe_source.cache = CachePolicy::HostManaged;
        assert_eq!(
            verify(
                provider.clone(),
                &id,
                unsafe_source,
                CancellationToken::new()
            )
            .await
            .unwrap_err(),
            ProviderError::InvalidResponse
        );
        let mut missing_revision = source;
        missing_revision.revision = None;
        assert_eq!(
            verify(provider, &id, missing_revision, CancellationToken::new())
                .await
                .unwrap_err(),
            ProviderError::InvalidResponse
        );
    }

    #[test]
    fn cancellation_is_immediate_and_destroy_is_bounded() {
        let runtime = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .unwrap(),
        );
        let provider = Arc::new(TikTokProvider::new(TikTokConfig::default()).unwrap());
        let host = Box::into_raw(Box::new(Host {
            runtime,
            provider,
            session: CancellationToken::new(),
            state: Arc::new(Mutex::new(State::Loading)),
        }));
        unsafe {
            bg_tok_explore_cancel(host);
            assert_eq!(bg_tok_explore_status(host), 3);
            assert_eq!(bg_tok_explore_count(host), 0);
            bg_tok_explore_destroy(host);
        }
    }
}
