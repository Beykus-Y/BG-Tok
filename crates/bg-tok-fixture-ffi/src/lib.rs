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
    runtime: Runtime,
    provider: Arc<FileProvider>,
    session: CancellationToken,
    items: Vec<ContentId>,
}
#[no_mangle]
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
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .ok()?;
            let provider = Arc::new(FileProvider::open(&files, 4).ok()?);
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
pub unsafe extern "C" fn bg_tok_fixture_count(host: *const Host) -> usize {
    catch_unwind(AssertUnwindSafe(|| {
        unsafe { host.as_ref() }.map_or(0, |h| h.items.len())
    }))
    .unwrap_or(0)
}
#[no_mangle]
pub unsafe extern "C" fn bg_tok_fixture_read_calls(host: *const Host) -> u64 {
    catch_unwind(AssertUnwindSafe(|| {
        unsafe { host.as_ref() }.map_or(0, |h| h.provider.read_calls() as u64)
    }))
    .unwrap_or(0)
}
#[no_mangle]
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
        let Ok(range) = RangeSource::new(
            host.runtime.handle().clone(),
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
pub unsafe extern "C" fn bg_tok_fixture_cancel(host: *mut Host) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if let Some(host) = unsafe { host.as_mut() } {
            host.session.cancel();
        }
    }));
}
#[no_mangle]
pub unsafe extern "C" fn bg_tok_fixture_destroy(host: *mut Host) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if !host.is_null() {
            let host = unsafe { Box::from_raw(host) };
            host.session.cancel();
            drop(host);
        }
    }));
}
