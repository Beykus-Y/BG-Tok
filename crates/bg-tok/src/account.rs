//! Host-supplied secure storage and cancellation on account switch/logout.
use crate::provider::ProviderError;
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

/// Secret is zeroed on drop and does not implement Debug, Display or Clone.
pub struct Secret(Zeroizing<Vec<u8>>);
impl Secret {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(Zeroizing::new(bytes))
    }
    pub fn expose(&self) -> &[u8] {
        &self.0
    }
}
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct AccountId(pub String);

/// Implementations are supplied by the host (e.g. Keychain/Keystore).
/// No persistent implementation or real secrets are included in BG Tok.
#[async_trait]
pub trait SecureStore: Send + Sync {
    async fn save(&self, id: &AccountId, secret: &Secret) -> Result<(), ProviderError>;
    async fn load(&self, id: &AccountId) -> Result<Option<Secret>, ProviderError>;
    async fn delete(&self, id: &AccountId) -> Result<(), ProviderError>;
}
#[derive(Clone)]
pub struct ActiveAccount {
    pub id: AccountId,
    pub cancellation: CancellationToken,
}
struct State {
    active: Option<ActiveAccount>,
}
pub struct Accounts<S: SecureStore> {
    store: Arc<S>,
    state: Mutex<State>,
}
impl<S: SecureStore> Accounts<S> {
    pub fn new(store: Arc<S>) -> Self {
        Self {
            store,
            state: Mutex::new(State { active: None }),
        }
    }
    pub async fn save(&self, id: &AccountId, secret: &Secret) -> Result<(), ProviderError> {
        let _guard = self.state.lock().await;
        self.store.save(id, secret).await
    }
    /// Serializes switches and invalidates all work tied to the previous account.
    pub async fn activate(&self, id: &AccountId) -> Result<ActiveAccount, ProviderError> {
        let mut state = self.state.lock().await;
        let _secret = self
            .store
            .load(id)
            .await?
            .ok_or(ProviderError::Unauthorized)?;
        if let Some(old) = state.active.take() {
            old.cancellation.cancel();
        }
        let active = ActiveAccount {
            id: id.clone(),
            cancellation: CancellationToken::new(),
        };
        state.active = Some(active.clone());
        Ok(active)
    }
    pub async fn active(&self) -> Option<ActiveAccount> {
        self.state.lock().await.active.clone()
    }
    /// Cancels active work before removing local credentials. Provider-side
    /// revocation must be performed separately by the provider when supported.
    pub async fn logout(&self, id: &AccountId) -> Result<(), ProviderError> {
        let mut state = self.state.lock().await;
        if state.active.as_ref().is_some_and(|active| &active.id == id) {
            if let Some(active) = state.active.take() {
                active.cancellation.cancel();
            }
        }
        self.store.delete(id).await
    }
}
