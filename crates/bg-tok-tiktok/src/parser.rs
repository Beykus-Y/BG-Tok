//! Parsers for TikTok SSR HTML, Explore JSON, Comments JSON, oEmbed, and Display API v2.
use crate::models::{
    CommentPage, DisplayUserInfo, OEmbedInfo, TikTokComment, TikTokSourceCandidate, TikTokVideo,
    VideoRef, TIKTOK_NAMESPACE,
};
use bg_tok::provider::{ContentId, Metadata, Page, ProviderError};
use serde_json::Value;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use url::Url;

const UNIVERSAL_DATA_MARKER: &str = "id=\"__UNIVERSAL_DATA_FOR_REHYDRATION__\"";

/// Parses a TikTok video ID or URL into a [`VideoRef`].
pub fn parse_video_ref(input: &str) -> Result<VideoRef, ProviderError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(ProviderError::InvalidResponse);
    }
    if trimmed.bytes().all(|b| b.is_ascii_digit()) {
        return Ok(VideoRef::Id(trimmed.to_string()));
    }
    let url = Url::parse(trimmed).map_err(|_| ProviderError::InvalidResponse)?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(ProviderError::InvalidResponse);
    }
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    if !is_tiktok_host(&host) {
        return Err(ProviderError::InvalidResponse);
    }

    if let Some(id) = extract_video_id_from_path(&url) {
        return Ok(VideoRef::Id(id));
    }

    // Short links (vm.tiktok.com, vt.tiktok.com, tiktok.com/t/...) must be fetched to resolve redirects.
    Ok(VideoRef::Url(url.to_string()))
}

fn is_tiktok_host(host: &str) -> bool {
    host == "tiktok.com" || host.ends_with(".tiktok.com")
}

fn extract_video_id_from_path(url: &Url) -> Option<String> {
    let segments: Vec<&str> = url.path_segments()?.filter(|s| !s.is_empty()).collect();
    // /@user/video/1234567890
    if let Some(pos) = segments.iter().position(|&s| s == "video") {
        if let Some(id) = segments.get(pos + 1) {
            if !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()) {
                return Some((*id).to_string());
            }
        }
    }
    // /v/1234567890.html
    if segments.first().copied() == Some("v") {
        if let Some(raw) = segments.get(1) {
            let id = raw.strip_suffix(".html").unwrap_or(raw);
            if !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()) {
                return Some(id.to_string());
            }
        }
    }
    None
}

/// Extracts `__UNIVERSAL_DATA_FOR_REHYDRATION__` JSON from a TikTok HTML page.
pub fn extract_universal_data(html: &str) -> Result<Value, ProviderError> {
    let marker_idx = html
        .find(UNIVERSAL_DATA_MARKER)
        .ok_or(ProviderError::InvalidResponse)?;
    let after_marker = &html[marker_idx + UNIVERSAL_DATA_MARKER.len()..];
    let gt_idx = after_marker
        .find('>')
        .ok_or(ProviderError::InvalidResponse)?;
    let json_start = &after_marker[gt_idx + 1..];
    let end_idx = json_start
        .find("</script>")
        .ok_or(ProviderError::InvalidResponse)?;
    let json_slice = json_start[..end_idx].trim();
    serde_json::from_str(json_slice).map_err(|_| ProviderError::InvalidResponse)
}

/// Parses a TikTok video detail HTML page into a [`TikTokVideo`].
pub fn parse_video_detail_html(html: &str) -> Result<TikTokVideo, ProviderError> {
    let root = extract_universal_data(html)?;
    let detail = root
        .get("__DEFAULT_SCOPE__")
        .and_then(|s| s.get("webapp.video-detail"))
        .ok_or(ProviderError::InvalidResponse)?;

    let status_code = detail
        .get("statusCode")
        .and_then(Value::as_i64)
        .unwrap_or(-1);
    if status_code != 0 {
        return Err(match status_code {
            10204 | 10216 | 10222 | 10221 => ProviderError::NotFound,
            10202 => ProviderError::Unauthorized,
            _ => ProviderError::InvalidResponse,
        });
    }

    let item = detail
        .get("itemInfo")
        .and_then(|info| info.get("itemStruct"))
        .ok_or(ProviderError::NotFound)?;

    parse_item_struct(item)
}

