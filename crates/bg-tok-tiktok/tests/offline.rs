//! Offline tests for `bg-tok-tiktok` using synthetic fixtures (no secrets, no network).
use async_trait::async_trait;
use bg_tok::{
    account::{AccountId, Accounts, Secret, SecureStore},
    media::MediaDelivery,
    network::{ByteRange, ByteStream, HttpResponse, NetworkError, Transport},
    provider::{ContentId, ContentProvider, FeedKind, ProviderError},
};
use bg_tok_tiktok::{
    auth::{
        build_authorize_url, parse_oauth_token_response, parse_qr_challenge,
        parse_qr_poll_response, LoginKitConfig, QrStatus, TikTokSession,
    },
    models::{VideoRef, TIKTOK_NAMESPACE},
    parser::{
        parse_comments_response, parse_display_user_info_response,
        parse_display_video_list_response, parse_explore_response, parse_oembed_response,
        parse_video_detail_html, parse_video_ref,
    },
    TikTokConfig, TikTokProvider,
};
use bytes::Bytes;
use futures_util::{stream, StreamExt};
use http::{header, HeaderMap, Method};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use url::Url;
use zeroize::Zeroizing;

struct StubResponse {
    status: u16,
    headers: HeaderMap,
    body: Result<Vec<u8>, NetworkError>,
}

type HandlerFn = Box<
    dyn Fn(&Method, &Url, &HeaderMap, Option<ByteRange>) -> Result<StubResponse, NetworkError>
        + Send
        + Sync,
>;

struct MockTransport {
    calls: AtomicUsize,
    seen_cookies: Mutex<Vec<Option<String>>>,
    handler: HandlerFn,
}

impl MockTransport {
    fn new(handler: HandlerFn) -> Self {
        Self {
            calls: AtomicUsize::new(0),
            seen_cookies: Mutex::new(Vec::new()),
            handler,
        }
    }
}

#[async_trait]
impl Transport for MockTransport {
    async fn send(
        &self,
        method: Method,
        url: &Url,
        headers: HeaderMap,
        _body: Option<Bytes>,
        range: Option<ByteRange>,
        cancel: CancellationToken,
    ) -> Result<HttpResponse, NetworkError> {
        if cancel.is_cancelled() {
            return Err(NetworkError::Cancelled);
        }
        self.calls.fetch_add(1, Ordering::SeqCst);
        let cookie = headers
            .get(header::COOKIE)
            .and_then(|v| v.to_str().ok())
            .map(ToString::to_string);
        self.seen_cookies.lock().await.push(cookie);

        let stub = (self.handler)(&method, url, &headers, range)?;
        let body: ByteStream = match stub.body {
            Ok(bytes) => Box::pin(stream::iter(vec![Ok(Bytes::from(bytes))])),
            Err(err) => Box::pin(stream::iter(vec![Err(err)])),
        };
        Ok(HttpResponse {
            status: stub.status,
            headers: stub.headers,
            body,
        })
    }
}

fn sample_video_html(video_id: &str, expire_ts: u64, status_code: i64) -> String {
    let payload = serde_json::json!({
        "__DEFAULT_SCOPE__": {
            "webapp.video-detail": {
                "statusCode": status_code,
                "itemInfo": {
                    "itemStruct": {
                        "id": video_id,
                        "desc": "Synthetic test clip #rust",
                        "createTime": 1700000000u64,
                        "author": {
                            "uniqueId": "synthetic_author",
                            "nickname": "Synthetic"
                        },
                        "stats": {
                            "playCount": 12345,
                            "diggCount": 678,
                            "commentCount": 12,
                            "shareCount": 5
                        },
                        "video": {
                            "duration": 15,
                            "width": 576,
                            "height": 1024,
                            "cover": "https://p16-sign.tiktokcdn-us.com/cover.jpg",
                            "playAddr": format!("https://v16-webapp-prime.tiktok.com/video/test/?expire={expire_ts}"),
                            "bitrateInfo": [
                                {
                                    "GearName": "normal_540_0",
                                    "Bitrate": 1500000,
                                    "PlayAddr": {
                                        "DataSize": "10",
                                        "Width": 576,
                                        "Height": 1024,
                                        "FileHash": "abc123",
                                        "UrlList": [
                                            format!("https://v16-webapp-prime.tiktok.com/video/test/?expire={expire_ts}")
                                        ]
                                    }
                                }
                            ]
                        }
                    }
                }
            }
        }
    });
    format!(
        "<html><head><script id=\"__UNIVERSAL_DATA_FOR_REHYDRATION__\" type=\"application/json\">{}</script></head></html>",
        payload
    )
}

