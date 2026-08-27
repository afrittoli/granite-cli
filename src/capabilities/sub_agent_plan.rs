//! `PlanSubAgentCapability`: defines a named planning sub-agent with a static
//! prompt and fixed tool allow-list (FileRead, Search, FileSearch, Shell,
//! WebFetch, WebSearch -- everything read-only, no FileWrite/FileEdit), and a
//! `Model`/`Provider` of its own. Mirrors `ExploreSubAgentCapability`.

// Standard
use serde_valid::Validate;
use std::collections::HashSet;

// Third Party
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

// Local
use crate::capabilities::ModelRequirement;
use crate::capabilities::base::{
    AgentModelBinding, Binding, BindingRequest, BindingType, Capability, CapabilityMetadata,
    Dependency, HasCapabilityMetadata, KnownSubAgent, SubAgentBinding, SubAgentBindingRequest,
    ToolName,
};
use crate::models::{ConfiguredModel, ConfiguredModelResolution, ModelFunction};
use crate::registry::ConfigConstructable;

/*-- PlanSubAgentCapabilityConfig --------------------------------------------*/

/// Configuration for the plan sub-agent capability. Unlike `SubAgentCapability`,
/// the prompt and tools are static (not configurable via JSON), leaving only the
/// description (what the sub-agent does) and model_id as configurable.
#[derive(Debug, Clone, Serialize, Deserialize, Default, schemars::JsonSchema, Validate)]
pub struct PlanSubAgentCapabilityConfig {
    /// Shown to the main agent so it can decide when to delegate to this
    /// plan sub-agent.
    #[validate(min_length = 1)]
    pub description: String,
    /// Key into the configured models map (the user-chosen instance ID) for
    /// the model this sub-agent runs on.
    #[validate(min_length = 1)]
    pub model_id: String,
}

/*-- PlanSubAgentCapability -----------------------------------------------*/

const PLAN_PROMPT: &str = "You are a software architect and planning specialist. Your role is to explore the codebase and design implementation plans.

=== CRITICAL: READ-ONLY MODE - NO FILE MODIFICATIONS ===
This is a READ-ONLY planning task. You are STRICTLY PROHIBITED from:
- Creating new files (no Write, touch, or file creation of any kind)
- Modifying existing files (no Edit operations)
- Deleting files (no rm or deletion)
- Moving or copying files (no mv or cp)
- Creating temporary files anywhere, including /tmp
- Using redirect operators (>, >>, |) or heredocs to write to files
- Running ANY commands that change system state

Your role is EXCLUSIVELY to explore the codebase and design implementation plans. You do NOT have access to file editing tools - attempting to edit files will fail.

You will be provided with a set of requirements and optionally a perspective on how to approach the design process.

## Your Process

1. **Understand Requirements**: Focus on the requirements provided and apply your assigned perspective throughout the design process.

