//! Host adapter for the BG Media v1 range ABI. bg-tok itself remains safe Rust.
use bg_tok::{
    media::MediaDelivery,
    network::ByteRange,
    provider::{ContentId, ContentProvider, ProviderError, Source},
};
use futures_util::StreamExt;
use std::{
    ffi::{c_int, c_void},
    panic::{catch_unwind, AssertUnwindSafe},
    slice,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::runtime::Handle;
use tokio_util::sync::CancellationToken;

const OK: c_int = 0;
const CANCELLED: c_int = 1;
const TIMEOUT: c_int = 2;
const EXPIRED: c_int = 3;
const IO: c_int = 4;
const MAX_READ: usize = 1024 * 1024;

type Cancel = Option<extern "C" fn(*mut c_void) -> c_int>;

/// C layout matches bgmedia/video/range_source.h. Prefer the RAII wrapper.
#[repr(C)]
pub struct RangeV1 {
    pub context: *mut c_void,
    pub length: u64,
    pub read_at:
        unsafe extern "C" fn(*mut c_void, u64, *mut u8, usize, u32, Cancel, *mut c_void) -> c_int,
    pub release: unsafe extern "C" fn(*mut c_void),
}

struct Bridge {
    runtime: Handle,
    delivery: MediaDelivery<dyn ContentProvider>,
    id: ContentId,
    source: Mutex<Source>,
    length: u64,
}

pub struct RangeSource(RangeV1);

impl RangeSource {
    /// The host supplies the verified size of an immutable representation.
    /// Keep the I/O runtime running through release. Invoke the callbacks
    /// on non-Tokio threads, without holding UI or provider locks.
    pub fn new(
        runtime: Handle,
        provider: Arc<dyn ContentProvider>,
        session: CancellationToken,
        id: ContentId,
        source: Source,
        length: u64,
    ) -> Result<Self, ProviderError> {
        if length == 0 || length > i64::MAX as u64 || !provider.capabilities().ranges {
            return Err(ProviderError::Unsupported);
        }
        let context = Box::new(Bridge {
            runtime,
            delivery: MediaDelivery::new(provider, session),
            id,
            source: Mutex::new(source),
            length,
        });
        Ok(Self(RangeV1 {
            context: Box::into_raw(context).cast(),
            length,
            read_at,
            release,
        }))
    }

    /// Transfer ownership to a host which must eventually call release once.
    pub fn into_raw(self) -> RangeV1 {
        let raw = std::mem::ManuallyDrop::new(self);
        RangeV1 {
            context: raw.0.context,
            length: raw.0.length,
            read_at,
            release,
        }
    }
}

impl Drop for RangeSource {
    fn drop(&mut self) {
        // This wrapper owns the Box until into_raw transfers it.
        unsafe { release(self.0.context) };
    }
}

async fn read_range(
    bridge: &Bridge,
    offset: u64,
    output: &mut [u8],
    cancel: CancellationToken,
) -> Result<(), ProviderError> {
    let range = ByteRange {
        start: offset,
        end: Some(offset + output.len() as u64 - 1),
    };
    let original = bridge
        .source
        .lock()
        .map_err(|_| ProviderError::InvalidResponse)?
        .clone();
    let (mut stream, resolved) = bridge
        .delivery
        .open_resolved(&bridge.id, original.clone(), Some(range), cancel)
        .await?;
    if resolved.cache != original.cache {
        return Err(ProviderError::SourceExpired);
    }
    {
        let mut source = bridge
            .source
            .lock()
            .map_err(|_| ProviderError::InvalidResponse)?;
        if *source == original {
            *source = resolved;
        }
    }
    let mut filled = 0;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if chunk.is_empty() || chunk.len() > output.len() - filled {
            return Err(ProviderError::InvalidResponse);
        }
        output[filled..filled + chunk.len()].copy_from_slice(&chunk);
        filled += chunk.len();
    }
    if filled == output.len() {
        Ok(())
    } else {
        Err(ProviderError::InvalidResponse)
    }
}