#[test]
fn parses_video_references() {
    assert_eq!(
        parse_video_ref("6718335390845095173").unwrap(),
        VideoRef::Id("6718335390845095173".into())
    );
    assert_eq!(
        parse_video_ref(
            "https://www.tiktok.com/@scout2015/video/6718335390845095173?is_from_webapp=1"
        )
        .unwrap(),
        VideoRef::Id("6718335390845095173".into())
    );
    assert_eq!(
        parse_video_ref("https://m.tiktok.com/v/6718335390845095173.html").unwrap(),
        VideoRef::Id("6718335390845095173".into())
    );
    assert_eq!(
        parse_video_ref("https://vm.tiktok.com/ZMY12345/").unwrap(),
        VideoRef::Url("https://vm.tiktok.com/ZMY12345/".into())
    );
    assert_eq!(
        parse_video_ref("https://example.com/video/123"),
        Err(ProviderError::InvalidResponse)
    );
    assert_eq!(parse_video_ref(""), Err(ProviderError::InvalidResponse));
}

#[test]
fn parses_video_html_and_errors() {
    let html = sample_video_html("6718335390845095173", 1_900_000_000, 0);
    let video = parse_video_detail_html(&html).unwrap();
    assert_eq!(video.id, "6718335390845095173");
    assert_eq!(video.author, "synthetic_author");
    assert_eq!(video.duration, Some(Duration::from_secs(15)));
    assert_eq!(video.views, Some(12345));
    assert_eq!(video.sources.len(), 1);
    assert_eq!(
        video.sources[0].source_id,
        "6718335390845095173:normal_540_0"
    );
    assert_eq!(video.sources[0].length, Some(10));

    let not_found_html = sample_video_html("1", 1_900_000_000, 10204);
    assert_eq!(
        parse_video_detail_html(&not_found_html).unwrap_err(),
        ProviderError::NotFound
    );

    let unauth_html = sample_video_html("1", 1_900_000_000, 10202);
    assert_eq!(
        parse_video_detail_html(&unauth_html).unwrap_err(),
        ProviderError::Unauthorized
    );

    assert_eq!(
        parse_video_detail_html("<html>no data</html>").unwrap_err(),
        ProviderError::InvalidResponse
    );
}

#[test]
fn parses_explore_pagination_comments_oembed_and_display_api() {
    let explore_p1 = r#"{"status_code":0,"hasMore":true,"itemList":[{"id":"111","desc":"clip1","author":{"uniqueId":"u1"},"video":{"duration":7,"cover":"https://c/1.jpg"},"stats":{"playCount":10}}]}"#;
    let page1 = parse_explore_response(explore_p1, None).unwrap();
    assert_eq!(page1.items.len(), 1);
    assert_eq!(page1.next_cursor.as_deref(), Some("1"));

    let explore_p2 = r#"{"status_code":0,"hasMore":false,"itemList":[{"id":"222","desc":"clip2","author":{"uniqueId":"u2"},"video":{"duration":9},"stats":{"playCount":20}}]}"#;
    let page2 = parse_explore_response(explore_p2, page1.next_cursor.as_deref()).unwrap();
    assert_eq!(page2.items.len(), 1);
    assert_eq!(page2.next_cursor, None);

    let comments_json = r#"{"status_code":0,"has_more":1,"cursor":20,"total":42,"comments":[{"cid":"c1","text":"nice!","digg_count":3,"create_time":1700000001,"user":{"unique_id":"commenter"}}]}"#;
    let cpage = parse_comments_response(comments_json).unwrap();
    assert_eq!(cpage.comments.len(), 1);
    assert_eq!(cpage.next_cursor.as_deref(), Some("20"));
    assert_eq!(cpage.total, Some(42));

    let oembed_json = r#"{"title":"Demo","author_name":"Creator","author_url":"https://www.tiktok.com/@creator","embed_product_id":"123"}"#;
    let oembed = parse_oembed_response(oembed_json).unwrap();
    assert_eq!(oembed.author_name.as_deref(), Some("Creator"));

    let display_user = r#"{"data":{"user":{"open_id":"oid_1","display_name":"Tester","follower_count":5}},"error":{"code":"ok"}}"#;
    let u = parse_display_user_info_response(display_user).unwrap();
    assert_eq!(u.open_id, "oid_1");
    assert_eq!(u.follower_count, Some(5));

    let display_videos = r#"{"data":{"videos":[{"id":"v1","title":"My video","duration":12,"view_count":99}],"cursor":170000,"has_more":true},"error":{"code":"ok"}}"#;
    let dv = parse_display_video_list_response(display_videos).unwrap();
    assert_eq!(dv.items.len(), 1);
    assert_eq!(dv.next_cursor.as_deref(), Some("170000"));

    let rate_limited = r#"{"error":{"code":"rate_limit_exceeded"}}"#;
    assert_eq!(
        parse_display_user_info_response(rate_limited).unwrap_err(),
        ProviderError::RateLimited
    );
}

