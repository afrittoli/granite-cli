// Standard
use std::collections::HashMap;
use std::sync::LazyLock;

// Third Party
use alog::{MessageLevel, alog_channel, use_channel};

use_channel!("CAPBL");

pub static CAPABILITY_REGISTRY: LazyLock<base::CapabilityFactory> = LazyLock::new(|| {
    let mut factory = base::CapabilityFactory::new();
    factory.register::<agent_model::AgentModelCapability>("agent-model");
    factory.register::<vision_mcp::VisionMCPCapability>("vision-mcp");
    factory.register::<sub_agent::SubAgentCapability>("sub-agent");
    factory.register::<sub_agent_code::CodeSubAgentCapability>("sub-agent-code");
    factory.register::<sub_agent_explore::ExploreSubAgentCapability>("sub-agent-explore");
    factory.register::<sub_agent_plan::PlanSubAgentCapability>("sub-agent-plan");
    factory
});

/*-- CapabilitySource -----------------------------------------------------------*/

/// The real `Configured<dyn Capability>`: builds a live capability instance
/// the first time one is asked for by its instance nickname
/// (`capability_id`) rather than its catalog type (`capability_type`). The
/// instance is kept, so every later ask for that id returns the same object.
pub struct CapabilitySource {
    /// The configuration this source was built from. Narrows to
    /// `HashMap<String, CapabilityConfig>` once `construct` stops taking the
    /// whole configuration.
    config: crate::config::Config,
    cache: std::sync::Mutex<HashMap<String, std::sync::Arc<dyn Capability>>>,
}

impl CapabilitySource {
    pub fn from_config(config: &crate::config::Config) -> Self {
        Self {
            config: config.clone(),
            cache: std::sync::Mutex::new(HashMap::new()),
        }
    }

    /// The capability configured under `capability_id`, built on the first
    /// ask and returned from the cache on every one after it. Errors when no
    /// entry is configured under that id, when its references do not resolve,
    /// or when its `capability_type` is not in the registry.
    pub fn get(&self, capability_id: &str) -> anyhow::Result<std::sync::Arc<dyn Capability>> {
        if let Some(built) = self.cache.lock().unwrap().get(capability_id) {
            return Ok(built.clone());
        }
        let capability_config = self
            .config
            .capabilities
            .get(capability_id)
            .ok_or_else(|| anyhow::anyhow!("capability '{capability_id}' is not configured"))?;

        // A capability whose references do not resolve cannot bind:
        // `model.provider()` fails without a provider, and a model that is
        // gone panics inside `ConfiguredModel::resolve` (#90) before
        // construction can even report the failure. Refuse it rather than
        // reach either.
        crate::config::validation::validate_ref(
            crate::config::validation::RefKind::Capability,
            &capability_config.capability_id,
            &self.config,
        )
        .map_err(|e| anyhow::anyhow!("Skipping capability '{capability_id}': {e}"))?;

        let built = CAPABILITY_REGISTRY
            .construct(
                &capability_config.capability_type,
                &capability_config.capability_id,
                &capability_config.config,
                &self.config,
            )
            .map_err(|e| {
                anyhow::anyhow!("could not construct capability '{capability_id}': {e}")
            })?;
        let built: std::sync::Arc<dyn Capability> = std::sync::Arc::from(built);
        self.cache
            .lock()
            .unwrap()
            .insert(capability_id.to_string(), built.clone());
        Ok(built)
    }
}

impl crate::dependency::Configured<dyn Capability> for CapabilitySource {
    fn instances(&self) -> Vec<(String, std::sync::Arc<dyn Capability + 'static>)> {
        self.config
            .capabilities
            .keys()
            .filter_map(|id| match self.get(id) {
                Ok(capability) => Some((id.clone(), capability)),
                Err(e) => {
                    alog_channel!(MessageLevel::Warning, "{e}");
                    None
                }
            })
            .collect()
    }

    fn catalog(&self) -> HashMap<&'static str, CapabilityMetadata> {
        CAPABILITY_REGISTRY.entries()
    }

    fn config_schema(&self, type_name: &str) -> Option<schemars::Schema> {
        CAPABILITY_REGISTRY.config_schema(type_name)
    }
}

