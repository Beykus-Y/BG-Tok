//! Real TikTok content provider for BG Tok.
#![forbid(unsafe_code)]

pub mod auth;
#[cfg(feature = "experimental-companion")]
pub mod companion;
pub mod error;
pub mod models;
pub mod parser;
pub mod provider;

pub use auth::{
    build_authorize_url, exchange_code, poll_qr_login, refresh_oauth_token, start_qr_login,
    LoginKitConfig, OAuthTokens, QrChallenge, QrStatus, TikTokSession,
};
pub use models::{
    CommentPage, DisplayUserInfo, OEmbedInfo, TikTokComment, TikTokSourceCandidate, TikTokVideo,
};
pub use provider::{TikTokConfig, TikTokProvider};