/// Parses a TikTok `itemStruct` JSON object (shared between video detail and explore feeds).
pub fn parse_item_struct(item: &Value) -> Result<TikTokVideo, ProviderError> {
    let id = item
        .get("id")
        .and_then(value_as_string)
        .filter(|s| !s.is_empty())
        .ok_or(ProviderError::InvalidResponse)?;

    let author_obj = item.get("author");
    let author = author_obj
        .and_then(|a| a.get("uniqueId"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .or_else(|| {
            author_obj
                .and_then(|a| a.get("nickname"))
                .and_then(Value::as_str)
        })
        .unwrap_or("unknown")
        .to_string();
    let author_nickname = author_obj
        .and_then(|a| a.get("nickname"))
        .and_then(Value::as_str)
        .map(ToString::to_string);

    let description = item
        .get("desc")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    let video = item.get("video");
    let duration = video
        .and_then(|v| v.get("duration"))
        .and_then(value_as_u64)
        .filter(|&d| d > 0)
        .map(Duration::from_secs);

    let thumbnail = video
        .and_then(|v| {
            v.get("cover")
                .or_else(|| v.get("originCover"))
                .or_else(|| v.get("dynamicCover"))
        })
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string);

    let stats = item.get("stats");
    let views = stats
        .and_then(|s| s.get("playCount"))
        .and_then(value_as_u64);
    let likes = stats
        .and_then(|s| s.get("diggCount"))
        .and_then(value_as_u64);
    let comments = stats
        .and_then(|s| s.get("commentCount"))
        .and_then(value_as_u64);
    let shares = stats
        .and_then(|s| s.get("shareCount"))
        .and_then(value_as_u64);
    let created_at = item.get("createTime").and_then(value_as_u64);

    let sources = video
        .map(|v| parse_video_sources(&id, v))
        .unwrap_or_default();

    Ok(TikTokVideo {
        id,
        author,
        author_nickname,
        description,
        duration,
        thumbnail,
        views,
        likes,
        comments,
        shares,
        created_at,
        sources,
    })
}

fn parse_video_sources(video_id: &str, video: &Value) -> Vec<TikTokSourceCandidate> {
    let mut candidates = Vec::new();
    let default_width = video.get("width").and_then(value_as_u64).map(|v| v as u32);
    let default_height = video.get("height").and_then(value_as_u64).map(|v| v as u32);

    if let Some(bitrates) = video.get("bitrateInfo").and_then(Value::as_array) {
        for (idx, entry) in bitrates.iter().enumerate() {
            let Some(play_addr) = entry.get("PlayAddr") else {
                continue;
            };
            let urls: Vec<String> = play_addr
                .get("UrlList")
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(Value::as_str)
                        .filter(|u| u.starts_with("http"))
                        .map(ToString::to_string)
                        .collect()
                })
                .unwrap_or_default();
            if urls.is_empty() {
                continue;
            }
            let gear = entry
                .get("GearName")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(ToString::to_string)
                .unwrap_or_else(|| format!("stream_{idx}"));
            let source_id = format!("{video_id}:{gear}");
            if candidates
                .iter()
                .any(|c: &TikTokSourceCandidate| c.source_id == source_id)
            {
                continue;
            }
            let expires_at = urls.iter().find_map(|u| extract_url_expiry(u));
            candidates.push(TikTokSourceCandidate {
                source_id,
                urls,
                mime_type: "video/mp4".to_string(),
                length: play_addr.get("DataSize").and_then(value_as_u64),
                revision: play_addr
                    .get("FileHash")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(ToString::to_string),
                width: play_addr
                    .get("Width")
                    .and_then(value_as_u64)
                    .map(|v| v as u32)
                    .or(default_width),
                height: play_addr
                    .get("Height")
                    .and_then(value_as_u64)
                    .map(|v| v as u32)
                    .or(default_height),
                bitrate_bps: entry.get("Bitrate").and_then(value_as_u64),
                expires_at,
            });
        }
    }

    if candidates.is_empty() {
        if let Some(play_addr) = video
            .get("playAddr")
            .and_then(Value::as_str)
            .filter(|u| u.starts_with("http"))
        {
            candidates.push(TikTokSourceCandidate {
                source_id: format!("{video_id}:default"),
                urls: vec![play_addr.to_string()],
                mime_type: "video/mp4".to_string(),
                length: None,
                revision: None,
                width: default_width,
                height: default_height,
                bitrate_bps: None,
                expires_at: extract_url_expiry(play_addr),
            });
        }
    }

    candidates
}