#[tokio::test]
async fn refreshes_expired_source_through_media_delivery() {
    let page_fetches = Arc::new(AtomicUsize::new(0));
    let page_fetches_clone = page_fetches.clone();

    let transport = Arc::new(MockTransport::new(Box::new(
        move |_method, url, _headers, range| {
            if url.path().starts_with("/@_/video/") {
                let n = page_fetches_clone.fetch_add(1, Ordering::SeqCst);
                let expire = if n == 0 { 1 } else { 4_000_000_000 };
                let mut h = HeaderMap::new();
                h.insert(
                    header::SET_COOKIE,
                    "tt_chain_token=test_chain; Path=/".parse().unwrap(),
                );
                return Ok(StubResponse {
                    status: 200,
                    headers: h,
                    body: Ok(sample_video_html("6718335390845095173", expire, 0).into_bytes()),
                });
            }
            assert!(range.is_some());
            Ok(StubResponse {
                status: 206,
                headers: HeaderMap::new(),
                body: Ok(b"0123456789".to_vec()),
            })
        },
    )));

    let provider = Arc::new(TikTokProvider::with_transport(
        transport,
        TikTokConfig {
            min_request_interval: Duration::ZERO,
            ..TikTokConfig::default()
        },
    ));
    let id = ContentId {
        namespace: TIKTOK_NAMESPACE.into(),
        value: "6718335390845095173".into(),
    };
    let initial_sources = provider
        .sources(&id, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(initial_sources.len(), 1);

    // First source has expire=1 (in the past); MediaDelivery must refresh once automatically.
    let delivery = MediaDelivery::new(provider.clone(), CancellationToken::new());
    let mut stream = delivery
        .open(
            &id,
            initial_sources[0].clone(),
            Some(ByteRange::new(0, Some(9)).unwrap()),
            CancellationToken::new(),
        )
        .await
        .unwrap();

    let mut received = Vec::new();
    while let Some(chunk) = stream.next().await {
        received.extend_from_slice(&chunk.unwrap());
    }
    assert_eq!(received, b"0123456789");
    assert_eq!(page_fetches.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn honors_capabilities_cancellation_rate_limits_and_logout() {
    let transport = Arc::new(MockTransport::new(Box::new(
        |_method, url, _headers, _range| {
            if url.path().contains("/api/explore/") {
                return Err(NetworkError::RateLimited);
            }
            Ok(StubResponse {
                status: 200,
                headers: HeaderMap::new(),
                body: Ok(sample_video_html("6718335390845095173", 4_000_000_000, 0).into_bytes()),
            })
        },
    )));

    let provider = TikTokProvider::with_transport(
        transport,
        TikTokConfig {
            min_request_interval: Duration::ZERO,
            ..TikTokConfig::default()
        },
    );

    let caps = provider.capabilities();
    assert!(!caps.recommended);
    assert!(caps.ranges);
    assert_eq!(
        provider
            .page(&FeedKind::Recommended, None, CancellationToken::new())
            .await
            .unwrap_err(),
        ProviderError::Unsupported
    );

    let cancelled = CancellationToken::new();
    cancelled.cancel();
    let id = ContentId {
        namespace: TIKTOK_NAMESPACE.into(),
        value: "6718335390845095173".into(),
    };
    assert_eq!(
        provider.sources(&id, cancelled).await.unwrap_err(),
        ProviderError::Cancelled
    );

    assert_eq!(
        provider
            .explore_page(120, None, CancellationToken::new())
            .await
            .unwrap_err(),
        ProviderError::RateLimited
    );

    let sources = provider
        .sources(&id, CancellationToken::new())
        .await
        .unwrap();
    provider.logout().await.unwrap();
    assert_eq!(
        provider
            .read(&sources[0], None, CancellationToken::new())
            .await
            .err()
            .unwrap(),
        ProviderError::SourceExpired
    );
}

struct MemoryStore(Mutex<HashMap<AccountId, Vec<u8>>>);

#[async_trait]
impl SecureStore for MemoryStore {
    async fn save(&self, id: &AccountId, secret: &Secret) -> Result<(), ProviderError> {
        self.0
            .lock()
            .await
            .insert(id.clone(), secret.expose().to_vec());
        Ok(())
    }
    async fn load(&self, id: &AccountId) -> Result<Option<Secret>, ProviderError> {
        Ok(self.0.lock().await.get(id).map(|b| Secret::new(b.clone())))
    }
    async fn delete(&self, id: &AccountId) -> Result<(), ProviderError> {
        self.0.lock().await.remove(id);
        Ok(())
    }
}

#[tokio::test]
async fn isolates_account_sessions_and_redacts_secrets() {
    let store = Arc::new(MemoryStore(Mutex::new(HashMap::new())));
    let accounts = Accounts::new(store.clone());

    let mut s1 = TikTokSession::default();
    s1.cookies
        .insert("sessionid".into(), Zeroizing::new("secret_acc_1".into()));
    let mut s2 = TikTokSession::default();
    s2.cookies
        .insert("sessionid".into(), Zeroizing::new("secret_acc_2".into()));

    let a1 = AccountId("acc1".into());
    let a2 = AccountId("acc2".into());
    accounts.save(&a1, &s1.to_secret().unwrap()).await.unwrap();
    accounts.save(&a2, &s2.to_secret().unwrap()).await.unwrap();

    let active1 = accounts.activate(&a1).await.unwrap();
    let loaded1 = TikTokSession::from_secret(&store.load(&a1).await.unwrap().unwrap()).unwrap();

    let transport = Arc::new(MockTransport::new(Box::new(|_, _, _, _| {
        Ok(StubResponse {
            status: 200,
            headers: HeaderMap::new(),
            body: Ok(sample_video_html("6718335390845095173", 4_000_000_000, 0).into_bytes()),
        })
    })));
    let provider = TikTokProvider::with_transport(
        transport.clone(),
        TikTokConfig {
            min_request_interval: Duration::ZERO,
            ..TikTokConfig::default()
        },
    );
    provider.apply_session(&loaded1).await;
    let _ = provider
        .resolve_video("6718335390845095173", active1.cancellation.clone())
        .await
        .unwrap();

    // Switching to account 2 cancels account 1's token and isolates cookies.
    let active2 = accounts.activate(&a2).await.unwrap();
    assert!(active1.cancellation.is_cancelled());
    let loaded2 = TikTokSession::from_secret(&store.load(&a2).await.unwrap().unwrap()).unwrap();
    provider.apply_session(&loaded2).await;
    let _ = provider
        .resolve_video("6718335390845095173", active2.cancellation.clone())
        .await
        .unwrap();

    let cookies = transport.seen_cookies.lock().await;
    assert_eq!(cookies[0].as_deref(), Some("sessionid=secret_acc_1"));
    assert_eq!(cookies[1].as_deref(), Some("sessionid=secret_acc_2"));

    let dbg = format!("{s1:?}");
    assert!(!dbg.contains("secret_acc_1"));
}

#[test]
fn builds_login_kit_and_parses_oauth_and_qr_responses() {
    let cfg = LoginKitConfig {
        client_key: "key123".into(),
        redirect_uri: "beygram://oauth/tiktok".into(),
        scopes: vec!["user.info.basic".into(), "video.list".into()],
    };
    let url = build_authorize_url(&cfg, "state_abc", Some("challenge_xyz")).unwrap();
    assert!(url
        .as_str()
        .starts_with("https://www.tiktok.com/v2/auth/authorize/"));
    assert!(url.as_str().contains("code_challenge_method=S256"));

    let token_json = br#"{"open_id":"oid","access_token":"tok_secret","refresh_token":"ref_secret","expires_in":86400,"scope":"user.info.basic","token_type":"Bearer"}"#;
    let tokens = parse_oauth_token_response(token_json).unwrap();
    let dbg = format!("{tokens:?}");
    assert!(!dbg.contains("tok_secret"));
    assert!(!dbg.contains("ref_secret"));

    let qr_json = br#"{"data":{"token":"qr_tok","qrcode":"iVBORw0=","qrcode_index_url":"https://www.tiktok.com/qr","expire_time":1795250446}}"#;
    let qr = parse_qr_challenge(qr_json).unwrap();
    assert_eq!(qr.token, "qr_tok");

    let mut headers = HeaderMap::new();
    headers.insert(
        header::SET_COOKIE,
        "sessionid=live_cookie; Path=/; HttpOnly".parse().unwrap(),
    );
    let confirmed =
        parse_qr_poll_response(br#"{"data":{"status":"confirmed"}}"#, &headers).unwrap();
    match confirmed {
        QrStatus::Confirmed(session) => {
            assert_eq!(
                session.cookies.get("sessionid").map(|z| z.as_str()),
                Some("live_cookie")
            );
        }
        other => panic!("unexpected qr status: {other:?}"),
    }
}
