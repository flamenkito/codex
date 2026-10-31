//! Explicit model routes retain the thread's primary provider and isolate upstream credentials.
//! Route files contain catalog metadata and credential variable names, never credential values.

use crate::ModelProviderInfo;
use codex_protocol::openai_models::ModelInfo;
use serde::Deserialize;
use serde::Serialize;
use std::collections::HashSet;
use std::io;
use std::path::Path;

pub const MODEL_ROUTES_FILE: &str = "model-routes.json";

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ModelRoutes {
    pub routes: Vec<ModelRoute>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ModelRoute {
    pub provider: RouteProvider,
    pub models: Vec<ModelInfo>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RouteProvider {
    pub name: String,
    pub base_url: String,
    pub env_key: String,
}

impl ModelRoutes {
    pub fn load(codex_home: &Path) -> io::Result<Self> {
        match std::fs::read(codex_home.join(MODEL_ROUTES_FILE)) {
            Ok(bytes) => {
                // Do not include user-controlled file contents in parse errors.
                let mut routes: Self = serde_json::from_slice(&bytes).map_err(|_| {
                    io::Error::new(io::ErrorKind::InvalidData, "Invalid model-routes.json")
                })?;
                routes.validate()?;
                for route in &mut routes.routes {
                    for model in &mut route.models {
                        model.use_responses_lite = false;
                    }
                }
                Ok(routes)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error),
        }
    }

    pub fn validate(&self) -> io::Result<()> {
        let invalid = || {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "Invalid model route configuration",
            )
        };
        let mut slugs = HashSet::new();
        for route in &self.routes {
            let provider = &route.provider;
            let uri: http::Uri = provider.base_url.parse().map_err(|_| invalid())?;
            let loopback = matches!(uri.host(), Some("127.0.0.1" | "localhost" | "[::1]"));
            if provider.name.trim().is_empty()
                || provider.name.eq_ignore_ascii_case("openai")
                || provider.env_key.is_empty()
                || provider.env_key.starts_with("OPENAI_")
                || provider.env_key.starts_with("CODEX_")
                || !provider
                    .env_key
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c == '_' || c.is_ascii_digit())
                || uri
                    .authority()
                    .is_none_or(|authority| authority.as_str().contains('@'))
                || !(uri.scheme_str() == Some("https")
                    || loopback && uri.scheme_str() == Some("http"))
                || uri.query().is_some()
                || route.models.is_empty()
            {
                return Err(invalid());
            }
            for model in &route.models {
                if model.slug.trim().is_empty()
                    || model.slug.starts_with("gpt-")
                    || model.slug.starts_with("codex-")
                    || !slugs.insert(model.slug.as_str())
                {
                    return Err(invalid());
                }
            }
        }
        Ok(())
    }

    pub fn provider_for_model(&self, model: &str) -> Option<ModelProviderInfo> {
        self.routes
            .iter()
            .find(|route| route.models.iter().any(|entry| entry.slug == model))
            .map(|route| ModelProviderInfo {
                name: route.provider.name.clone(),
                base_url: Some(route.provider.base_url.clone()),
                env_key: Some(route.provider.env_key.clone()),
                // The default custom provider uses HTTP Responses and no OpenAI authentication.
                ..Default::default()
            })
    }

    pub fn models(&self) -> impl Iterator<Item = &ModelInfo> {
        self.routes.iter().flat_map(|route| &route.models)
    }

    pub fn is_empty(&self) -> bool {
        self.routes.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn routes() -> ModelRoutes {
        // Use the protocol's full metadata schema, matching a real catalog file.
        let model: ModelInfo = serde_json::from_value(serde_json::json!({
            "slug": "k3.1-pro", "display_name": "Kimi K3.1 Pro", "description": "Kimi",
            "default_reasoning_level": "high", "supported_reasoning_levels": [],
            "shell_type": "shell_command", "visibility": "list", "supported_in_api": true,
            "priority": 200, "availability": "generally_available", "upgrade": null,
            "base_instructions": "", "supports_reasoning_summaries": false,
            "support_verbosity": false, "default_verbosity": null,
            "apply_patch_tool_type": "freeform", "web_search_tool_type": "text",
            "truncation_policy": {"mode": "tokens", "limit": 10000},
            "context_window": 200000, "effective_context_window_percent": 95,
            "experimental_supported_tools": [], "input_modalities": ["text"]
        }))
        .unwrap();
        ModelRoutes {
            routes: vec![ModelRoute {
                provider: RouteProvider {
                    name: "Kimi".into(),
                    base_url: "https://api.kimi.ai/coding/v1".into(),
                    env_key: "KIMI_API_KEY".into(),
                },
                models: vec![model],
            }],
        }
    }

    #[test]
    fn routes_are_exact_and_do_not_inherit_openai_authentication() {
        let routes = routes();
        routes.validate().unwrap();
        let provider = routes.provider_for_model("k3.1-pro").unwrap();
        assert_eq!(provider.env_key.as_deref(), Some("KIMI_API_KEY"));
        assert!(!provider.requires_openai_auth);
        assert!(!provider.supports_websockets);
        assert!(provider.http_headers.is_none());
        assert!(routes.provider_for_model("gpt-6.1-sol").is_none());
        assert!(routes.provider_for_model("k3.1-pro-unknown").is_none());
    }

    #[test]
    fn rejects_ambiguous_models_credentials_and_insecure_destinations() {
        for case in ["duplicate", "openai_key", "http", "userinfo", "reserved"] {
            let mut routes = routes();
            let route = &mut routes.routes[0];
            match case {
                "duplicate" => route.models.push(route.models[0].clone()),
                "openai_key" => route.provider.env_key = "OPENAI_API_KEY".into(),
                "http" => route.provider.base_url = "http://api.kimi.ai/coding/v1".into(),
                "userinfo" => {
                    route.provider.base_url = "https://secret@api.kimi.ai/coding/v1".into()
                }
                "reserved" => route.models[0].slug = "gpt-6.1-sol".into(),
                _ => unreachable!(),
            }
            assert!(routes.validate().is_err(), "{case}");
        }
    }

    #[test]
    fn absent_file_preserves_normal_behavior_and_invalid_file_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        assert!(ModelRoutes::load(dir.path()).unwrap().is_empty());
        std::fs::write(dir.path().join(MODEL_ROUTES_FILE), "not json: secret").unwrap();
        let error = ModelRoutes::load(dir.path()).unwrap_err().to_string();
        assert!(!error.contains("secret"));
    }
}
