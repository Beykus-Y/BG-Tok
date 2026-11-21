//! Test CLI for verifying real TikTok integration via `bg-tok-tiktok`.
use bg_tok::{
    media::MediaDelivery,
    network::{ByteRange, HttpConfig},
    provider::{ContentId, ContentProvider},
};
use bg_tok_tiktok::{models::TIKTOK_NAMESPACE, start_qr_login, TikTokConfig, TikTokProvider};
use futures_util::StreamExt;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut proxy = None;
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--proxy" && i + 1 < args.len() {
            proxy = Some(args.remove(i + 1));
            args.remove(i);
        } else {
            i += 1;
        }
    }

    let Some(cmd) = args.first().map(String::as_str) else {
        print_usage();
        return Ok(());
    };

    let config = TikTokConfig {
        http: HttpConfig {
            proxy,
            ..HttpConfig::default()
        },
        ..TikTokConfig::default()
    };
    let provider = Arc::new(TikTokProvider::new(config.clone())?);
    let cancel = CancellationToken::new();

    match cmd {
        "video" => {
            let target = args.get(1).ok_or("missing <id_or_url>")?;
            let read_bytes = args.iter().any(|a| a == "--bytes");
            let video = provider.resolve_video(target, cancel.clone()).await?;
            println!("ID: {}", video.id);
            println!("Author: @{}", video.author);
            println!("Description: {}", video.description);
            println!("Duration: {:?}", video.duration);
            println!("Views: {:?}", video.views);
            println!("Thumbnail: {:?}", video.thumbnail);
            println!("Sources: {}", video.sources.len());

            let content_id = ContentId {
                namespace: TIKTOK_NAMESPACE.to_string(),
                value: video.id.clone(),
            };
            let sources = provider.sources(&content_id, cancel.clone()).await?;
            if let Some(first) = sources.first() {
                println!(
                    "First source: id={}, mime={}, len={:?}, dims={:?}x{:?}, bitrate={:?}, expires={:?}",
                    first.id,
                    first.mime_type,
                    first.length,
                    first.width,
                    first.height,
                    first.bitrate_bps,
                    first.expires_at
                );
                if read_bytes {
                    let session = CancellationToken::new();
                    let delivery = MediaDelivery::new(provider.clone(), session);
                    let range = ByteRange::new(0, Some(65_535))?;
                    let mut stream = delivery
                        .open(&content_id, first.clone(), Some(range), cancel)
                        .await?;
                    let mut total = 0usize;
                    let mut header_bytes = Vec::new();
                    while let Some(chunk) = stream.next().await {
                        let data = chunk?;
                        if header_bytes.len() < 16 {
                            let take = (16 - header_bytes.len()).min(data.len());
                            header_bytes.extend_from_slice(&data[..take]);
                        }
                        total += data.len();
                    }
                    println!(
                        "Streamed {total} bytes via MediaDelivery (magic={header_bytes:02x?})"
                    );
                }
            }
        }
        "explore" => {
            let category = parse_flag_u32(&args, "--category").unwrap_or(120);
            let pages = parse_flag_u32(&args, "--pages").unwrap_or(2);
            let mut cursor: Option<String> = None;
            for page_idx in 1..=pages {
                let page = provider
                    .explore_page(category, cursor.as_deref(), cancel.clone())
                    .await?;
                println!(
                    "Explore page {page_idx}: {} items, next_cursor={:?}",
                    page.items.len(),
                    page.next_cursor
                );
                for item in &page.items {
                    println!(
                        "  - {} (@{}): {}",
                        item.id.value, item.author, item.description
                    );
                }
                if page.next_cursor.is_none() {
                    break;
                }
                cursor = page.next_cursor;
            }
        }
        "comments" => {
            let target = args.get(1).ok_or("missing <video_id>")?;
            let id = ContentId {
                namespace: TIKTOK_NAMESPACE.to_string(),
                value: target.clone(),
            };
            let page = provider.comments(&id, None, cancel).await?;
            println!(
                "Comments: {} (total={:?}, next_cursor={:?})",
                page.comments.len(),
                page.total,
                page.next_cursor
            );
            for c in page.comments.iter().take(10) {
                println!("  @{} (likes={}): {}", c.author, c.likes, c.text);
            }
        }
        "oembed" => {
            let target = args.get(1).ok_or("missing <url>")?;
            let info = provider.oembed(target, cancel).await?;
            println!("oEmbed: {info:?}");
        }
        "qr-login" => {
            let transport = bg_tok::network::HttpTransport::new(config.http)?;
            let challenge = start_qr_login(&transport, None, cancel).await?;
            println!(
                "QR login ready: png_base64_len={}, index_url={:?}, expire_time={:?}",
                challenge.qrcode_base64_png.len(),
                challenge.qrcode_index_url,
                challenge.expire_time
            );
        }
        _ => print_usage(),
    }
    Ok(())
}

fn parse_flag_u32(args: &[String], flag: &str) -> Option<u32> {
    args.windows(2)
        .find(|w| w[0] == flag)
        .and_then(|w| w[1].parse::<u32>().ok())
}

fn print_usage() {
    eprintln!("Usage:");
    eprintln!("  bg-tok-tiktok video <id_or_url> [--bytes] [--proxy <url>]");
    eprintln!("  bg-tok-tiktok explore [--category <id>] [--pages <n>] [--proxy <url>]");
    eprintln!("  bg-tok-tiktok comments <video_id> [--proxy <url>]");
    eprintln!("  bg-tok-tiktok oembed <url> [--proxy <url>]");
    eprintln!("  bg-tok-tiktok qr-login [--proxy <url>]");
}
