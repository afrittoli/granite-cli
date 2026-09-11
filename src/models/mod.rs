// Standard
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

// Third Party
use alog::{MessageLevel, alog_channel, use_channel};

// Include generated code from build.rs
include!(concat!(env!("OUT_DIR"), "/generated_models.rs"));

use_channel!("MODEL");

/*-- Public API --------------------------------------------------------------*/

pub static MODEL_REGISTRY: LazyLock<base::ModelFactory> = LazyLock::new(|| {
    let mut factory = base::ModelFactory::new();
    register_all_models(&mut factory);
    factory.register::<custom::CustomModel>("custom");
    factory
});

/*-- ModelSource ---------------------------------------------------------------*/

/// The real `Configured<dyn Model>`: builds a live model instance the first
/// time one is asked for by its instance id (`ModelConfig.model_id`) --
/// distinct from the registry key it's constructed from
/// (`ModelConfig.model_type`), so the same catalog type can be configured
/// more than once. The instance is kept, so every later ask for that id
/// returns the same object.
pub struct ModelSource {
    /// The configuration this source was built from. Narrows to
    /// `HashMap<String, ModelConfig>` once `construct` stops taking the whole
    /// configuration.
    config: crate::config::Config,
    model_proxy: Option<crate::proxy::ProxyHandle>,
    cache: std::sync::Mutex<HashMap<String, Arc<dyn Model>>>,
}

impl ModelSource {
    pub fn from_config(config: &crate::config::Config) -> Self {
        Self {
            config: config.clone(),
            model_proxy: config.model_proxy.clone(),
            cache: std::sync::Mutex::new(HashMap::new()),
        }
    }

    /// The model configured under `model_id` (the instance id -- matches
    /// `ModelConfig.model_id`, which config loading enforces equals the outer
    /// `config.models` key), built on the first ask and returned from the
    /// cache on every one after it, so two capabilities naming one model
    /// share one object. Errors when no entry is configured under that id, or
    /// when its `model_type` is not in the registry.
    pub fn get(&self, model_id: &str) -> anyhow::Result<Arc<dyn Model>> {
        if let Some(built) = self.cache.lock().unwrap().get(model_id) {
            return Ok(built.clone());
        }
        let model_config = self
            .config
            .models
            .get(model_id)
            .ok_or_else(|| anyhow::anyhow!("model '{model_id}' is not configured"))?;

        let mut cfg = model_config.config.clone();
        if let Some(provider_config) = self.config.get_provider(&model_config.provider_id) {
            cfg["provider_config"] = serde_json::to_value(provider_config).unwrap_or_default();
        }
        let built = MODEL_REGISTRY
            .construct(
                &model_config.model_type,
                &model_config.model_id,
                &cfg,
                &self.config,
            )
            .map_err(|e| anyhow::anyhow!("could not construct model '{model_id}': {e}"))?;

        let built: Arc<dyn Model> = Arc::from(built);
        let built = match &self.model_proxy {
            Some(handle) => {
                route_and_wrap(built, model_id, model_config.variant.as_deref(), handle)
            }
            None => built,
        };
        self.cache
            .lock()
            .unwrap()
            .insert(model_id.to_string(), built.clone());
        Ok(built)
    }

    /// Instance ids built so far, sorted. Lets a test tell what a call
    /// built from what it merely could have built.
    #[cfg(test)]
    pub(crate) fn cached_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.cache.lock().unwrap().keys().cloned().collect();
        ids.sort();
        ids
    }
}

/*-- private ------------------------------------------------------------------*/

