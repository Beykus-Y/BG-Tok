use async_trait::async_trait;
use bg_tok::{
    account::{AccountId, Accounts, Secret, SecureStore},
    feed::Feed,
    media::MediaDelivery,
    network::ByteRange,
    provider::{
        Action, Capabilities, ContentId, ContentProvider, FeedKind, MediaStream, Page,
        PlaybackEvent, ProviderError, Source,
    },
};
use bg_tok_mock::{id, MockProvider};
use futures_util::StreamExt;
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, SystemTime},
};
use tokio::sync::{Mutex, Notify};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn pagination_deduplicates_and_concurrent_loads_are_serialized() {
    let provider = Arc::new(MockProvider::default());
    let feed = Arc::new(Feed::new(provider.clone(), FeedKind::Recommended, 10));
    let (a, b) = tokio::join!(
        feed.load_next(CancellationToken::new()),
        feed.load_next(CancellationToken::new())
    );
    assert_eq!(a.unwrap() + b.unwrap(), 3);
    assert_eq!(
        feed.snapshot()
            .await
            .into_iter()
            .map(|i| i.id.value)
            .collect::<Vec<_>>(),
        ["one", "two", "three"]
    );
    assert!(feed.exhausted().await);
    assert_eq!(feed.load_next(CancellationToken::new()).await.unwrap(), 0);
    assert_eq!(provider.page_calls(), 2);
    assert_eq!(feed.get(2).await.unwrap().id, id("three"));
}
#[tokio::test]
async fn capacity_failure_does_not_advance_cursor() {
    let provider = Arc::new(MockProvider::default());
    let feed = Feed::new(provider.clone(), FeedKind::Recommended, 2);
    assert_eq!(feed.load_next(CancellationToken::new()).await.unwrap(), 2);
    assert_eq!(
        feed.load_next(CancellationToken::new()).await,
        Err(ProviderError::Capacity)
    );
    assert_eq!(feed.snapshot().await.len(), 2);
    assert!(!feed.exhausted().await);
    assert_eq!(
        feed.load_next(CancellationToken::new()).await,
        Err(ProviderError::Capacity)
    );
    assert_eq!(provider.page_calls(), 3);
}
#[tokio::test]
async fn expired_source_is_refreshed_and_range_delivered_without_playback() {
    let provider = Arc::new(MockProvider::default());
    let mut source = provider
        .sources(&id("one"), CancellationToken::new())
        .await
        .unwrap()
        .remove(0);
    source.expires_at = Some(SystemTime::UNIX_EPOCH);
    let delivery = MediaDelivery::new(provider.clone(), CancellationToken::new());
    let mut stream = delivery
        .open(
            &id("one"),
            source,
            Some(ByteRange::new(0, Some(3)).unwrap()),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(stream.next().await.unwrap().unwrap(), "synt");
    assert!(provider.events().await.is_empty());
    provider
        .playback(&id("one"), PlaybackEvent::Started, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(provider.events().await.len(), 1);
}
#[derive(Default)]
struct MemoryStore(Mutex<HashMap<String, Vec<u8>>>);
#[async_trait]
impl SecureStore for MemoryStore {
    async fn save(&self, id: &AccountId, secret: &Secret) -> Result<(), ProviderError> {
        self.0
            .lock()
            .await
            .insert(id.0.clone(), secret.expose().to_vec());
        Ok(())
    }
    async fn load(&self, id: &AccountId) -> Result<Option<Secret>, ProviderError> {
        Ok(self.0.lock().await.get(&id.0).cloned().map(Secret::new))
    }
    async fn delete(&self, id: &AccountId) -> Result<(), ProviderError> {
        self.0.lock().await.remove(&id.0);
        Ok(())
    }
}
#[tokio::test]
async fn switch_and_logout_cancel_old_work_and_isolate_credentials() {
    let accounts = Accounts::new(Arc::new(MemoryStore::default()));
    let a = AccountId("synthetic-a".into());
    let b = AccountId("synthetic-b".into());
    accounts.save(&a, &Secret::new(vec![1])).await.unwrap();
    accounts.save(&b, &Secret::new(vec![2])).await.unwrap();
    let active_a = accounts.activate(&a).await.unwrap();
    let feed = Feed::with_session(
        Arc::new(MockProvider::default()),
        FeedKind::Recommended,
        10,
        active_a.cancellation.clone(),
    );
    feed.load_next(CancellationToken::new()).await.unwrap();
    let active_b = accounts.activate(&b).await.unwrap();
    assert!(active_a.cancellation.is_cancelled());
    assert!(feed.snapshot().await.is_empty());
    assert!(feed.get(0).await.is_none());
    assert_eq!(
        feed.load_next(CancellationToken::new()).await,
        Err(ProviderError::SessionClosed)
    );
    accounts.logout(&b).await.unwrap();
    assert!(active_b.cancellation.is_cancelled());
    assert!(accounts.active().await.is_none());
    assert!(matches!(
        accounts.activate(&b).await,
        Err(ProviderError::Unauthorized)
    ));
    assert!(accounts.activate(&a).await.is_ok());
}
struct DelayedProvider {
    started: Notify,
    calls: AtomicUsize,
    read_token: Mutex<Option<CancellationToken>>,
}
#[async_trait]
impl ContentProvider for DelayedProvider {
    fn capabilities(&self) -> Capabilities {
        Capabilities::default()
    }
    async fn page(
        &self,
        _: &FeedKind,
        _: Option<&str>,
        _: CancellationToken,
    ) -> Result<Page, ProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.started.notify_one();
        tokio::time::sleep(Duration::from_secs(20)).await;
        Ok(Page {
            items: vec![],
            next_cursor: None,
        })
    }
    async fn sources(
        &self,
        _: &ContentId,
        _: CancellationToken,
    ) -> Result<Vec<Source>, ProviderError> {
        Err(ProviderError::Unsupported)
    }
    async fn read(
        &self,
        _: &Source,
        _: Option<ByteRange>,
        cancel: CancellationToken,
    ) -> Result<MediaStream, ProviderError> {
        *self.read_token.lock().await = Some(cancel);
        Ok(Box::pin(futures_util::stream::pending()))
    }
    async fn action(
        &self,
        _: &ContentId,
        _: Action,
        _: CancellationToken,
    ) -> Result<(), ProviderError> {
        Err(ProviderError::Unsupported)
    }
    async fn playback(
        &self,
        _: &ContentId,
        _: PlaybackEvent,
        _: CancellationToken,
    ) -> Result<(), ProviderError> {
        Err(ProviderError::Unsupported)
    }
    async fn logout(&self) -> Result<(), ProviderError> {
        Ok(())
    }
}
#[tokio::test]
async fn close_interrupts_pending_provider_and_does_not_commit_page() {
    let provider = Arc::new(DelayedProvider {
        started: Notify::new(),
        calls: AtomicUsize::new(0),
        read_token: Mutex::new(None),
    });
    let feed = Arc::new(Feed::new(provider.clone(), FeedKind::Recommended, 10));
    let handle = tokio::spawn({
        let feed = feed.clone();
        async move { feed.load_next(CancellationToken::new()).await }
    });
    provider.started.notified().await;
    feed.close();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), handle)
            .await
            .unwrap()
            .unwrap(),
        Err(ProviderError::SessionClosed)
    );
    assert!(feed.snapshot().await.is_empty());
}

