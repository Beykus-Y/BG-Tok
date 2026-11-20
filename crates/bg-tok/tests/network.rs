use bg_tok::network::{ByteRange, HttpConfig, HttpTransport, NetworkError};
use futures_util::StreamExt;
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use tokio_util::sync::CancellationToken;

async fn server(responses: Vec<&'static str>) -> (reqwest::Url, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "http://{}/video?secret=not-real",
        listener.local_addr().unwrap()
    )
    .parse()
    .unwrap();
    let task = tokio::spawn(async move {
        for response in responses {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 4096];
            let _ = socket.read(&mut request).await.unwrap();
            socket.write_all(response.as_bytes()).await.unwrap();
        }
    });
    (url, task)
}
fn transport() -> HttpTransport {
    HttpTransport::new(HttpConfig {
        backoff: Duration::from_millis(1),
        max_backoff: Duration::from_millis(5),
        ..Default::default()
    })
    .unwrap()
}
#[tokio::test]
async fn retries_rate_limit_and_reads_range() {
    let (url, task) = server(vec!["HTTP/1.1 429 Too Many Requests\r\nRetry-After: 0\r\nContent-Length: 0\r\nConnection: close\r\n\r\n", "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 2-4/10\r\nContent-Length: 3\r\nConnection: close\r\n\r\ncde"]).await;
    let mut stream = transport()
        .get(
            &url,
            Some(ByteRange::new(2, Some(4)).unwrap()),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(stream.next().await.unwrap().unwrap(), "cde");
    assert!(stream.next().await.is_none());
    task.await.unwrap();
}
#[tokio::test]
async fn rejects_unhonored_range() {
    let (url, task) = server(vec![
        "HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\nabc",
    ])
    .await;
    assert!(matches!(
        transport()
            .get(
                &url,
                Some(ByteRange::new(1, None).unwrap()),
                CancellationToken::new()
            )
            .await,
        Err(NetworkError::InvalidRange)
    ));
    task.await.unwrap();
}
#[tokio::test]
async fn cancellation_interrupts_waiting_body() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap())
        .parse()
        .unwrap();
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = [0; 4096];
        assert!(socket.read(&mut request).await.unwrap() > 0);
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n")
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_secs(10)).await;
    });
    let cancel = CancellationToken::new();
    let mut stream = transport().get(&url, None, cancel.clone()).await.unwrap();
    cancel.cancel();
    assert!(matches!(
        stream.next().await.unwrap(),
        Err(NetworkError::Cancelled)
    ));
    task.abort();
}
#[test]
fn proxy_credentials_are_not_debugged() {
    let config = HttpConfig {
        proxy: Some("http://user:secret@localhost:8080".into()),
        ..Default::default()
    };
    assert!(!format!("{config:?}").contains("secret"));
}

#[tokio::test]
async fn honors_retry_after_beyond_budget_without_retrying_early() {
    let (url, task) = server(vec!["HTTP/1.1 429 Too Many Requests\r\nRetry-After: 100\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"]).await;
    assert!(matches!(
        transport().get(&url, None, CancellationToken::new()).await,
        Err(NetworkError::RateLimited)
    ));
    task.await.unwrap();
}

#[tokio::test]
async fn post_is_not_retried_and_does_not_expose_url() {
    let (url, task) = server(vec![
        "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    ])
    .await;
    let result = transport()
        .request(
            reqwest::Method::POST,
            &url,
            reqwest::header::HeaderMap::new(),
            Some("synthetic".into()),
            None,
            CancellationToken::new(),
        )
        .await;
    match result {
        Err(error) => {
            assert!(matches!(error, NetworkError::Http(503)));
            assert!(!error.to_string().contains("not-real"));
        }
        Ok(_) => panic!("expected error"),
    }
    task.await.unwrap();
}

#[tokio::test]
async fn cancellation_while_waiting_for_capacity() {
    let (url, task) = server(vec![
        "HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\nabc",
    ])
    .await;
    let client = HttpTransport::new(HttpConfig {
        max_in_flight: 1,
        ..Default::default()
    })
    .unwrap();
    let stream = client
        .get(&url, None, CancellationToken::new())
        .await
        .unwrap();
    let cancel = CancellationToken::new();
    let waiting = tokio::spawn({
        let client = client.clone();
        let cancel = cancel.clone();
        async move { client.get(&url, None, cancel).await }
    });
    cancel.cancel();
    assert!(matches!(
        waiting.await.unwrap(),
        Err(NetworkError::Cancelled)
    ));
    drop(stream);
    task.await.unwrap();
}
