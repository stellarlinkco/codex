use crate::ModelProviderInfo;
use codex_protocol::models::ImageDetail;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum ProviderRuntime {
    RemoteCompaction,
    Default,
}

impl ProviderRuntime {
    pub(crate) fn from_provider(provider: &ModelProviderInfo) -> Self {
        if codex_api::supports_remote_compaction(&provider.name, provider.base_url.as_deref()) {
            Self::RemoteCompaction
        } else {
            Self::Default
        }
    }

    pub(crate) fn supports_remote_compaction(self) -> bool {
        matches!(self, Self::RemoteCompaction)
    }

    pub(crate) fn supports_original_image_detail(self, model_supports_original: bool) -> bool {
        let _ = self;
        model_supports_original
    }
}

pub(crate) fn default_output_image_detail(
    provider: &ModelProviderInfo,
    image_detail_original_enabled: bool,
    model_supports_original: bool,
) -> Option<ImageDetail> {
    (image_detail_original_enabled
        && ProviderRuntime::from_provider(provider)
            .supports_original_image_detail(model_supports_original))
    .then_some(ImageDetail::Original)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::WireApi;
    use pretty_assertions::assert_eq;

    fn test_provider(name: &str, base_url: Option<&str>) -> ModelProviderInfo {
        ModelProviderInfo {
            name: name.to_string(),
            base_url: base_url.map(str::to_string),
            env_key: None,
            env_key_instructions: None,
            experimental_bearer_token: None,
            wire_api: WireApi::Responses,
            query_params: None,
            http_headers: None,
            env_http_headers: None,
            request_max_retries: None,
            stream_max_retries: None,
            stream_idle_timeout_ms: None,
            requires_openai_auth: false,
            supports_websockets: false,
        }
    }

    #[test]
    fn maps_openai_and_azure_to_remote_compaction_runtime() {
        let openai = test_provider("OpenAI", Some("https://api.openai.com/v1"));
        let azure = test_provider("Azure", Some("https://example.com/openai/deployments/test"));
        let other = test_provider("test", Some("https://example.com/openai"));

        assert_eq!(
            ProviderRuntime::from_provider(&openai),
            ProviderRuntime::RemoteCompaction
        );
        assert_eq!(
            ProviderRuntime::from_provider(&azure),
            ProviderRuntime::RemoteCompaction
        );
        assert_eq!(
            ProviderRuntime::from_provider(&other),
            ProviderRuntime::Default
        );
    }

    #[test]
    fn original_image_detail_follows_feature_flag_and_model_capability() {
        let provider = test_provider("test", Some("https://example.com/openai"));

        assert_eq!(
            default_output_image_detail(&provider, true, true),
            Some(ImageDetail::Original)
        );
        assert_eq!(default_output_image_detail(&provider, false, true), None);
        assert_eq!(default_output_image_detail(&provider, true, false), None);
    }
}