/// Extracts `expire=<unix_seconds>` or `x-expires=<unix_seconds>` from a signed CDN URL.
pub fn extract_url_expiry(raw_url: &str) -> Option<SystemTime> {
    let url = Url::parse(raw_url).ok()?;
    for (k, v) in url.query_pairs() {
        if matches!(k.as_ref(), "expire" | "x-expires") {
            if let Ok(secs) = v.parse::<u64>() {
                return Some(UNIX_EPOCH + Duration::from_secs(secs));
            }
        }
    }
    None
}

/// Parses `/api/explore/item_list/` JSON into a BG Tok [`Page`].
pub fn parse_explore_response(
    json: &str,
    current_cursor: Option<&str>,
) -> Result<Page, ProviderError> {
    let root: Value = serde_json::from_str(json).map_err(|_| ProviderError::InvalidResponse)?;
    let status = root
        .get("status_code")
        .or_else(|| root.get("statusCode"))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    if status != 0 {
        return Err(ProviderError::InvalidResponse);
    }

    let list = root
        .get("itemList")
        .and_then(Value::as_array)
        .ok_or(ProviderError::InvalidResponse)?;

    let mut items = Vec::with_capacity(list.len());
    for entry in list {
        if let Ok(video) = parse_item_struct(entry) {
            items.push(video.to_metadata());
        }
    }

    let has_more = root
        .get("hasMore")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let next_cursor = if has_more && !items.is_empty() {
        let current_offset = current_cursor
            .and_then(|c| c.parse::<usize>().ok())
            .unwrap_or(0);
        let server_cursor = root
            .get("cursor")
            .and_then(value_as_string)
            .filter(|c| !c.is_empty() && Some(c.as_str()) != current_cursor);
        Some(server_cursor.unwrap_or_else(|| (current_offset + items.len()).to_string()))
    } else {
        None
    };

    Ok(Page { items, next_cursor })
}

/// Parses `/api/comment/list/` JSON into a [`CommentPage`].
pub fn parse_comments_response(json: &str) -> Result<CommentPage, ProviderError> {
    let root: Value = serde_json::from_str(json).map_err(|_| ProviderError::InvalidResponse)?;
    if let Some(status) = root.get("status_code").and_then(Value::as_i64) {
        if status != 0 {
            return Err(ProviderError::InvalidResponse);
        }
    }

    let raw_comments = root
        .get("comments")
        .and_then(Value::as_array)
        .ok_or(ProviderError::InvalidResponse)?;

    let mut comments = Vec::with_capacity(raw_comments.len());
    for c in raw_comments {
        let Some(id) = c.get("cid").and_then(value_as_string) else {
            continue;
        };
        let author = c
            .get("user")
            .and_then(|u| u.get("unique_id").or_else(|| u.get("nickname")))
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        let text = c
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let likes = c.get("digg_count").and_then(value_as_u64).unwrap_or(0);
        let create_time = c.get("create_time").and_then(value_as_u64).unwrap_or(0);
        comments.push(TikTokComment {
            id,
            author,
            text,
            likes,
            create_time,
        });
    }

    let has_more = root
        .get("has_more")
        .and_then(|v| v.as_bool().or_else(|| v.as_i64().map(|n| n != 0)))
        .unwrap_or(false);
    let next_cursor = if has_more {
        root.get("cursor").and_then(value_as_string)
    } else {
        None
    };
    let total = root.get("total").and_then(value_as_u64);

    Ok(CommentPage {
        comments,
        next_cursor,
        total,
    })
}

