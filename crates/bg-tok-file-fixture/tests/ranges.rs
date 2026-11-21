#![cfg(target_os = "linux")]
use bg_tok::{
    feed::Feed,
    provider::{ContentProvider, FeedKind},
};
use bg_tok_file_fixture::FileProvider;
use bg_tok_range::RangeSource;
use std::{fs, process::Command, ptr, sync::Arc};
use tokio_util::sync::CancellationToken;

#[test]
fn real_mp4_ranges_travel_through_delivery_and_ffi() {
    let dir = std::env::temp_dir().join(format!("bg-tok-fixture-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("sample.mp4");
    let status = Command::new("ffmpeg")
        .args([
            "-nostdin",
            "-v",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=320x180:rate=10",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440",
            "-t",
            "0.5",
            "-c:v",
            "libx264",
            "-threads",
            "1",
            "-c:a",
            "aac",
            "-movflags",
            "+faststart",
        ])
        .arg(&path)
        .status()
        .expect("ffmpeg is required for this integration test");
    assert!(status.success());
    let file = fs::read(&path).unwrap();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let provider = Arc::new(FileProvider::open(&[&path], 2).unwrap());
    let session = CancellationToken::new();
    let feed = Feed::with_session(provider.clone(), FeedKind::Recommended, 3, session.clone());
    runtime
        .block_on(feed.load_next(CancellationToken::new()))
        .unwrap();
    let item = runtime.block_on(feed.get(0)).unwrap();
    let source = runtime
        .block_on(provider.sources(&item.id, CancellationToken::new()))
        .unwrap()
        .remove(0);
    assert_eq!(source.length, Some(file.len() as u64));
    let handle = RangeSource::new(
        runtime.handle().clone(),
        provider.clone(),
        session.clone(),
        item.id,
        source,
        file.len() as u64,
    )
    .unwrap()
    .into_raw();
    for offset in [0, file.len() / 2, file.len() - 32] {
        let mut actual = [0; 32];
        let result = unsafe {
            (handle.read_at)(
                handle.context,
                offset as u64,
                actual.as_mut_ptr(),
                actual.len(),
                1000,
                None,
                ptr::null_mut(),
            )
        };
        assert_eq!(result, 0);
        assert_eq!(&actual[..], &file[offset..offset + 32]);
    }
    assert_eq!(provider.read_calls(), 3);
    let context = handle.context as usize;
    let read_at = handle.read_at;
    std::thread::scope(|threads| {
        for index in 0..12 {
            let expected = &file;
            threads.spawn(move || {
                let offset = index * 37;
                let mut actual = [0; 32];
                let status = unsafe {
                    read_at(
                        context as *mut _,
                        offset as u64,
                        actual.as_mut_ptr(),
                        actual.len(),
                        1000,
                        None,
                        ptr::null_mut(),
                    )
                };
                assert_eq!(status, 0);
                assert_eq!(&actual[..], &expected[offset..offset + 32]);
            });
        }
    });
    assert_eq!(provider.read_calls(), 15);
    session.cancel();
    let mut byte = [0];
    assert_eq!(
        unsafe {
            (handle.read_at)(
                handle.context,
                0,
                byte.as_mut_ptr(),
                1,
                1000,
                None,
                ptr::null_mut(),
            )
        },
        1
    );
    unsafe { (handle.release)(handle.context) };
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn slow_file_fixture_honors_timeout_and_cross_thread_cancellation() {
    use std::time::{Duration, Instant};
    let path = std::env::temp_dir().join(format!("bg-tok-throttle-{}", std::process::id()));
    fs::write(&path, [7u8; 128]).unwrap();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let provider = Arc::new(FileProvider::open(&[&path], 1).unwrap().with_rate_limit(16));
    let item = bg_tok::provider::ContentId {
        namespace: "file-fixture".into(),
        value: "0".into(),
    };
    let session = CancellationToken::new();
    let source = runtime
        .block_on(provider.sources(&item, CancellationToken::new()))
        .unwrap()
        .remove(0);
    let raw = RangeSource::new(
        runtime.handle().clone(),
        provider,
        session.clone(),
        item,
        source,
        128,
    )
    .unwrap()
    .into_raw();
    let mut out = [0; 32];
    let start = Instant::now();
    assert_eq!(
        unsafe {
            (raw.read_at)(
                raw.context,
                0,
                out.as_mut_ptr(),
                out.len(),
                30,
                None,
                ptr::null_mut(),
            )
        },
        2
    );
    assert!(start.elapsed() < Duration::from_secs(1));
    let cancel = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(30));
        session.cancel();
    });
    assert_eq!(
        unsafe {
            (raw.read_at)(
                raw.context,
                0,
                out.as_mut_ptr(),
                out.len(),
                1000,
                None,
                ptr::null_mut(),
            )
        },
        1
    );
    cancel.join().unwrap();
    unsafe { (raw.release)(raw.context) };
    fs::remove_file(path).unwrap();
}

#[test]
fn rejects_missing_or_mutated_input() {
    let path = std::env::temp_dir().join(format!("bg-tok-mutated-{}", std::process::id()));
    let _ = fs::remove_file(&path);
    assert!(FileProvider::open(&[&path], 1).is_err());
    fs::write(&path, [3u8; 128]).unwrap();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let provider = Arc::new(FileProvider::open(&[&path], 1).unwrap());
    let id = bg_tok::provider::ContentId {
        namespace: "file-fixture".into(),
        value: "0".into(),
    };
    let source = runtime
        .block_on(provider.sources(&id, CancellationToken::new()))
        .unwrap()
        .remove(0);
    let raw = RangeSource::new(
        runtime.handle().clone(),
        provider,
        CancellationToken::new(),
        id,
        source,
        128,
    )
    .unwrap()
    .into_raw();
    fs::write(&path, [4u8; 129]).unwrap();
    let mut buffer = [0; 16];
    assert_eq!(
        unsafe {
            (raw.read_at)(
                raw.context,
                0,
                buffer.as_mut_ptr(),
                buffer.len(),
                1000,
                None,
                ptr::null_mut(),
            )
        },
        3
    );
    unsafe { (raw.release)(raw.context) };
    fs::remove_file(path).unwrap();
}
