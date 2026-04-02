use std::sync::Arc;

use crate::AuthManager;
use crate::model_provider_info::ModelProviderInfo;

pub(crate) fn auth_manager_for_provider(
    auth_manager: Option<Arc<AuthManager>>,
    provider: &ModelProviderInfo,
) -> Option<Arc<AuthManager>> {
    match provider.auth.clone() {
        Some(config) => Some(AuthManager::external_bearer_only(config)),
        None => auth_manager,
    }
}

pub(crate) fn required_auth_manager_for_provider(
    auth_manager: Arc<AuthManager>,
    provider: &ModelProviderInfo,
) -> Arc<AuthManager> {
    match provider.auth.clone() {
        Some(config) => AuthManager::external_bearer_only(config),
        None => auth_manager,
    }
}