/// Parses `https://www.tiktok.com/oembed` JSON into [`OEmbedInfo`].
pub fn parse_oembed_response(json: &str) -> Result<OEmbedInfo, ProviderError> {
    let root: Value = serde_json::from_str(json).map_err(|_| ProviderError::InvalidResponse)?;
    Ok(OEmbedInfo {
        title: root
            .get("title")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        author_name: root
            .get("author_name")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        author_url: root
            .get("author_url")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        thumbnail_url: root
            .get("thumbnail_url")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        width: root.get("width").and_then(value_as_u64).map(|v| v as u32),
        height: root.get("height").and_then(value_as_u64).map(|v| v as u32),
        embed_product_id: root
            .get("embed_product_id")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        html: root
            .get("html")
            .and_then(Value::as_str)
            .map(ToString::to_string),
    })
}

/// Parses official Display API v2 `GET /v2/user/info/` JSON.
pub fn parse_display_user_info_response(json: &str) -> Result<DisplayUserInfo, ProviderError> {
    let root: Value = serde_json::from_str(json).map_err(|_| ProviderError::InvalidResponse)?;
    check_display_api_error(&root)?;
    let user = root
        .get("data")
        .and_then(|d| d.get("user"))
        .ok_or(ProviderError::InvalidResponse)?;
    let open_id = user
        .get("open_id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or(ProviderError::InvalidResponse)?
        .to_string();

    Ok(DisplayUserInfo {
        open_id,
        union_id: user
            .get("union_id")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        display_name: user
            .get("display_name")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        username: user
            .get("username")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        avatar_url: user
            .get("avatar_url")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        is_verified: user.get("is_verified").and_then(Value::as_bool),
        follower_count: user.get("follower_count").and_then(value_as_u64),
        following_count: user.get("following_count").and_then(value_as_u64),
        likes_count: user.get("likes_count").and_then(value_as_u64),
        video_count: user.get("video_count").and_then(value_as_u64),
    })
}

/// Parses official Display API v2 `POST /v2/video/list/` JSON into a BG Tok [`Page`].
pub fn parse_display_video_list_response(json: &str) -> Result<Page, ProviderError> {
    let root: Value = serde_json::from_str(json).map_err(|_| ProviderError::InvalidResponse)?;
    check_display_api_error(&root)?;
    let data = root.get("data").ok_or(ProviderError::InvalidResponse)?;
    let videos = data
        .get("videos")
        .and_then(Value::as_array)
        .ok_or(ProviderError::InvalidResponse)?;

    let mut items = Vec::with_capacity(videos.len());
    for v in videos {
        let Some(id) = v.get("id").and_then(value_as_string) else {
            continue;
        };
        let description = v
            .get("video_description")
            .or_else(|| v.get("title"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let duration = v
            .get("duration")
            .and_then(value_as_u64)
            .filter(|&d| d > 0)
            .map(Duration::from_secs);
        let thumbnail = v
            .get("cover_image_url")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        let views = v.get("view_count").and_then(value_as_u64);

        items.push(Metadata {
            id: ContentId {
                namespace: TIKTOK_NAMESPACE.to_string(),
                value: id,
            },
            author: String::new(),
            description,
            duration,
            thumbnail,
            views,
        });
    }

    let has_more = data
        .get("has_more")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let next_cursor = if has_more {
        data.get("cursor").and_then(value_as_string)
    } else {
        None
    };

    Ok(Page { items, next_cursor })
}

fn check_display_api_error(root: &Value) -> Result<(), ProviderError> {
    let Some(err) = root.get("error") else {
        return Ok(());
    };
    let code = err.get("code").and_then(Value::as_str).unwrap_or("ok");
    match code {
        "ok" | "" => Ok(()),
        "access_token_invalid" | "scope_not_authorized" | "invalid_token" => {
            Err(ProviderError::Unauthorized)
        }
        "rate_limit_exceeded" => Err(ProviderError::RateLimited),
        _ => Err(ProviderError::InvalidResponse),
    }
}

fn value_as_u64(v: &Value) -> Option<u64> {
    v.as_u64()
        .or_else(|| v.as_i64().and_then(|n| u64::try_from(n).ok()))
        .or_else(|| v.as_str().and_then(|s| s.parse::<u64>().ok()))
}

fn value_as_string(v: &Value) -> Option<String> {
    v.as_str()
        .map(ToString::to_string)
        .or_else(|| v.as_u64().map(|n| n.to_string()))
        .or_else(|| v.as_i64().map(|n| n.to_string()))
}