/*-- Module Declarations -----------------------------------------------------*/

mod base;
pub use crate::providers::ApiType;
pub use base::{
    AgentModelBinding, AgentModelBindingRequest, Binding, BindingRequest, BindingType, Capability,
    CapabilityMetadata, Dependency, EnvBinding, KnownSubAgent, LaunchContext, McpBinding,
    McpBindingRequest, McpTransportKind, SubAgentBinding, SubAgentBindingRequest, ToolName,
};

mod requirement;
pub use requirement::{ModelRequirement, ProviderRequirement, ShellCommandRequirement};

mod agent_model;
pub use agent_model::{AgentModelCapability, AgentModelCapabilityConfig};

mod vision_mcp;
pub use vision_mcp::{VisionMCPCapability, VisionMCPCapabilityConfig};

mod sub_agent;
pub use sub_agent::{SubAgentCapability, SubAgentCapabilityConfig};

mod sub_agent_code;
pub use sub_agent_code::{CodeSubAgentCapability, CodeSubAgentCapabilityConfig};

mod sub_agent_explore;
pub use sub_agent_explore::{ExploreSubAgentCapability, ExploreSubAgentCapabilityConfig};

mod sub_agent_plan;
pub use sub_agent_plan::{PlanSubAgentCapability, PlanSubAgentCapabilityConfig};

/*-- tests ---------------------------------------------------------------------*/

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CapabilityConfig, Config, ModelConfig, ProviderConfig};
    use crate::dependency::Configured;

    fn agent_model_config(id: &str, model_key: &str) -> CapabilityConfig {
        CapabilityConfig {
            capability_id: id.to_string(),
            capability_type: "agent-model".to_string(),
            config: serde_json::json!({
                "model_id": model_key,
            }),
        }
    }

    #[test]
    fn capability_source_constructs_one_instance_per_named_capability() {
        let mut config = Config::default();
        // The model needs a provider to bind, so the capability is only
        // constructible with one configured.
        config.providers.insert(
            "ollama".to_string(),
            ProviderConfig {
                provider_id: "ollama".to_string(),
                provider_type: "ollama".to_string(),
                config: serde_json::json!({}),
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
        config.capabilities.insert(
            "chat".to_string(),
            agent_model_config("chat", "granite-3.1-8b-instruct"),
        );

        let source = CapabilitySource::from_config(&config);
        let ids: Vec<String> = source.instances().into_iter().map(|(id, _)| id).collect();
        assert_eq!(ids, vec!["chat".to_string()]);
    }

    #[test]
    fn capability_source_skips_unknown_capability_types() {
        let mut config = Config::default();
        config.capabilities.insert(
            "bogus".to_string(),
            CapabilityConfig {
                capability_id: "bogus".to_string(),
                capability_type: "not-a-real-capability".to_string(),
                config: serde_json::json!({}),
            },
        );

        let source = CapabilitySource::from_config(&config);
        assert!(source.instances().is_empty());
    }

    #[test]
    fn capability_source_skips_a_capability_whose_model_is_gone() {
        let mut config = Config::default();
        config.capabilities.insert(
            "chat".to_string(),
            agent_model_config("chat", "granite-3.1-8b-instruct"),
        );

        // No model entry, so constructing `chat` would panic inside
        // ConfiguredModel::resolve. It is skipped before reaching that.
        let source = CapabilitySource::from_config(&config);
        assert!(source.instances().is_empty());
    }

    #[test]
    fn capability_registry_has_agent_model() {
        assert!(CAPABILITY_REGISTRY.get("agent-model").is_some());
    }

    #[test]
    fn capability_registry_has_sub_agent() {
        assert!(CAPABILITY_REGISTRY.get("sub-agent").is_some());
    }

    #[test]
    fn capability_registry_has_sub_agent_plan() {
        assert!(CAPABILITY_REGISTRY.get("sub-agent-plan").is_some());
    }
}
