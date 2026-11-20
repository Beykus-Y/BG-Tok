# BG Tok

Rust provider-independent primitives for BeyGram, with a separate synthetic
provider and CLI. There is no real TikTok provider, login, player or C ABI yet.
The requirements for real service access are in [feasibility](docs/feasibility.md).
See [architecture](docs/architecture.md) for the current interfaces and limits.

## Development

Install stable Rust with rustfmt and clippy:

```sh
cargo check --workspace --locked
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
cargo run -p bg-tok-mock
```

The mock package is a separate workspace member, excluded from the default
build and from the core's dependencies. The CLI uses only synthetic data. Tests
use local loopback HTTP servers, never service accounts or the TikTok network.