/// Registers `model`'s real connection details as a route on the session
/// proxy and returns it wrapped to point at the proxy instead of the real
/// upstream. The route key is computed on the real, unwrapped provider from
/// the variant the model was configured with, so it matches the alias
/// `resolve_provider_endpoint` computes later and the launched process
/// addresses the model by. Registration is best-effort: a failure is logged
/// and the model is returned unrouted rather than failing construction over
/// an accounting feature.
fn route_and_wrap(
    model: Arc<dyn Model>,
    model_id: &str,
    configured_variant: Option<&str>,
    handle: &crate::proxy::ProxyHandle,
) -> Arc<dyn Model> {
    match model.provider() {
        Ok(provider) => {
            let variant = base::find_variant(model.variants(), configured_variant);
            let route_key = provider
                .model_alias(model_id.to_string(), variant)
                .unwrap_or_else(|| model_id.to_string());
            let target = crate::proxy::UpstreamTarget {
                base_url: provider.base_url().to_string(),
                verify_ssl: provider.verify_ssl(),
                auth: crate::proxy::UpstreamAuth::Inject(provider.api_key().cloned()),
            };
            if let Err(e) = handle.register_route(route_key, target, model_id.to_string()) {
                alog_channel!(
                    MessageLevel::Warning,
                    "failed to register proxy route for model '{model_id}': {e}"
                );
            }
        }
        Err(e) => {
            alog_channel!(
                MessageLevel::Warning,
                "model '{model_id}' has no usable provider, skipping proxy route: {e}"
            );
        }
    }
    Arc::new(crate::proxy::ProxiedModel::wrap(
        model,
        handle.local_base_url.clone(),
    ))
}

impl crate::dependency::Configured<dyn Model> for ModelSource {
    fn instances(&self) -> Vec<(String, Arc<dyn Model + 'static>)> {
        self.config
            .models
            .keys()
            .filter_map(|id| match self.get(id) {
                Ok(model) => Some((id.clone(), model)),
                Err(e) => {
                    alog_channel!(MessageLevel::Warning, "{e}");
                    None
                }
            })
            .collect()
    }

    fn catalog(&self) -> HashMap<&'static str, ModelMetadata> {
        MODEL_REGISTRY.entries()
    }

    fn config_schema(&self, type_name: &str) -> Option<schemars::Schema> {
        MODEL_REGISTRY.config_schema(type_name)
    }
}

// Re-export types from base
mod base;
pub use base::{
    ConfiguredModel, LayerKind, LayerTypeCount, MambaShape, Model, ModelArchitecture,
    ModelFunction, ModelMetadata, ModelType, ModelVariant,
};

mod custom;
pub use custom::CustomModelConfig;

pub(crate) mod context_fit;
pub use context_fit::{ContextFit, required_gb};

pub mod huggingface;

