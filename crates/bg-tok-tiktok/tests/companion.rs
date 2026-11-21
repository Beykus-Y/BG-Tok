//! Offline tests for the experimental Android Companion For You adapter.
#![cfg(feature = "experimental-companion")]

use async_trait::async_trait;
use bg_tok::{
    account::AccountId,
    feed::Feed,
    media::MediaDelivery,
    network::{ByteRange, HttpResponse, NetworkError, Transport},
    provider::{ContentProvider, FeedKind, PlaybackEvent, ProviderError},
};
use bg_tok_tiktok::{
    companion::{CompanionBatch, CompanionFeedAdapter, CompanionItem},
    TikTokConfig, TikTokProvider,
};
use bytes::Bytes;
use futures_util::{stream, StreamExt};
use http::{header, HeaderMap, Method};
use std::{sync::Arc, time::Duration};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use url::Url;

struct CompanionMockTransport {
    seen_cookies: Mutex<Vec<Option<String>>>,
}

impl CompanionMockTransport {
    fn new() -> Self {
        Self {
            seen_cookies: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl Transport for CompanionMockTransport {
    async fn send(
        &self,
        _method: Method,
        url: &Url,
        headers: HeaderMap,
        _body: Option<Bytes>,
        _range: Option<ByteRange>,
        cancel: CancellationToken,
    ) -> Result<HttpResponse, NetworkError> {
        if cancel.is_cancelled() {
            return Err(NetworkError::Cancelled);
        }
        let cookie = headers
            .get(header::COOKIE)
            .and_then(|v| v.to_str().ok())
            .map(ToString::to_string);
        self.seen_cookies.lock().await.push(cookie);

        if url.path().starts_with("/@_/video/") {
            let id = url.path().trim_start_matches("/@_/video/");
            let payload = serde_json::json!({
                "__DEFAULT_SCOPE__": {
                    "webapp.video-detail": {
                        "statusCode": 0,
                        "itemInfo": {
                            "itemStruct": {
                                "id": id,
                                "desc": format!("ForYou item {id}"),
                                "author": { "uniqueId": format!("creator_{id}") },
                                "stats": { "playCount": 42 },
                                "video": {
                                    "duration": 10,
                                    "width": 576,
                                    "height": 1024,
                                    "playAddr": format!("https://v16-webapp-prime.tiktok.com/media/{id}?expire=4000000000"),
                                    "bitrateInfo": [{
                                        "GearName": "normal_540_0",
                                        "Bitrate": 1000000,
                                        "PlayAddr": {
                                            "DataSize": "8",
                                            "Width": 576,
                                            "Height": 1024,
                                            "FileHash": format!("hash_{id}"),
                                            "UrlList": [format!("https://v16-webapp-prime.tiktok.com/media/{id}?expire=4000000000")]
                                        }
                                    }]
                                }
                            }
                        }
                    }
                }
            });
            let html = format!(
                "<html><script id=\"__UNIVERSAL_DATA_FOR_REHYDRATION__\" type=\"application/json\">{payload}</script></html>"
            );
            return Ok(HttpResponse {
                status: 200,
                headers: HeaderMap::new(),
                body: Box::pin(stream::iter(vec![Ok(Bytes::from(html))])),
            });
        }

        Ok(HttpResponse {
            status: 206,
            headers: HeaderMap::new(),
            body: Box::pin(stream::iter(vec![Ok(Bytes::from_static(b"ftypmp42"))])),
        })
    }
}

#[tokio::test]
async fn preserves_order_deduplicates_and_streams_without_session_transfer() {
    let transport = Arc::new(CompanionMockTransport::new());
    let inner = Arc::new(TikTokProvider::with_transport(
        transport.clone(),
        TikTokConfig {
            min_request_interval: Duration::ZERO,
            ..TikTokConfig::default()
        },
    ));
    let adapter = Arc::new(CompanionFeedAdapter::new(inner, 2));
    let acc = AccountId("test_user_1".into());

    // Batch 1: out-of-order positions + duplicate ID 1001.
    let added1 = adapter
        .push_batch(CompanionBatch {
            account_id: acc.clone(),
            batch_seq: 1,
            has_more: true,
            items: vec![
                CompanionItem {
                    video_id: "1002".into(),
                    position: 1,
                    observed_at_ms: 100,
                },
                CompanionItem {
                    video_id: "1001".into(),
                    position: 0,
                    observed_at_ms: 100,
                },
                CompanionItem {
                    video_id: "1001".into(),
                    position: 2,
                    observed_at_ms: 101,
                },
            ],
        })
        .await
        .unwrap();
    assert_eq!(added1, 2);

    let feed = Feed::new(adapter.clone(), FeedKind::Recommended, 50);
    assert_eq!(feed.load_next(CancellationToken::new()).await.unwrap(), 2);

    // Batch 2 arrives on scroll.
    let added2 = adapter
        .push_batch(CompanionBatch {
            account_id: acc,
            batch_seq: 2,
            has_more: false,
            items: vec![CompanionItem {
                video_id: "1003".into(),
                position: 0,
                observed_at_ms: 200,
            }],
        })
        .await
        .unwrap();
    assert_eq!(added2, 1);
    assert_eq!(feed.load_next(CancellationToken::new()).await.unwrap(), 1);
    assert!(feed.exhausted().await);

    let snap = feed.snapshot().await;
    let ids: Vec<&str> = snap.iter().map(|m| m.id.value.as_str()).collect();
    assert_eq!(ids, vec!["1001", "1002", "1003"]);

    // Resolve and stream first item through MediaDelivery.
    let sources = adapter
        .sources(&snap[0].id, CancellationToken::new())
        .await
        .unwrap();
    let delivery = MediaDelivery::new(adapter.clone(), CancellationToken::new());
    let mut body = delivery
        .open(
            &snap[0].id,
            sources[0].clone(),
            Some(ByteRange::new(0, Some(7)).unwrap()),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let mut bytes = Vec::new();
    while let Some(chunk) = body.next().await {
        bytes.extend_from_slice(&chunk.unwrap());
    }
    assert_eq!(bytes, b"ftypmp42");

    // Verify no cookies/session were transferred.
    let cookies = transport.seen_cookies.lock().await;
    assert!(cookies.iter().all(Option::is_none));
}

#[tokio::test]
async fn account_switch_and_logout_isolate_state_and_reject_unsupported() {
    let transport = Arc::new(CompanionMockTransport::new());
    let inner = Arc::new(TikTokProvider::with_transport(
        transport,
        TikTokConfig {
            min_request_interval: Duration::ZERO,
            ..TikTokConfig::default()
        },
    ));
    let adapter = CompanionFeedAdapter::new(inner, 5);

    assert_eq!(
        adapter
            .page(&FeedKind::Recommended, None, CancellationToken::new())
            .await
            .unwrap_err(),
        ProviderError::Unauthorized
    );
    assert_eq!(
        adapter
            .page(
                &FeedKind::Search("cats".into()),
                None,
                CancellationToken::new()
            )
            .await
            .unwrap_err(),
        ProviderError::Unsupported
    );

    adapter
        .push_batch(CompanionBatch {
            account_id: AccountId("u1".into()),
            batch_seq: 1,
            has_more: false,
            items: vec![CompanionItem {
                video_id: "2001".into(),
                position: 0,
                observed_at_ms: 10,
            }],
        })
        .await
        .unwrap();

    // Switching account clears previous user's buffered IDs.
    adapter.switch_account(AccountId("u2".into())).await;
    let p = adapter
        .page(&FeedKind::Recommended, None, CancellationToken::new())
        .await
        .unwrap();
    assert!(p.items.is_empty());

    // Logout resets active account.
    adapter.logout().await.unwrap();
    assert_eq!(
        adapter
            .page(&FeedKind::Recommended, None, CancellationToken::new())
            .await
            .unwrap_err(),
        ProviderError::Unauthorized
    );
    assert_eq!(
        adapter
            .playback(
                &bg_tok_tiktok::companion::companion_content_id("2001"),
                PlaybackEvent::Started,
                CancellationToken::new(),
            )
            .await
            .unwrap_err(),
        ProviderError::Unsupported
    );
}