2. **Explore Thoroughly**:
   - Read any files provided to you in the initial prompt
   - Find existing patterns and conventions using `find`, `grep`, file search / glob, and search
   - Understand the current architecture
   - Identify similar features as reference
   - Trace through relevant code paths
   - Use the shell tool ONLY for read-only operations (`ls, git status, git log, git diff, find, grep, cat, head, tail, git status, git log, git diff`
   - NEVER use the shell tool for: `mkdir, touch, rm, cp, mv, git add, git commit, npm install, pip install, git add, git commit, npm install, pip install`, or any file creation/modification

3. **Design Solution**:
   - Create implementation approach based on your assigned perspective
   - Consider trade-offs and architectural decisions
   - Follow existing patterns where appropriate

4. **Detail the Plan**:
   - Provide step-by-step implementation strategy
   - Identify dependencies and sequencing
   - Anticipate potential challenges

## Required Output

End your response with:

### Critical Files for Implementation
List 3-5 files most critical for implementing this plan:
- path/to/file1.ts
- path/to/file2.ts
- path/to/file3.ts

REMEMBER: You can ONLY explore and plan. You CANNOT and MUST NOT write, edit, or modify any files. You do NOT have access to file editing tools.";

pub struct PlanSubAgentCapability {
    instance_id: String,
    config: PlanSubAgentCapabilityConfig,
    /// `Err` when `config.model_id` doesn't resolve (e.g. the model was
    /// removed after this capability was configured). Construction stays
    /// infallible per `ConfigConstructable`; the error is surfaced at
    /// `bind()` time and via `Capability::is_healthy`.
    configured_model: Result<ConfiguredModel, String>,
    /// Static prompt for the plan sub-agent (placeholder for now; user can
    /// override by editing this field later).
    pub prompt: String,
    /// Static tool allow-list for the plan sub-agent.
    pub tools: Vec<ToolName>,
}

impl ConfigConstructable for PlanSubAgentCapability {
    type Config = PlanSubAgentCapabilityConfig;

    /// Constructs the capability by resolving its model through
    /// `ConfiguredModel`, exactly like `AgentModelCapability::new` -- so
    /// `model.provider()` works at bind time and, when a usage-tracking
    /// session is active, the model is transparently tracked.
    fn new(
        instance_id: &str,
        cfg: &serde_json::Value,
        global_config: &crate::config::Config,
    ) -> Self {
        let config: PlanSubAgentCapabilityConfig =
            serde_json::from_value(cfg.clone()).unwrap_or_default();
        let configured_model = ConfiguredModel::resolve(&config.model_id, global_config);

        // Static prompt
        // TODO: Make prompt a template that should be expanded by the launcher
        let prompt = PLAN_PROMPT.to_string();
        // Static tools: everything read-only (no FileWrite/FileEdit)
        let tools = vec![
            ToolName::FileRead,
            ToolName::Search,
            ToolName::FileSearch,
            ToolName::Shell,
            ToolName::WebFetch,
            ToolName::WebSearch,
        ];

        Self {
            instance_id: instance_id.to_string(),
            config,
            configured_model,
            prompt,
            tools,
        }
    }
}

impl crate::registry::Named for PlanSubAgentCapability {
    fn instance_id(&self) -> &str {
        &self.instance_id
    }
}

#[async_trait]
impl Capability for PlanSubAgentCapability {
    fn name(&self) -> &str {
        "Plan Sub-Agent"
    }

    fn description(&self) -> &str {
        "Defines a named planning sub-agent (static prompt, fixed read-only tools, and model) that a launched coding agent can delegate implementation-plan design to."
    }

    fn dependencies(&self) -> Vec<Dependency> {
        vec![Dependency::Model {
            config_key: "model_id".to_string(),
            requirement: ModelRequirement {
                supported_functions: vec![ModelFunction::Chat, ModelFunction::ToolCalling],
                ..Default::default()
            },
            resolved_id: Some(self.config.model_id.clone()),
            required: true,
        }]
    }

    fn binding_types(&self) -> HashSet<BindingType> {
        HashSet::from([BindingType::SubAgent])
    }

    async fn bind(&self, request: BindingRequest) -> anyhow::Result<Binding> {
        let api_type = match request {
            BindingRequest::SubAgent(SubAgentBindingRequest { api_type }) => api_type,
            other => anyhow::bail!(
                "PlanSubAgentCapability does not handle {:?} binding requests",
                other.binding_type()
            ),
        };
        let model_id = &self.config.model_id;
        let configured_model = self.configured_model.resolved()?;

        let (provider, endpoint, model_name) = configured_model.resolve_provider_endpoint(
            model_id,
            api_type.clone(),
            ModelFunction::Chat,
            ModelFunction::Chat,
        )?;

        Ok(Binding::SubAgent(SubAgentBinding {
            description: self.config.description.clone(),
            prompt: self.prompt.clone(),
            tools: self.tools.clone(),
            model: AgentModelBinding {
                api_type,
                provider_name: provider.instance_id().to_string(),
                base_url: provider.base_url().to_string(),
                model_name,
                endpoint_path: endpoint.path().to_string(),
                api_key: provider.api_key().cloned(),
                verify_ssl: provider.verify_ssl(),
                context_length: Some(configured_model.model.context_length()),
            },
            known_type: Some(KnownSubAgent::Plan),
        }))
    }

    fn is_healthy(&self) -> Result<(), String> {
        self.configured_model.health()
    }
}

impl HasCapabilityMetadata for PlanSubAgentCapability {
    fn metadata() -> CapabilityMetadata {
        CapabilityMetadata {
            name: "Plan Sub-Agent".to_string(),
            description: "Defines a named planning sub-agent (static prompt, fixed read-only tools, and model) that a launched coding agent can delegate implementation-plan design to.".to_string(),
            dependencies: vec![Dependency::Model {
                config_key: "model_id".to_string(),
                requirement: ModelRequirement {
                    supported_functions: vec![ModelFunction::Chat, ModelFunction::ToolCalling],
                    ..Default::default()
                },
                resolved_id: None,
                required: true,
            }],
            tags: vec!["agent".to_string(), "plan".to_string()],
            supported_binding_types: HashSet::from([BindingType::SubAgent]),
        }
    }
}

/*-- tests -------------------------------------------------------------------*/

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, ModelConfig};
    use crate::models::Model;
    use crate::providers::{
        ApiEndpoint, ApiType, HealthStatus, ModelFormat, Provider, ProviderError,
    };
    use crate::registry::Secret;
    use std::collections::HashMap;
    use std::sync::Arc;

    #[derive(Clone, Default)]
    struct FakeProvider {
        instance_id: String,
        base_url: String,
        api_key: Option<Secret>,
        verify_ssl: bool,
        api_types: Vec<ApiType>,
        endpoints: HashMap<ModelFunction, Vec<ApiEndpoint>>,
        alias: Option<String>,
    }

    impl ConfigConstructable for FakeProvider {
        type Config = crate::registry::NoConfig;
        fn new(_: &str, _: &serde_json::Value, _: &crate::config::Config) -> Self {
            unimplemented!("not used in tests")
        }
    }

    impl crate::registry::Named for FakeProvider {
        fn instance_id(&self) -> &str {
            &self.instance_id
        }
    }

    #[async_trait]
    impl Provider for FakeProvider {
        fn name(&self) -> &str {
            "Fake Provider"
        }
        fn function_endpoints(&self) -> HashMap<ModelFunction, Vec<ApiEndpoint>> {
            self.endpoints.clone()
        }
        fn supported_api_types(&self) -> Vec<ApiType> {
            self.api_types.clone()
        }
        fn base_url(&self) -> &str {
            &self.base_url
        }
        fn api_key(&self) -> Option<&Secret> {
            self.api_key.as_ref()
        }
        fn verify_ssl(&self) -> bool {
            self.verify_ssl
        }
        fn supported_formats(&self) -> Vec<ModelFormat> {
            vec![]
        }
        fn model_alias(&self, _variant: Option<&crate::models::ModelVariant>) -> Option<String> {
            self.alias.clone()
        }
        async fn health_check(&self) -> Result<HealthStatus, ProviderError> {
            unimplemented!("not used in tests")
        }
    }

    fn ok_provider() -> FakeProvider {
        let mut endpoints = HashMap::new();
        endpoints.insert(
            ModelFunction::Chat,
            vec![ApiEndpoint::OpenAIChat, ApiEndpoint::AnthropicMessages],
        );
        FakeProvider {
            instance_id: "my-ollama".to_string(),
            base_url: "http://localhost:11434".to_string(),
            api_key: None,
            verify_ssl: true,
            api_types: vec![ApiType::OpenAI, ApiType::Anthropic],
            endpoints,
            alias: None,
        }
    }

    struct TestModel {
        supported_functions: Vec<ModelFunction>,
        provider: FakeProvider,
    }

    impl ConfigConstructable for TestModel {
        type Config = crate::registry::NoConfig;
        fn new(_: &str, _: &serde_json::Value, _: &crate::config::Config) -> Self {
            unimplemented!("not used in tests")
        }
    }

    impl crate::registry::Named for TestModel {
        fn instance_id(&self) -> &str {
            "granite-3.1-8b-instruct"
        }
    }

    impl Model for TestModel {
        fn family(&self) -> &str {
            "Test"
        }
        fn version(&self) -> &str {
            "1.0"
        }
        fn size(&self) -> u64 {
            1
        }
        fn context_length(&self) -> u64 {
            4096
        }
        fn model_type(&self) -> &crate::models::ModelType {
            &crate::models::ModelType::Text
        }
        fn huggingface_repo(&self) -> &str {
            "test/test"
        }
        fn native_dtype(&self) -> &str {
            "bfloat16"
        }
        fn architecture(&self) -> &crate::models::ModelArchitecture {
            unimplemented!("not used in tests")
        }
        fn variants(&self) -> &[crate::models::ModelVariant] {
            &[]
        }
        fn description(&self) -> Option<&str> {
            None
        }
        fn tags(&self) -> &[String] {
            &[]
        }
        fn supported_functions(&self) -> &[ModelFunction] {
            &self.supported_functions
        }
        fn provider(&self) -> anyhow::Result<Box<dyn Provider>> {
            Ok(Box::new(self.provider.clone()))
        }
    }

    /// Builds a `PlanSubAgentCapability` with a real registry model id (so
    /// construction succeeds) and then swaps in a test double model/provider,
    /// mirroring the test pattern from `sub_agent_explore.rs`.
    fn plan_capability_with_test_model(
        functions: Vec<ModelFunction>,
        provider: FakeProvider,
    ) -> PlanSubAgentCapability {
        let mut config = Config::default();
        config.models.insert(
            "granite-3.1-8b-instruct".to_string(),
            ModelConfig {
                model_id: "granite-3.1-8b-instruct".to_string(),
                provider_id: None,
                variant: None,
            },
        );
        let cap = PlanSubAgentCapability::new(
            "planner",
            &serde_json::json!({
                "description": "Plans changes",
                "model_id": "granite-3.1-8b-instruct",
            }),
            &config,
        );
        PlanSubAgentCapability {
            instance_id: cap.instance_id,
            config: cap.config,
            configured_model: Ok(crate::models::ConfiguredModel::for_test(
                Arc::new(TestModel {
                    supported_functions: functions,
                    provider,
                }),
                None,
            )),
            prompt: cap.prompt,
            tools: cap.tools,
        }
    }

    fn request(api_type: ApiType) -> BindingRequest {
        BindingRequest::SubAgent(SubAgentBindingRequest { api_type })
    }

    #[tokio::test]
    async fn bind_succeeds_and_carries_description_prompt_and_tools() {
        let mut config = Config::default();
        config.models.insert(
            "granite-3.1-8b-instruct".to_string(),
            ModelConfig {
                model_id: "granite-3.1-8b-instruct".to_string(),
                provider_id: None,
                variant: None,
            },
        );
        let cap = PlanSubAgentCapability::new(
            "planner",
            &serde_json::json!({
                "description": "Plans changes",
                "model_id": "granite-3.1-8b-instruct",
            }),
            &config,
        );
        let cap = PlanSubAgentCapability {
            instance_id: cap.instance_id,
            config: cap.config,
            configured_model: Ok(crate::models::ConfiguredModel::for_test(
                Arc::new(TestModel {
                    supported_functions: vec![ModelFunction::Chat],
                    provider: ok_provider(),
                }),
                None,
            )),
            prompt: cap.prompt,
            tools: cap.tools,
        };

        let binding = cap.bind(request(ApiType::Anthropic)).await.unwrap();
        let Binding::SubAgent(binding) = binding else {
            panic!("expected SubAgent binding")
        };
        assert_eq!(binding.description, "Plans changes");
        assert_eq!(binding.prompt, PLAN_PROMPT.to_string());
        assert_eq!(
            binding.tools,
            vec![
                ToolName::FileRead,
                ToolName::Search,
                ToolName::FileSearch,
                ToolName::Shell,
                ToolName::WebFetch,
                ToolName::WebSearch,
            ]
        );
        assert_eq!(binding.model.base_url, "http://localhost:11434");
        assert_eq!(binding.model.model_name, "granite-3.1-8b-instruct");
        assert_eq!(binding.model.api_type, ApiType::Anthropic);
    }

    #[test]
    fn binding_types_reports_sub_agent() {
        let cap = plan_capability_with_test_model(vec![ModelFunction::Chat], ok_provider());
        assert_eq!(cap.binding_types(), HashSet::from([BindingType::SubAgent]));
    }

    #[test]
    fn dependencies_carry_resolved_model_id() {
        let cap = plan_capability_with_test_model(vec![ModelFunction::Chat], ok_provider());
        let deps = cap.dependencies();
        assert_eq!(deps.len(), 1);
        assert!(deps.iter().any(|d| matches!(
            d,
            Dependency::Model { resolved_id: Some(id), .. } if id == "granite-3.1-8b-instruct"
        )));
    }

    #[test]
    fn metadata_reports_supported_binding_types_and_wildcard_dependency() {
        let meta = PlanSubAgentCapability::metadata();
        assert_eq!(
            meta.supported_binding_types,
            HashSet::from([BindingType::SubAgent])
        );
        assert!(meta.dependencies.iter().any(|d| matches!(
            d,
            Dependency::Model {
                resolved_id: None,
                ..
            }
        )));
    }
}
