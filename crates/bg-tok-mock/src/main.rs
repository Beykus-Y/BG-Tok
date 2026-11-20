use bg_tok::{
    feed::Feed,
    media::MediaDelivery,
    provider::{ContentProvider, FeedKind},
};
use bg_tok_mock::MockProvider;
use futures_util::StreamExt;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let provider = Arc::new(MockProvider::default());
    let session = CancellationToken::new();
    let feed = Feed::with_session(
        provider.clone(),
        FeedKind::Recommended,
        100,
        session.clone(),
    );
    while !feed.exhausted().await {
        feed.load_next(CancellationToken::new()).await?;
    }
    for item in feed.snapshot().await {
        println!("{}: {}", item.id.value, item.description);
    }
    let first = feed.get(0).await.ok_or("empty mock feed")?;
    let source = provider
        .sources(&first.id, CancellationToken::new())
        .await?
        .remove(0);
    let mut body = MediaDelivery::new(provider, session)
        .open(&first.id, source, None, CancellationToken::new())
        .await?;
    let mut bytes = 0;
    while let Some(chunk) = body.next().await {
        bytes += chunk?.len();
    }
    println!("Read {bytes} synthetic bytes on request; no playback event sent.");
    Ok(())
}
