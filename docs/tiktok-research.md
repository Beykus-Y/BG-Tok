# TikTok Integration Research & Live Verification (2026-11-21)

This document records empirical verification results against live TikTok endpoints and official documentation for `bg-tok-tiktok`. No CAPTCHA, DRM, or access control was bypassed, and no secrets or user cookies are stored.

## 1. Verified Working Capabilities

| Capability | Source / Endpoint | Auth Required | Real Data | Required Params | Pagination | Restrictions / Limits | Stability | BG Media Playback |
| :--- | :--- | :--- | :--- | :--- | :--- | :--- | :--- | :--- |
| **Video Metadata & Streams by ID/URL** | Public SSR HTML `https://www.tiktok.com/@_/video/<id>` (`__UNIVERSAL_DATA_FOR_REHYDRATION__` -> `webapp.video-detail.itemInfo.itemStruct`) | No | Yes | Video ID or canonical/short URL | N/A | Rate-limited if burst-polled; signed CDN `playAddr` / `bitrateInfo` URLs expire (`expire=` query param, ~48h) | Medium-High (standard SSR hydration schema) | **Yes** (`206 Partial Content`, `Content-Range`, `video/mp4`, verified `ftypisom` magic bytes when sent with session `tt_chain_token` cookie + `Referer: https://www.tiktok.com/`) |
| **Public Explore Feed** | Web JSON API `https://www.tiktok.com/api/explore/item_list/` | No (session `ttwid`/`msToken` cookie primed via `/explore`) | Yes | `aid=1988`, `categoryType` (e.g. `120`), `count` | Yes (`cursor` offset) | **Public Explore only — NOT personalized For You.** | Medium | Yes (items contain `id` and `video` streams) |
| **Public Video Comments** | Web JSON API `https://www.tiktok.com/api/comment/list/` | No (session cookie) | Yes | `aid=1988`, `aweme_id`, `count`, `cursor` | Yes (`cursor`, `has_more`) | Read-only without user session | Medium | N/A |
| **Official oEmbed** | Official API `https://www.tiktok.com/oembed` | No | Yes | `url` | N/A | Metadata + embed HTML only; no raw MP4 stream URLs | High (Official) | No (embed iframe only) |
| **Login Kit v2 (OAuth 2.0)** | Official `https://www.tiktok.com/v2/auth/authorize/` + `https://open.tiktokapis.com/v2/oauth/token/` | User OAuth | Endpoint live (requires registered `client_key`/`client_secret`) | `client_key`, `redirect_uri`, `scope`, `code`, PKCE | N/A | Grants Display/Share API scopes only; does **not** grant For You feed | High (Official) | No |
| **Display API v2 (Profile & Own Videos)** | Official `https://open.tiktokapis.com/v2/user/info/` & `POST /v2/video/list/` | OAuth Bearer token (`user.info.basic`, `video.list`) | Endpoint live (`401` without token) | `fields`, `max_count` (<=20), `cursor` | Yes (`cursor`, `has_more`) | Authorized user's **own** public posts only; no For You, no raw MP4 stream URLs | High (Official) | No (`embed_link` only) |
| **Web QR Login** | `https://www.tiktok.com/passport/web/get_qrcode/` + `/passport/web/check_qrconnect/` | User scans QR in TikTok app | Yes (returns PNG QR + `token` + `qrcode_index_url`) | `aid=1459`, `next` | N/A | Requires explicit user scan in mobile TikTok app; no password collection | Medium | N/A |

## 2. Investigated & Unavailable Capabilities (Without JS Signing / Private App APIs)

| Capability | Tested Endpoint | Observed Live Result | Exact Reason Unavailable |
| :--- | :--- | :--- | :--- |
| **Personalized For You Feed** | `https://www.tiktok.com/api/recommend/item_list/?aid=1988&count=5` and `https://www.tiktok.com/foryou` SSR | API returns `{"status_code": 0}` (empty); SSR `webapp.foryou.video` returns 1 anonymous seed item only | Web XHR `recommend/item_list` requires client-side obfuscated JS request signatures (`X-Bogus` / `X-Gnarly`) and an authenticated user session. Official Developer APIs do not expose For You. |
| **Search** | `https://www.tiktok.com/api/search/general/full/?aid=1988&keyword=cat` | `HTTP 200`, empty body (`0` bytes) | Requires `X-Bogus` / `X-Gnarly` JS request signature. |
| **User Profile / User Video List (Web)** | `https://www.tiktok.com/api/user/detail/` & `/@tiktok` SSR `itemList` | API returns `0` bytes; SSR `webapp.user-detail.itemList` is empty (`0` items) | Web XHR user feed requires `X-Bogus` / `X-Gnarly` JS request signature. Official alternative for consenting user's own videos is Display API v2 `/v2/video/list/`. |
| **Related Videos** | `https://www.tiktok.com/api/related/item_list/` | `HTTP 200`, empty body (`0` bytes) | Requires `X-Bogus` / `X-Gnarly` JS request signature. |
| **Likes / Follow / Comment Post / Playback View Events** | N/A | Not implemented | Requires authenticated user session + `X-Bogus` / signed mobile/web RPCs; official APIs do not provide third-party playback view reporting or like/follow actions. |
| **Phone / Email / Google / Apple Login** | N/A | Not implemented | Web password/SMS/OAuth flows are protected by device/risk verification and CAPTCHA; never collect third-party passwords or bypass CAPTCHA. |

## 3. Compliance & Isolation Decisions

- `TikTokProvider::capabilities()` reports `recommended: false`, `following: false`, `search: false`, `actions: false`, `playback_events: false`, `ranges: true`.
- `ContentProvider::page(FeedKind::Recommended, ...)` returns `Err(ProviderError::Unsupported)` rather than substituting public Explore or Trending items.
- Public Explore is exposed explicitly via `TikTokProvider::explore_page`.
- All upstream CDN URLs remain encapsulated inside `TikTokProvider` (`Source::id` is an opaque handle); expired handles return `ProviderError::SourceExpired` so `MediaDelivery` refreshes them via `ContentProvider::sources`.
