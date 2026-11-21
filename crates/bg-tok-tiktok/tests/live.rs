//! Opt-in live smoke tests against real TikTok endpoints.
//! Never run in public CI; invoke manually with `cargo test -p bg-tok-tiktok --test live -- --ignored --nocapture`.
use bg_tok::{
    media::MediaDelivery,
    network::ByteRange,
    provider::{ContentId, ContentProvider},
};
use bg_tok_tiktok::{models::TIKTOK_NAMESPACE, TikTokConfig, TikTokProvider};
use futures_util::StreamExt;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[tokio::test]
#[ignore = "live network test against TikTok; run manually"]
async fn live_video_metadata_and_media_delivery_stream() {
    let provider = Arc::new(TikTokProvider::new(TikTokConfig::default()).unwrap());
    let id = ContentId {
        namespace: TIKTOK_NAMESPACE.into(),
        value: "6718335390845095173".into(),
    };
    let video = provider
        .resolve_video(&id.value, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(video.id, "6718335390845095173");
    assert_eq!(video.author, "scout2015");
    assert!(!video.sources.is_empty());

    let sources = provider
        .sources(&id, CancellationToken::new())
        .await
        .unwrap();
    let delivery = MediaDelivery::new(provider.clone(), CancellationToken::new());
    let range = ByteRange::new(0, Some(4095)).unwrap();
    let mut stream = delivery
        .open(
            &id,
            sources[0].clone(),
            Some(range),
            CancellationToken::new(),
        )
        .await
        .unwrap();

    let mut total = Vec::new();
    while let Some(chunk) = stream.next().await {
        total.extend_from_slice(&chunk.unwrap());
    }
    assert_eq!(total.len(), 4096);
    assert_eq!(&total[4..8], b"ftyp");
}

#[tokio::test]
#[ignore = "live network test against TikTok; run manually"]
async fn live_explore_pagination_comments_and_oembed() {
    let provider = TikTokProvider::new(TikTokConfig::default()).unwrap();
    let p1 = provider
        .explore_page(120, None, CancellationToken::new())
        .await
        .unwrap();
    assert!(!p1.items.is_empty());

    if let Some(cursor) = p1.next_cursor.as_deref() {
        let p2 = provider
            .explore_page(120, Some(cursor), CancellationToken::new())
            .await
            .unwrap();
        assert!(!p2.items.is_empty());
    }

    let id = ContentId {
        namespace: TIKTOK_NAMESPACE.into(),
        value: "6718335390845095173".into(),
    };
    let comments = provider
        .comments(&id, None, CancellationToken::new())
        .await
        .unwrap();
    assert!(!comments.comments.is_empty());

    let oembed = provider
        .oembed(
            "https://www.tiktok.com/@scout2015/video/6718335390845095173",
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(
        oembed.embed_product_id.as_deref(),
        Some("6718335390845095173")
    );
}
