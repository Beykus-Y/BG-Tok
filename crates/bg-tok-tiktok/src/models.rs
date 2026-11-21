//! Typed models for TikTok responses mapped into BG Tok primitives.
use bg_tok::provider::{CachePolicy, ContentId, Metadata, Source};
use std::time::{Duration, SystemTime};

/// Namespace used for all TikTok [`ContentId`] values.
pub const TIKTOK_NAMESPACE: &str = "tiktok";

/// Parsed reference to a TikTok video from an ID or URL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VideoRef {
    /// Numeric video ID (e.g. `6718335390845095173`).
    Id(String),
    /// Full canonical, mobile, or short TikTok URL (e.g. `https://vm.tiktok.com/...`).
    Url(String),
}

/// Internal candidate media stream with upstream URLs kept private inside the provider.
#[derive(Clone)]
pub struct TikTokSourceCandidate {
    /// Stable source identifier within the content item (never a raw URL).
    pub source_id: String,
    pub urls: Vec<String>,
    pub mime_type: String,
    pub length: Option<u64>,
    pub revision: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub bitrate_bps: Option<u64>,
    pub expires_at: Option<SystemTime>,
}

impl std::fmt::Debug for TikTokSourceCandidate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TikTokSourceCandidate")
            .field("source_id", &self.source_id)
            .field("url_count", &self.urls.len())
            .field("mime_type", &self.mime_type)
            .field("length", &self.length)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("bitrate_bps", &self.bitrate_bps)
            .finish()
    }
}

impl TikTokSourceCandidate {
    /// Converts this candidate into a host-facing opaque [`Source`] handle.
    pub fn to_source(&self) -> Source {
        Source {
            id: self.source_id.clone(),
            mime_type: self.mime_type.clone(),
            length: self.length,
            revision: self.revision.clone(),
            width: self.width,
            height: self.height,
            bitrate_bps: self.bitrate_bps,
            expires_at: self.expires_at,
            cache: CachePolicy::HostManaged,
        }
    }
}

/// Full parsed TikTok video metadata and stream candidates.
#[derive(Clone, Debug)]
pub struct TikTokVideo {
    pub id: String,
    pub author: String,
    pub author_nickname: Option<String>,
    pub description: String,
    pub duration: Option<Duration>,
    pub thumbnail: Option<String>,
    pub views: Option<u64>,
    pub likes: Option<u64>,
    pub comments: Option<u64>,
    pub shares: Option<u64>,
    pub created_at: Option<u64>,
    pub sources: Vec<TikTokSourceCandidate>,
}

impl TikTokVideo {
    /// Converts this video into BG Tok [`Metadata`].
    pub fn to_metadata(&self) -> Metadata {
        Metadata {
            id: ContentId {
                namespace: TIKTOK_NAMESPACE.to_string(),
                value: self.id.clone(),
            },
            author: self.author.clone(),
            description: self.description.clone(),
            duration: self.duration,
            thumbnail: self.thumbnail.clone(),
            views: self.views,
        }
    }
}

/// Public comment returned by `/api/comment/list/`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TikTokComment {
    pub id: String,
    pub author: String,
    pub text: String,
    pub likes: u64,
    pub create_time: u64,
}

/// Paginated list of comments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommentPage {
    pub comments: Vec<TikTokComment>,
    pub next_cursor: Option<String>,
    pub total: Option<u64>,
}

/// Official TikTok oEmbed response from `https://www.tiktok.com/oembed`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OEmbedInfo {
    pub title: Option<String>,
    pub author_name: Option<String>,
    pub author_url: Option<String>,
    pub thumbnail_url: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub embed_product_id: Option<String>,
    pub html: Option<String>,
}

/// Official TikTok Display API v2 user profile (`GET /v2/user/info/`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DisplayUserInfo {
    pub open_id: String,
    pub union_id: Option<String>,
    pub display_name: Option<String>,
    pub username: Option<String>,
    pub avatar_url: Option<String>,
    pub is_verified: Option<bool>,
    pub follower_count: Option<u64>,
    pub following_count: Option<u64>,
    pub likes_count: Option<u64>,
    pub video_count: Option<u64>,
}
