//! Explicit live test. Requires permitted network access and optionally
//! BG_TOK_LIVE_PROXY. Never run as part of ordinary CI.
use bg_tok_explore_ffi::*;
use std::{
    ffi::CString,
    mem::MaybeUninit,
    ptr, thread,
    time::{Duration, Instant},
};

#[test]
#[ignore = "requires live TikTok Explore and CDN access"]
fn explore_three_sources_through_c_abi() {
    let proxy = std::env::var("BG_TOK_LIVE_PROXY")
        .ok()
        .map(|p| CString::new(p).unwrap());
    let host =
        unsafe { bg_tok_explore_create(proxy.as_ref().map_or(ptr::null(), |s| s.as_ptr()), 120) };
    assert!(!host.is_null(), "could not create Explore host");
    let deadline = Instant::now() + Duration::from_secs(35);
    let status = loop {
        let status = unsafe { bg_tok_explore_status(host) };
        if status != 0 || Instant::now() >= deadline {
            break status;
        }
        thread::sleep(Duration::from_millis(20));
    };
    if status != 1 {
        unsafe { bg_tok_explore_destroy(host) };
        panic!("live Explore unavailable or timed out (status {status})");
    }
    assert_eq!(unsafe { bg_tok_explore_count(host) }, 3);
    for index in 0..3 {
        let mut raw = MaybeUninit::uninit();
        assert_eq!(
            unsafe { bg_tok_explore_open(host, index, raw.as_mut_ptr()) },
            0
        );
        let raw = unsafe { raw.assume_init() };
        assert!(raw.length >= 4096);
        let mut data = [0u8; 4096];
        assert_eq!(
            unsafe {
                (raw.read_at)(
                    raw.context,
                    0,
                    data.as_mut_ptr(),
                    data.len(),
                    10_000,
                    None,
                    ptr::null_mut(),
                )
            },
            0
        );
        assert_eq!(&data[4..8], b"ftyp");
        unsafe { (raw.release)(raw.context) };
    }
    unsafe {
        bg_tok_explore_cancel(host);
        bg_tok_explore_destroy(host)
    };
}
