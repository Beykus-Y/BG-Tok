//! C-facing host for the Linux-only integration fixture, not a production API.
#![cfg(target_os = "linux")]
use bg_tok::{
    feed::Feed,
    provider::{ContentId, ContentProvider, FeedKind},
};
use bg_tok_file_fixture::FileProvider;
use bg_tok_range::{RangeSource, RangeV1};
use std::{
    ffi::{c_char, CStr, OsStr},
    os::unix::ffi::OsStrExt,
    panic::{catch_unwind, AssertUnwindSafe},
    path::PathBuf,
    sync::Arc,
};
use tokio::runtime::Runtime;
use tokio_util::sync::CancellationToken;

pub struct Host {
    runtime: Arc<Runtime>,
    provider: Arc<FileProvider>,
    session: CancellationToken,
    items: Vec<ContentId>,
}
#[no_mangle]
/// Create a test host from `count` NUL-terminated Linux paths.
///
/// # Safety
/// `paths` must point to `count` readable pointers to valid NUL-terminated
/// strings for the duration of the call. Calls must be made outside Tokio.
pub unsafe extern "C" fn bg_tok_fixture_create(
    paths: *const *const c_char,
    count: usize,
) -> *mut Host {
    catch_unwind(AssertUnwindSafe(|| {
        if paths.is_null() || count == 0 || count > 100 {
            return std::ptr::null_mut();
        }
        let mut files = Vec::with_capacity(count);
        for &path in unsafe { std::slice::from_raw_parts(paths, count) } {
            if path.is_null() {
                return std::ptr::null_mut();
            }
            files.push(PathBuf::from(OsStr::from_bytes(
                unsafe { CStr::from_ptr(path) }.to_bytes(),
            )));
        }
        let result = (|| {
            let runtime = Arc::new(
                tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .build()
                    .ok()?,
            );
            let rate = std::env::var("BG_TOK_FIXTURE_BYTES_PER_SECOND")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            let provider = Arc::new(FileProvider::open(&files, 4).ok()?.with_rate_limit(rate));
            let session = CancellationToken::new();
            let feed = Feed::with_session(
                provider.clone(),
                FeedKind::Recommended,
                count,
                session.clone(),
            );
            runtime
                .block_on(feed.load_next(CancellationToken::new()))
                .ok()?;
            let items = runtime
                .block_on(feed.snapshot())
                .into_iter()
                .map(|m| m.id)
                .collect();
            Some(Box::into_raw(Box::new(Host {
                runtime,
                provider,
                session,
                items,
            })))
        })();
        result.unwrap_or(std::ptr::null_mut())
    }))
    .unwrap_or(std::ptr::null_mut())
}
#[no_mangle]
/// # Safety
/// `host` must be null or a live host returned by `bg_tok_fixture_create`.
/// Calls involving the same host must be serialized against all host calls.
pub unsafe extern "C" fn bg_tok_fixture_count(host: *const Host) -> usize {
    catch_unwind(AssertUnwindSafe(|| {
        unsafe { host.as_ref() }.map_or(0, |h| h.items.len())
    }))
    .unwrap_or(0)
}
#[no_mangle]
/// # Safety
/// `host` must be null or a live host returned by `bg_tok_fixture_create`.
/// Calls involving the same host must be serialized against all host calls.
pub unsafe extern "C" fn bg_tok_fixture_read_calls(host: *const Host) -> u64 {
    catch_unwind(AssertUnwindSafe(|| {
        unsafe { host.as_ref() }.map_or(0, |h| h.provider.read_calls() as u64)
    }))
    .unwrap_or(0)
}
#[no_mangle]
/// # Safety
/// `host` must be null or a live host, with host calls serialized. If `out`
/// is non-null and `capacity` is sufficient, it must point to writable storage
/// of at least `capacity` bytes. Call outside Tokio worker threads.
pub unsafe extern "C" fn bg_tok_fixture_revision(
    host: *mut Host,
    index: usize,
    out: *mut c_char,
    capacity: usize,
) -> usize {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(host) = (unsafe { host.as_mut() }) else {
            return 0;
        };
        let Some(id) = host.items.get(index) else {
            return 0;
        };
        let Ok(sources) = host
            .runtime
            .block_on(host.provider.sources(id, host.session.clone()))
        else {
            return 0;
        };
        let Some(revision) = sources.first().and_then(|s| s.revision.as_ref()) else {
            return 0;
        };
        let Some(required) = revision.len().checked_add(1) else {
            return 0;
        };
        if !out.is_null() && capacity >= required {
            unsafe {
                std::ptr::copy_nonoverlapping(revision.as_ptr(), out.cast::<u8>(), revision.len());
                out.add(revision.len()).write(0);
            }
        }
        required
    }))
    .unwrap_or(0)
}
#[no_mangle]
/// # Safety
/// `host` must be null or a live host, with host calls serialized. If non-null,
/// `out` must be aligned and writable for one `RangeV1`. On success its
/// context is transferred to the caller, who must release it exactly once
/// after all reads finish. Call outside Tokio worker threads.
pub unsafe extern "C" fn bg_tok_fixture_open(
    host: *mut Host,
    index: usize,
    out: *mut RangeV1,
) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(host) = (unsafe { host.as_mut() }) else {
            return 4;
        };
        if out.is_null() {
            return 4;
        }
        let Some(id) = host.items.get(index) else {
            return 4;
        };
        let Ok(mut sources) = host
            .runtime
            .block_on(host.provider.sources(id, host.session.clone()))
        else {
            return 4;
        };
        if sources.len() != 1 {
            return 4;
        }
        let source = sources.remove(0);
        let Some(length) = source.length else {
            return 4;
        };
        let Ok(range) = RangeSource::new_owned_runtime(
            host.runtime.clone(),
            host.provider.clone(),
            host.session.clone(),
            id.clone(),
            source,
            length,
        ) else {
            return 4;
        };
        unsafe { out.write(range.into_raw()) };
        0
    }))
    .unwrap_or(4)
}
#[no_mangle]
/// # Safety
/// `host` must be null or a live host returned by `bg_tok_fixture_create`.
/// Calls involving the same host must be serialized against all host calls.
pub unsafe extern "C" fn bg_tok_fixture_cancel(host: *mut Host) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if let Some(host) = unsafe { host.as_mut() } {
            host.session.cancel();
        }
    }));
}
#[no_mangle]
/// # Safety
/// `host` must be null or a live host returned by `bg_tok_fixture_create`,
/// and may be destroyed only once. Serialize with other calls on the host.
/// Destruction must occur outside Tokio worker threads.
pub unsafe extern "C" fn bg_tok_fixture_destroy(host: *mut Host) {
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
    use std::{ffi::CString, fs, ptr};
    #[test]
    fn ranges_keep_runtime_alive_after_host_cancellation() {
        let path =
            std::env::temp_dir().join(format!("bg-tok-host-lifetime-{}", std::process::id()));
        fs::write(&path, b"fixture bytes").unwrap();
        let name = CString::new(path.as_os_str().as_bytes()).unwrap();
        let paths = [name.as_ptr()];
        unsafe {
            let host = bg_tok_fixture_create(paths.as_ptr(), paths.len());
            assert!(!host.is_null());
            assert_eq!(bg_tok_fixture_count(host), 1);
            let mut raw = std::mem::MaybeUninit::<RangeV1>::uninit();
            assert_eq!(bg_tok_fixture_open(host, 0, raw.as_mut_ptr()), 0);
            let raw = raw.assume_init();
            bg_tok_fixture_destroy(host);
            let mut bytes = [0; 4];
            assert_eq!(
                (raw.read_at)(
                    raw.context,
                    0,
                    bytes.as_mut_ptr(),
                    bytes.len(),
                    1000,
                    None,
                    ptr::null_mut()
                ),
                1
            );
            (raw.release)(raw.context);
        }
        fs::remove_file(path).unwrap();
    }
}
