# Feasibility gate: personalized TikTok feed

Review date: 2026-11-20. This is a documentation review, not a live service
verification or legal opinion. No developer application, approved scopes or
volunteer account were available for an end-to-end test.

## Evidence and decision

* [Login Kit](https://developers.tiktok.com/doc/login-kit-overview/) documents
  user authorization for TikTok developer products. Login alone does not grant
  a For You feed or a TikTok app session.
* [Display API](https://developers.tiktok.com/doc/display-api-overview/) and
  [video list](https://developers.tiktok.com/doc/tiktok-api-v2-video-list/)
  cover a consenting user's profile and publicly posted videos; the latter is
  not that user's personalized recommendations. Its documented embed/display
  use is not proof of permission to fetch raw media bytes for a native player.
* [Research API](https://developers.tiktok.com/doc/research-api-get-started/)
  is subject to eligibility and approval and is intended for research. It is
  not evidence of access to a consumer's personalized feed or view reporting.
* [Developer guidelines](https://developers.tiktok.com/doc/our-guidelines-developer-guidelines/)
  and [developer terms](https://www.tiktok.com/legal/page/global/tik-tok-developer-terms-of-service/en)
  require review for the intended product, distribution and jurisdiction.
  Product approval, scopes and current terms must be checked before any launch.
* [TikTok-Api](https://github.com/davidteather/TikTok-Api) identifies itself as
  an unofficial Python wrapper (MIT). Its advertised methods do not establish
  authorized access, stable operation, playback rights or truthful view
  reporting. No code has been copied; its license does not grant TikTok rights.

The reviewed official documentation does not establish an authorized native
For You feed, direct media byte access, or an API for reporting genuine playback
views. Those capabilities are **unverified**, not implemented. Do not proceed
with the personalized feed implementation until all are demonstrated under
applicable permissions and terms. Unofficial endpoints can change and may
expose accounts to restrictions; do not bypass access controls, CAPTCHA, DRM,
or extract cookies from another app or browser.

## Authentication and alternatives

The host may initiate official TikTok authorization only after an explicit user
action, using the platform's permitted system authorization mechanism. QR,
phone/SMS and email login as app-session acquisition methods are not established
by the reviewed developer documentation. Google and Apple sign-in through a
system flow are likewise unverified for this integration: their tokens must not
be treated as TikTok sessions, and their passwords must never be collected.

A permitted alternative to investigate is an approved Display API experience
for the consenting user's public posts or links, subject to its display and
sharing terms. It is not a replacement for the personalized native feed. An
explicit TikTok partnership or future official product may change the decision.

## Evidence needed to reopen the gate

1. Written product/scope approval and applicable terms covering a consumer's
   personalized recommendation order, native playback and view events.
2. A voluntary test account and explicit consent; document the supported login
   flow, revocation, account isolation and any geographic restrictions.
3. A reproducible manual smoke test of feed pagination and ordering, permitted
   media playback/expiry and real playback event submission, including failure
   and rate-limit behavior. Never use user accounts in CI or store tokens,
   signed URLs or private responses in fixtures.
4. Record limitations and results here without secrets. If any part fails or is
   unavailable, keep the gate closed and report the supported alternative.

## Open implementation review (2026-11-20)

This review inspected project metadata and READMEs, not live TikTok responses:

* [TikTok-Api](https://github.com/davidteather/TikTok-Api) (MIT) describes an
  unofficial Python/Playwright data retrieval wrapper. Its README explicitly
  says it has no user-authenticated routes and cannot post on behalf of a user.
  It cannot demonstrate the required personalized account feed or view events.
* [TikTokLive](https://github.com/isaackogan/TikTokLive) (AGPL-3.0) describes
  unofficial livestream event reception and calls itself a reverse engineering
  project, not production-ready. LIVE event reception is a different use case;
  it does not demonstrate the required personalized feed or native playback.
  Its license and any service terms require independent review before reuse.

These statements establish what the projects describe, not that their methods
currently work or are authorized. Neither implementation was incorporated or
run against the service; no protection bypass is proposed.
