use codex_client::Request;
use codex_client::RequestCompression;
use codex_client::RetryOn;
use codex_client::RetryPolicy;
use http::Method;
use http::header::HeaderMap;
use std::collections::HashMap;
use std::time::Duration;
use url::Url;

/// High-level retry configuration for a provider.
///
/// This is converted into a `RetryPolicy` used by `codex-client` to drive
/// transport-level retries for both unary and streaming calls.
#[derive(Debug, Clone)]
pub struct RetryConfig {
    pub max_attempts: u64,
    pub base_delay: Duration,
    pub retry_429: bool,
    pub retry_5xx: bool,
    pub retry_transport: bool,
}

impl RetryConfig {
    pub fn to_policy(&self) -> RetryPolicy {
        RetryPolicy {
            max_attempts: self.max_attempts,
            base_delay: self.base_delay,
            retry_on: RetryOn {
                retry_429: self.retry_429,
                retry_5xx: self.retry_5xx,
                retry_transport: self.retry_transport,
            },
        }
    }
}

/// HTTP endpoint configuration used to talk to a concrete API deployment.
///
/// Encapsulates base URL, default headers, query params, retry policy, and
/// stream idle timeout, plus helper methods for building requests.
#[derive(Debug, Clone)]
pub struct Provider {
    pub name: String,
    pub base_url: String,
    pub query_params: Option<HashMap<String, String>>,
    pub headers: HeaderMap,
    pub retry: RetryConfig,
    pub stream_idle_timeout: Duration,
}

impl Provider {
    pub fn url_for_path(&self, path: &str) -> String {
        let base = self.base_url.trim_end_matches('/');
        let path = path.trim_start_matches('/');
        let mut url = if path.is_empty() {
            base.to_string()
        } else {
            format!("{base}/{path}")
        };

        if let Some(params) = &self.query_params
            && !params.is_empty()
        {
            let qs = params
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("&");
            url.push('?');
            url.push_str(&qs);
        }

        url
    }

    pub fn build_request(&self, method: Method, path: &str) -> Request {
        Request {
            method,
            url: self.url_for_path(path),
            headers: self.headers.clone(),
            body: None,
            compression: RequestCompression::None,
            timeout: None,
        }
    }

    pub fn is_azure_responses_endpoint(&self) -> bool {
        is_azure_responses_wire_base_url(&self.name, Some(&self.base_url))
    }

    pub fn stores_responses_by_default(&self) -> bool {
        stores_responses_by_default(&self.name, Some(&self.base_url))
    }

    pub fn supports_chatgpt_request_compression(&self) -> bool {
        supports_chatgpt_request_compression(&self.name, Some(&self.base_url))
    }

    pub fn supports_realtime_env_api_key_fallback(&self) -> bool {
        supports_realtime_env_api_key_fallback(&self.name, Some(&self.base_url))
    }

    pub fn websocket_url_for_path(&self, path: &str) -> Result<Url, url::ParseError> {
        let mut url = Url::parse(&self.url_for_path(path))?;

        let scheme = match url.scheme() {
            "http" => "ws",
            "https" => "wss",
            "ws" | "wss" => return Ok(url),
            _ => return Ok(url),
        };
        let _ = url.set_scheme(scheme);
        Ok(url)
    }
}

pub fn supports_remote_compaction(name: &str, base_url: Option<&str>) -> bool {
    name.eq_ignore_ascii_case("openai") || is_azure_responses_wire_base_url(name, base_url)
}

pub fn supports_chatgpt_request_compression(name: &str, _base_url: Option<&str>) -> bool {
    name.eq_ignore_ascii_case("openai")
}

pub fn supports_realtime_env_api_key_fallback(name: &str, _base_url: Option<&str>) -> bool {
    name.eq_ignore_ascii_case("openai")
}

pub fn stores_responses_by_default(name: &str, base_url: Option<&str>) -> bool {
    is_azure_responses_wire_base_url(name, base_url)
}

pub fn is_azure_responses_wire_base_url(name: &str, base_url: Option<&str>) -> bool {
    if name.eq_ignore_ascii_case("azure") {
        return true;
    }

    let Some(base_url) = base_url else {
        return false;
    };

    let base = base_url.to_ascii_lowercase();
    base.contains("openai.azure.") || matches_azure_responses_base_url(&base)
}

fn matches_azure_responses_base_url(base_url: &str) -> bool {
    const AZURE_MARKERS: [&str; 5] = [
        "cognitiveservices.azure.",
        "aoai.azure.",
        "azure-api.",
        "azurefd.",
        "windows.net/openai",
    ];
    AZURE_MARKERS.iter().any(|marker| base_url.contains(marker))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_providers_that_support_chatgpt_request_compression() {
        assert!(supports_chatgpt_request_compression(
            "OpenAI",
            Some("https://api.openai.com/v1")
        ));
        assert!(!supports_chatgpt_request_compression(
            "Azure",
            Some("https://example.com/openai/deployments/test")
        ));
        assert!(!supports_chatgpt_request_compression(
            "gpt-oss",
            Some("https://api.openai.com/v1")
        ));
    }

    #[test]
    fn detects_providers_that_support_realtime_env_api_key_fallback() {
        assert!(supports_realtime_env_api_key_fallback(
            "OpenAI",
            Some("https://api.openai.com/v1")
        ));
        assert!(!supports_realtime_env_api_key_fallback(
            "Azure",
            Some("https://example.com/openai/deployments/test")
        ));
        assert!(!supports_realtime_env_api_key_fallback(
            "gpt-oss",
            Some("https://api.openai.com/v1")
        ));
    }

    #[test]
    fn detects_remote_compaction_providers() {
        assert!(supports_remote_compaction(
            "OpenAI",
            Some("https://api.openai.com/v1")
        ));
        assert!(supports_remote_compaction(
            "test",
            Some("https://foo.openai.azure.com/openai")
        ));
        assert!(!supports_remote_compaction(
            "test",
            Some("https://example.com/openai")
        ));
    }

    #[test]
    fn detects_providers_that_store_responses_by_default() {
        assert!(stores_responses_by_default(
            "Azure",
            Some("https://example.com/openai/deployments/test")
        ));
        assert!(!stores_responses_by_default(
            "OpenAI",
            Some("https://api.openai.com/v1")
        ));
    }

    #[test]
    fn detects_azure_responses_base_urls() {
        let positive_cases = [
            "https://foo.openai.azure.com/openai",
            "https://foo.openai.azure.us/openai/deployments/bar",
            "https://foo.cognitiveservices.azure.cn/openai",
            "https://foo.aoai.azure.com/openai",
            "https://foo.openai.azure-api.net/openai",
            "https://foo.z01.azurefd.net/",
        ];

        for base_url in positive_cases {
            assert!(
                is_azure_responses_wire_base_url("test", Some(base_url)),
                "expected {base_url} to be detected as Azure"
            );
        }

        assert!(is_azure_responses_wire_base_url(
            "Azure",
            Some("https://example.com")
        ));

        let negative_cases = [
            "https://api.openai.com/v1",
            "https://example.com/openai",
            "https://myproxy.azurewebsites.net/openai",
        ];

        for base_url in negative_cases {
            assert!(
                !is_azure_responses_wire_base_url("test", Some(base_url)),
                "expected {base_url} not to be detected as Azure"
            );
        }
    }
}
