# BG Tok

Rust foundation for a TikTok content provider hosted by BeyGram. This repository
is at feasibility stage: it does **not** provide login, a feed, playback, or a
C ABI yet. The personalized feed is gated on evidence of authorized access; see
[feasibility](docs/feasibility.md) and [architecture](docs/architecture.md).

## Development

Install stable Rust with rustfmt and clippy, then run:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
```

Tests use no service accounts or network access.