unsafe extern "C" fn read_at(
    context: *mut c_void,
    offset: u64,
    output: *mut u8,
    count: usize,
    timeout_ms: u32,
    cancelled: Cancel,
    cancel_context: *mut c_void,
) -> c_int {
    if context.is_null()
        || output.is_null()
        || count == 0
        || count > MAX_READ
        || Handle::try_current().is_ok()
    {
        return IO;
    }
    if timeout_ms == 0 {
        return TIMEOUT;
    }
    catch_unwind(AssertUnwindSafe(|| {
        // The host owns both pointers for this synchronous call. Release must
        // wait for all outstanding reads, and each mutable span is exclusive.
        let bridge = unsafe { &*context.cast::<Bridge>() };
        if offset > bridge.length || count as u64 > bridge.length - offset {
            return IO;
        }
        let output = unsafe { slice::from_raw_parts_mut(output, count) };
        let is_cancelled = || cancelled.is_some_and(|f| f(cancel_context) != 0);
        if is_cancelled() {
            return CANCELLED;
        }
        let token = CancellationToken::new();
        let _guard = token.clone().drop_guard();
        bridge.runtime.block_on(async {
            tokio::select! {
                biased;
                _ = async {
                    loop {
                        if is_cancelled() { break; }
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                } => CANCELLED,
                _ = tokio::time::sleep(Duration::from_millis(u64::from(timeout_ms))) => TIMEOUT,
                result = read_range(bridge, offset, output, token) => match result {
                    Ok(()) => OK,
                    Err(ProviderError::Cancelled | ProviderError::SessionClosed) => CANCELLED,
                    Err(ProviderError::SourceExpired) => EXPIRED,
                    Err(_) => IO,
                },
            }
        })
    }))
    .unwrap_or(IO)
}

unsafe extern "C" fn release(context: *mut c_void) {
    if !context.is_null() {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            // The transfer contract permits exactly one release.
            drop(unsafe { Box::from_raw(context.cast::<Bridge>()) });
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bg_tok_mock::{id, MockProvider};
    use std::{
        ptr,
        time::{Instant, SystemTime},
    };

    #[test]
    fn reads_exact_ranges_refreshes_and_respects_timeout_and_logout() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let provider = Arc::new(MockProvider::default());
        let session = CancellationToken::new();
        let item = id("one");
        let mut source = runtime
            .block_on(provider.sources(&item, session.clone()))
            .unwrap()
            .remove(0);
        source.expires_at = Some(SystemTime::UNIX_EPOCH);
        let mut expected = b"synthetic-media".to_vec();
        let range = RangeSource::new(
            runtime.handle().clone(),
            provider.clone(),
            session.clone(),
            item,
            source,
            expected.len() as u64,
        )
        .unwrap();
        let raw = &range.0;
        let call = |offset, out: &mut [u8], timeout| unsafe {
            (raw.read_at)(
                raw.context,
                offset,
                out.as_mut_ptr(),
                out.len(),
                timeout,
                None,
                ptr::null_mut(),
            )
        };
        let mut sample = [0; 5];
        assert_eq!(call(10, &mut sample, 1000), OK);
        assert_eq!(&sample, b"media");
        assert_eq!(call(0, &mut expected, 1000), OK);
        assert_eq!(expected, b"synthetic-media");
        assert_eq!(call(11, &mut sample, 1000), IO);
        provider.set_read_delay(Duration::from_secs(1));
        assert_eq!(call(0, &mut sample, 20), TIMEOUT);
        extern "C" fn elapsed(context: *mut c_void) -> c_int {
            let start = unsafe { &*context.cast::<Instant>() };
            (start.elapsed() > Duration::from_millis(15)) as c_int
        }
        let mut start = Instant::now();
        assert_eq!(
            unsafe {
                (raw.read_at)(
                    raw.context,
                    0,
                    sample.as_mut_ptr(),
                    sample.len(),
                    1000,
                    Some(elapsed),
                    (&mut start as *mut Instant).cast(),
                )
            },
            CANCELLED
        );
        session.cancel();
        assert_eq!(call(0, &mut sample, 1000), CANCELLED);
        assert!(runtime.block_on(provider.events()).is_empty());
        let transferred = range.into_raw();
        unsafe { (transferred.release)(transferred.context) };
    }
}
