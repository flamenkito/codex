//! Add explicitly routed models without replacing or contaminating the primary catalog cache.

use crate::CatalogModel;
use crate::ModelOrigin;
use crate::ModelsManagerConfig;
use crate::manager::ModelsManager;
use crate::manager::ModelsManagerFuture;
use crate::manager::RefreshStrategy;
use crate::manager::SharedModelsManager;
use codex_http_client::HttpClientFactory;
use codex_login::AuthManager;
use codex_model_provider_info::ModelProviderInfo;
use codex_model_provider_info::model_routes::ModelRoutes;
use codex_protocol::config_types::CollaborationModeMask;
use codex_protocol::openai_models::ModelInfo;
use codex_protocol::openai_models::ModelPreset;
use std::sync::Arc;
use tokio::sync::TryLockError;

#[derive(Debug)]
struct RoutedModelsManager {
    primary: SharedModelsManager,
    routes: Arc<ModelRoutes>,
}

pub fn with_model_routes(
    primary: SharedModelsManager,
    routes: Arc<ModelRoutes>,
) -> SharedModelsManager {
    if routes.is_empty() {
        primary
    } else {
        Arc::new(RoutedModelsManager { primary, routes })
    }
}

impl RoutedModelsManager {
    fn merge(&self, mut models: Vec<ModelInfo>) -> Vec<ModelInfo> {
        models.retain(|model| self.routes.provider_for_model(&model.slug).is_none());
        models.extend(self.routes.models().cloned());
        models
    }
}

impl ModelsManager for RoutedModelsManager {
    fn routed_provider_for_model(&self, model: &str) -> Option<ModelProviderInfo> {
        self.routes.provider_for_model(model)
    }

    fn has_model_routes(&self) -> bool {
        true
    }

    fn for_originator(&self, originator: &str) -> Option<SharedModelsManager> {
        self.primary
            .for_originator(originator)
            .map(|primary| with_model_routes(primary, Arc::clone(&self.routes)))
    }

    fn set_api_key_model_discovery_enabled(&self, enabled: bool) {
        self.primary.set_api_key_model_discovery_enabled(enabled);
    }

    fn model_catalog_with_origins(
        &self,
        strategy: RefreshStrategy,
        factory: HttpClientFactory,
    ) -> ModelsManagerFuture<'_, Vec<CatalogModel>> {
        Box::pin(async move {
            let mut catalog = self
                .primary
                .model_catalog_with_origins(strategy, factory)
                .await;
            catalog.retain(|entry| self.routes.provider_for_model(&entry.model.slug).is_none());
            catalog.extend(self.routes.models().cloned().map(|model| CatalogModel {
                model,
                origin: ModelOrigin::ExplicitConfig,
            }));
            catalog
        })
    }

    fn refresh_after_auth_change(&self, factory: HttpClientFactory) -> ModelsManagerFuture<'_, ()> {
        self.primary.refresh_after_auth_change(factory)
    }

    fn get_remote_models(&self) -> ModelsManagerFuture<'_, Vec<ModelInfo>> {
        Box::pin(async move { self.merge(self.primary.get_remote_models().await) })
    }

    fn try_get_remote_models(&self) -> Result<Vec<ModelInfo>, TryLockError> {
        self.primary
            .try_get_remote_models()
            .map(|models| self.merge(models))
    }

    fn auth_manager(&self) -> Option<&AuthManager> {
        self.primary.auth_manager()
    }

    fn build_available_models(&self, mut models: Vec<ModelInfo>) -> Vec<ModelPreset> {
        models.retain(|model| self.routes.provider_for_model(&model.slug).is_none());
        let mut presets = self.primary.build_available_models(models);
        // Primary-account entitlements do not determine access to an independently authenticated route.
        presets.extend(self.routes.models().cloned().map(|model| {
            let mut preset = ModelPreset::from(model);
            preset.is_default = false;
            preset
        }));
        presets
    }

    fn list_collaboration_modes(&self) -> Vec<CollaborationModeMask> {
        self.primary.list_collaboration_modes()
    }

    fn get_model_info<'a>(
        &'a self,
        model: &'a str,
        config: &'a ModelsManagerConfig,
    ) -> ModelsManagerFuture<'a, ModelInfo> {
        if self.routes.provider_for_model(model).is_some() {
            Box::pin(async move {
                crate::manager::construct_model_info_from_candidates(
                    model,
                    &self.routes.models().cloned().collect::<Vec<_>>(),
                    config,
                )
            })
        } else {
            self.primary.get_model_info(model, config)
        }
    }

    fn refresh_if_new_etag(
        &self,
        etag: String,
        factory: HttpClientFactory,
    ) -> ModelsManagerFuture<'_, ()> {
        self.primary.refresh_if_new_etag(etag, factory)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manager::StaticModelsManager;
    use codex_model_provider_info::model_routes::ModelRoute;
    use codex_model_provider_info::model_routes::RouteProvider;
    use codex_protocol::openai_models::ModelVisibility;
    use codex_protocol::openai_models::ModelsResponse;

    #[tokio::test]
    async fn routed_catalog_preserves_primary_metadata_and_adds_exact_route_models() {
        let mut primary = crate::bundled_models_response().unwrap().models.remove(0);
        primary.slug = "gpt-primary-test".into();
        primary.visibility = ModelVisibility::List;
        primary.supported_in_api = true;
        let mut kimi = primary.clone();
        kimi.slug = "k3.1-pro".into();
        kimi.display_name = "Kimi K3.1 Pro".into();
        kimi.context_window = Some(256_000);
        let inner: SharedModelsManager = Arc::new(StaticModelsManager::new(
            None,
            ModelsResponse {
                models: vec![primary.clone()],
            },
            ModelOrigin::ExplicitConfig,
        ));
        let routes = Arc::new(ModelRoutes {
            routes: vec![ModelRoute {
                provider: RouteProvider {
                    name: "Kimi".into(),
                    base_url: "https://api.kimi.ai/coding/v1".into(),
                    env_key: "KIMI_API_KEY".into(),
                },
                models: vec![kimi.clone()],
            }],
        });
        let manager = with_model_routes(Arc::clone(&inner), routes);
        let listed = manager.try_list_models().unwrap();
        assert!(listed.iter().any(|model| model.model == primary.slug));
        assert!(listed.iter().any(|model| model.model == kimi.slug));
        assert_eq!(
            inner.try_get_remote_models().unwrap(),
            vec![primary.clone()]
        );
        assert!(manager.routed_provider_for_model(&primary.slug).is_none());
        assert!(manager.routed_provider_for_model("k3.1-pro-typo").is_none());
        assert!(manager.routed_provider_for_model(&kimi.slug).is_some());
        let resolved = manager
            .get_model_info(&kimi.slug, &ModelsManagerConfig::default())
            .await;
        assert_eq!(resolved.context_window, kimi.context_window);
        assert!(!resolved.used_fallback_model_metadata);
        let resolved_primary = manager
            .get_model_info(&primary.slug, &ModelsManagerConfig::default())
            .await;
        assert_eq!(resolved_primary.display_name, primary.display_name);
        assert_eq!(manager.try_get_remote_models().unwrap().len(), 2);
    }
}
