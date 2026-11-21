//! Error mapping between HTTP transport and `ProviderError`.
use bg_tok::{network::NetworkError, provider::ProviderError};

/// Maps a [`NetworkError`] into a [`ProviderError`].
pub fn map_network_error(err: NetworkError) -> ProviderError {
    match err {
        NetworkError::Cancelled => ProviderError::Cancelled,
        NetworkError::Unauthorized => ProviderError::Unauthorized,
        NetworkError::RateLimited => ProviderError::RateLimited,
        NetworkError::Http(404) => ProviderError::NotFound,
        NetworkError::Http(410) => ProviderError::SourceExpired,
        NetworkError::Http(429) => ProviderError::RateLimited,
        NetworkError::Http(401 | 403) => ProviderError::Unauthorized,
        NetworkError::Timeout | NetworkError::Transport => ProviderError::Transport,
        NetworkError::Configuration | NetworkError::InvalidRange | NetworkError::Http(_) => {
            ProviderError::InvalidResponse
        }
    }
}