/*-- tests -------------------------------------------------------------------*/

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_source_constructs_one_instance_per_configured_model() {
        use crate::config::{Config, ModelConfig};
        use crate::dependency::Configured;

        let mut config = Config::default();
        config.models.insert(
            "granite-3.1-8b-instruct".to_string(),
            ModelConfig {
                model_id: "granite-3.1-8b-instruct".to_string(),
                model_type: "granite-3.1-8b-instruct".to_string(),
                config: serde_json::json!({}),
                provider_id: "ollama".to_string(),
                variant: None,
            },
        );
        config.models.insert(
            "granite-guardian-3.1-8b".to_string(),
            ModelConfig {
                model_id: "granite-guardian-3.1-8b".to_string(),
                model_type: "granite-guardian-3.1-8b".to_string(),
                config: serde_json::json!({}),
                provider_id: "ollama".to_string(),
                variant: None,
            },
        );

        let source = ModelSource::from_config(&config);
        let mut ids: Vec<String> = source.instances().into_iter().map(|(id, _)| id).collect();
        ids.sort();
        assert_eq!(
            ids,
            vec![
                "granite-3.1-8b-instruct".to_string(),
                "granite-guardian-3.1-8b".to_string()
            ]
        );
    }

    #[test]
    fn model_source_resolves_provider_from_provider_id() {
        use crate::config::{Config, ModelConfig, ProviderConfig};
        use crate::dependency::Configured;

        let mut config = Config::default();
        config.providers.insert(
            "ollama".to_string(),
            ProviderConfig {
                provider_id: "ollama".to_string(),
                provider_type: "ollama".to_string(),
                config: serde_json::json!({ "base_url": "http://localhost:11434" }),
            },
        );
        config.models.insert(
            "granite-3.1-8b-instruct".to_string(),
            ModelConfig {
                model_id: "granite-3.1-8b-instruct".to_string(),
                model_type: "granite-3.1-8b-instruct".to_string(),
                config: serde_json::json!({}),
                provider_id: "ollama".to_string(),
                variant: None,
            },
        );

        let source = ModelSource::from_config(&config);
        let (_, model) = source
            .instances()
            .into_iter()
            .find(|(id, _)| id == "granite-3.1-8b-instruct")
            .unwrap();
        let provider = model.provider().unwrap();
        assert_eq!(provider.base_url(), "http://localhost:11434");
    }

    #[test]
    fn model_source_provider_errs_when_provider_id_unresolvable() {
        use crate::config::{Config, ModelConfig};
        use crate::dependency::Configured;

        let mut config = Config::default();
        config.models.insert(
            "granite-3.1-8b-instruct".to_string(),
            ModelConfig {
                model_id: "granite-3.1-8b-instruct".to_string(),
                model_type: "granite-3.1-8b-instruct".to_string(),
                config: serde_json::json!({}),
                provider_id: "does-not-exist".to_string(),
                variant: None,
            },
        );

        let source = ModelSource::from_config(&config);
        let (_, model) = source
            .instances()
            .into_iter()
            .find(|(id, _)| id == "granite-3.1-8b-instruct")
            .unwrap();
        assert!(model.provider().is_err());
    }

    #[test]
    fn model_source_skips_unknown_model_ids() {
        use crate::config::{Config, ModelConfig};
        use crate::dependency::Configured;

        let mut config = Config::default();
        config.models.insert(
            "not-a-real-model".to_string(),
            ModelConfig {
                model_id: "not-a-real-model".to_string(),
                model_type: "not-a-real-model".to_string(),
                config: serde_json::json!({}),
                provider_id: "ollama".to_string(),
                variant: None,
            },
        );

        let source = ModelSource::from_config(&config);
        assert!(source.instances().is_empty());
    }

    #[test]
    fn get_returns_the_same_instance_for_every_call() {
        use crate::config::{Config, ModelConfig};

        let mut config = Config::default();
        config.models.insert(
            "granite-3.1-8b-instruct".to_string(),
            ModelConfig {
                model_id: "granite-3.1-8b-instruct".to_string(),
                model_type: "granite-3.1-8b-instruct".to_string(),
                config: serde_json::json!({}),
                provider_id: "ollama".to_string(),
                variant: None,
            },
        );

        let source = ModelSource::from_config(&config);
        let first = source.get("granite-3.1-8b-instruct").unwrap();
        let second = source.get("granite-3.1-8b-instruct").unwrap();
        assert!(
            Arc::ptr_eq(&first, &second),
            "a second get must return the same object, not a rebuilt one"
        );
    }

    #[test]
    fn instances_hands_out_the_same_object_get_does() {
        use crate::config::{Config, ModelConfig};
        use crate::dependency::Configured;

        let mut config = Config::default();
        config.models.insert(
            "granite-3.1-8b-instruct".to_string(),
            ModelConfig {
                model_id: "granite-3.1-8b-instruct".to_string(),
                model_type: "granite-3.1-8b-instruct".to_string(),
                config: serde_json::json!({}),
                provider_id: "ollama".to_string(),
                variant: None,
            },
        );

        let source = ModelSource::from_config(&config);
        let from_instances = source
            .instances()
            .into_iter()
            .find(|(id, _)| id == "granite-3.1-8b-instruct")
            .map(|(_, model)| model)
            .unwrap();
        let from_get = source.get("granite-3.1-8b-instruct").unwrap();
        assert!(Arc::ptr_eq(&from_instances, &from_get));
    }

    #[test]
    fn get_errs_naming_an_id_that_is_not_configured() {
        use crate::config::Config;

        let source = ModelSource::from_config(&Config::default());
        let err = source
            .get("not-configured")
            .err()
            .expect("an unconfigured id must not resolve")
            .to_string();
        assert!(
            err.contains("not-configured"),
            "the error must name the id that was asked for, got: {err}"
        );
    }

    #[test]
    fn get_errs_for_a_configured_id_whose_type_is_unknown() {
        use crate::config::{Config, ModelConfig};

        let mut config = Config::default();
        config.models.insert(
            "mystery".to_string(),
            ModelConfig {
                model_id: "mystery".to_string(),
                model_type: "not-a-catalog-id".to_string(),
                config: serde_json::json!({}),
                provider_id: "ollama".to_string(),
                variant: None,
            },
        );

        let source = ModelSource::from_config(&config);
        let err = source
            .get("mystery")
            .err()
            .expect("an unknown model_type must not construct")
            .to_string();
        assert!(err.contains("mystery"), "got: {err}");
    }

    #[test]
    fn get_builds_only_the_model_it_was_asked_for() {
        use crate::config::{Config, ModelConfig};

        let mut config = Config::default();
        for id in ["granite-3.1-8b-instruct", "granite-3.1-2b-instruct"] {
            config.models.insert(
                id.to_string(),
                ModelConfig {
                    model_id: id.to_string(),
                    model_type: id.to_string(),
                    config: serde_json::json!({}),
                    provider_id: "ollama".to_string(),
                    variant: None,
                },
            );
        }

        let source = ModelSource::from_config(&config);
        assert!(source.cached_ids().is_empty(), "nothing is built up front");
        source.get("granite-3.1-8b-instruct").unwrap();
        assert_eq!(
            source.cached_ids(),
            vec!["granite-3.1-8b-instruct".to_string()],
            "asking for one model must not drag in the other"
        );
    }

    #[test]
    fn instances_omits_a_model_that_cannot_be_built_and_keeps_the_rest() {
        use crate::config::{Config, ModelConfig};
        use crate::dependency::Configured;

        let mut config = Config::default();
        for (id, model_type) in [
            ("granite-3.1-8b-instruct", "granite-3.1-8b-instruct"),
            ("broken", "not-a-catalog-id"),
        ] {
            config.models.insert(
                id.to_string(),
                ModelConfig {
                    model_id: id.to_string(),
                    model_type: model_type.to_string(),
                    config: serde_json::json!({}),
                    provider_id: "ollama".to_string(),
                    variant: None,
                },
            );
        }

        let source = ModelSource::from_config(&config);
        // The healthy model is reachable on its own, without the broken one
        // being touched at all.
        assert!(source.get("granite-3.1-8b-instruct").is_ok());
        assert_eq!(source.cached_ids(), vec!["granite-3.1-8b-instruct"]);

        let ids: Vec<String> = source.instances().into_iter().map(|(id, _)| id).collect();
        assert_eq!(ids, vec!["granite-3.1-8b-instruct".to_string()]);
    }

    #[tokio::test]
    async fn take_routes_through_proxy_and_registers_a_route_when_a_handle_is_active() {
        use crate::config::{Config, ModelConfig, ProviderConfig};
        use crate::proxy::ProxyServer;

        async fn echo_model(body: axum::body::Bytes) -> axum::response::Response {
            use axum::response::IntoResponse;
            let value: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
            axum::Json(serde_json::json!({ "model": value.get("model") })).into_response()
        }
        let app = axum::Router::new().route("/echo", axum::routing::post(echo_model));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let mut config = Config::default();
        config.providers.insert(
            "ollama".to_string(),
            ProviderConfig {
                provider_id: "ollama".to_string(),
                provider_type: "ollama".to_string(),
                config: serde_json::json!({ "base_url": format!("http://{addr}") }),
            },
        );
        config.models.insert(
            "granite-3.1-8b-instruct".to_string(),
            ModelConfig {
                model_id: "granite-3.1-8b-instruct".to_string(),
                model_type: "granite-3.1-8b-instruct".to_string(),
                config: serde_json::json!({}),
                provider_id: "ollama".to_string(),
                variant: None,
            },
        );
        let server = ProxyServer::start().unwrap();
        config.model_proxy = Some(server.handle.clone());

        let source = ModelSource::from_config(&config);
        let model = source.get("granite-3.1-8b-instruct").unwrap();
        let provider = model.provider().unwrap();
        assert_eq!(provider.base_url(), server.handle.local_base_url);
        assert!(provider.api_key().is_none());

        // The real route (to the un-proxied fake upstream) was registered
        // under the model's catalog id (no alias, so it falls back to that)
        // -- prove it's actually live by round-tripping through the proxy.
        let client = reqwest::Client::new();
        let resp: serde_json::Value = client
            .post(format!("{}/echo", provider.base_url()))
            .json(&serde_json::json!({ "model": "granite-3.1-8b-instruct" }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(resp["model"], "granite-3.1-8b-instruct");

        server.shutdown().await;
    }

    #[test]
    fn test_all_models_registered() {
        let models = MODEL_REGISTRY.entries();
        assert!(!models.is_empty(), "Expected models to be registered");
    }

    #[test]
    fn test_get_specific_model() {
        let model = MODEL_REGISTRY.get("granite-3.1-8b-instruct");
        assert!(
            model.is_some(),
            "granite-3.1-8b-instruct should be registered"
        );

        let metadata = model.unwrap();
        assert_eq!(metadata.family, "Granite Language");
        assert_eq!(metadata.version, "3.1");
        assert_eq!(metadata.context_length, 131072);
        assert_eq!(metadata.model_type, ModelType::Text);
    }

    #[test]
    fn test_model_variants() {
        let model = MODEL_REGISTRY.get("granite-3.1-8b-instruct").unwrap();
        assert!(
            !model.variants.is_empty(),
            "granite-3.1-8b-instruct should have variants"
        );

        // Check first variant
        let variant = &model.variants[0];
        assert!(!variant.format.is_empty());
        assert!(!variant.precision.is_empty());
        assert!(variant.size_gb.unwrap_or(0.0) > 0.0);
    }

    #[test]
    fn test_all_model_ids() {
        let models = MODEL_REGISTRY.entries();
        let ids: Vec<&str> = models.keys().copied().collect();

        assert!(ids.contains(&"granite-3.1-8b-instruct"));
        assert!(ids.contains(&"granite-guardian-3.1-8b"));
    }

    #[test]
    fn test_model_types() {
        let text_model = MODEL_REGISTRY.get("granite-3.1-8b-instruct").unwrap();
        assert_eq!(text_model.model_type, ModelType::Text);

        let vision_model = MODEL_REGISTRY.get("granite-vision-3.3-2b").unwrap();
        assert_eq!(vision_model.model_type, ModelType::Vision);

        let speech_model = MODEL_REGISTRY.get("granite-speech-4.1-2b").unwrap();
        assert_eq!(speech_model.model_type, ModelType::Speech);
    }

    #[test]
    fn test_model_supported_functions() {
        let text_model = MODEL_REGISTRY.get("granite-3.1-8b-instruct").unwrap();
        assert!(
            text_model
                .supported_functions
                .contains(&ModelFunction::Chat)
        );

        let vision_model = MODEL_REGISTRY.get("granite-vision-3.3-2b").unwrap();
        assert!(
            vision_model
                .supported_functions
                .contains(&ModelFunction::Chat)
        );
        assert!(
            vision_model
                .supported_functions
                .contains(&ModelFunction::ImageUnderstanding)
        );

        let speech_model = MODEL_REGISTRY.get("granite-speech-4.1-2b").unwrap();
        assert!(
            speech_model
                .supported_functions
                .contains(&ModelFunction::Chat)
        );
        assert!(
            speech_model
                .supported_functions
                .contains(&ModelFunction::Transcription)
        );
    }
}