#[tokio::test]
async fn rejects_chunk_larger_than_host_budget() {
    let provider = Arc::new(MockProvider::default());
    let source = provider
        .sources(&id("one"), CancellationToken::new())
        .await
        .unwrap()
        .remove(0);
    let delivery = MediaDelivery::with_chunk_limit(provider, CancellationToken::new(), 4);
    let mut stream = delivery
        .open(&id("one"), source, None, CancellationToken::new())
        .await
        .unwrap();
    assert!(matches!(
        stream.next().await,
        Some(Err(ProviderError::Capacity))
    ));
    assert!(stream.next().await.is_none());
}

#[tokio::test]
async fn dropping_media_stream_cancels_provider_work() {
    let provider = Arc::new(DelayedProvider {
        started: Notify::new(),
        calls: AtomicUsize::new(0),
        read_token: Mutex::new(None),
    });
    let source = Source {
        id: "synthetic".into(),
        mime_type: "application/octet-stream".into(),
        length: None,
        revision: None,
        width: None,
        height: None,
        bitrate_bps: None,
        expires_at: None,
        cache: bg_tok::provider::CachePolicy::NoStore,
    };
    let delivery = MediaDelivery::new(provider.clone(), CancellationToken::new());
    let stream = delivery
        .open(&id("one"), source, None, CancellationToken::new())
        .await
        .unwrap();
    let token = provider.read_token.lock().await.clone().unwrap();
    assert!(!token.is_cancelled());
    drop(stream);
    assert!(token.is_cancelled());
}

#[tokio::test]
async fn transient_page_failure_can_be_retried_without_skipping_items() {
    let provider = Arc::new(MockProvider::default());
    let feed = Feed::new(provider.clone(), FeedKind::Recommended, 10);
    provider.fail_next_page(ProviderError::RateLimited).await;
    assert_eq!(
        feed.load_next(CancellationToken::new()).await,
        Err(ProviderError::RateLimited)
    );
    assert!(feed.snapshot().await.is_empty());
    assert_eq!(feed.load_next(CancellationToken::new()).await.unwrap(), 2);
    assert_eq!(feed.get(0).await.unwrap().id, id("one"));
}

#[tokio::test]
async fn cyclic_cursor_is_rejected_without_advancing() {
    let provider = Arc::new(MockProvider::default());
    provider.set_cursor_cycle(true);
    let feed = Feed::new(provider, FeedKind::Recommended, 10);
    assert_eq!(feed.load_next(CancellationToken::new()).await.unwrap(), 0);
    assert_eq!(
        feed.load_next(CancellationToken::new()).await,
        Err(ProviderError::InvalidResponse)
    );
    assert!(!feed.exhausted().await);
}
