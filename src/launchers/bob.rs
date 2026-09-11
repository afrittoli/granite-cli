// Standard
use std::collections::HashSet;
use std::path::PathBuf;

// Third Party
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

// Local
use crate::capabilities::{Binding, BindingType, Capability, McpBinding};
use crate::launchers::base::HasLauncherMetadata as HasBobLauncherMetadata;
use crate::launchers::base::{EnvBinding, LaunchContext, Launcher, LauncherMetadata, run_command};
use crate::launchers::shared::mcp_cli::{
    mcp_binding_request, register_mcp_server, remove_mcp_server,
};
use crate::registry::ConfigConstructable;
use crate::utils::resolve_shell_command;
use crate::utils::ui::Ui;

/*-- public --*/

#[derive(Debug, Clone, Serialize, Deserialize, Default, schemars::JsonSchema)]
pub struct BobLauncherConfig {
    /// Override path to the `bob` binary for non-PATH installs.
    /// Leave unset to use PATH lookup.
    #[serde(default)]
    pub command_path: Option<String>,
}

pub struct BobLauncher {
    instance_id: String,
    config: BobLauncherConfig,
    /// `(server_name, binding)` for every MCP-capable capability bound to
    /// this launcher, registered/removed around `run_command` in `launch()`.
    bound_mcp_bindings: Vec<(String, McpBinding)>,
}

impl ConfigConstructable for BobLauncher {
    type Config = BobLauncherConfig;

    fn new(instance_id: &str, cfg: &serde_json::Value) -> Self {
        let config: BobLauncherConfig = serde_json::from_value(cfg.clone()).unwrap_or_default();
        Self {
            instance_id: instance_id.to_string(),
            config,
            bound_mcp_bindings: vec![],
        }
    }
}

impl crate::registry::Named for BobLauncher {
    fn instance_id(&self) -> &str {
        &self.instance_id
    }
}

#[async_trait]
impl Launcher for BobLauncher {
    fn name(&self) -> &str {
        "Bob CLI"
    }

    fn command(&self) -> &str {
        self.config.command_path.as_deref().unwrap_or("bob")
    }

    async fn bind_capability(&mut self, capability: &dyn Capability) -> anyhow::Result<()> {
        let supported = Self::metadata().supported_capabilities;
        let capability_types = capability.binding_types();
        if !capability_types.is_subset(&supported) {
            anyhow::bail!(
                "capability supports {:?} which this launcher does not support",
                capability_types.difference(&supported).collect::<Vec<_>>()
            );
        }

        let binding = capability.bind(mcp_binding_request()).await?;
        match binding {
            Binding::Mcp(binding) => {
                self.bound_mcp_bindings
                    .push((capability.instance_id().to_string(), binding));
            }
            other => anyhow::bail!("expected an Mcp binding, got {:?}", other.binding_type()),
        }
        Ok(())
    }

    fn validate_command(&self) -> anyhow::Result<PathBuf> {
        resolve_shell_command(&self.config.command_path, "bob")
    }

    async fn env_overlay(&self, _ctx: &LaunchContext) -> anyhow::Result<Vec<EnvBinding>> {
        Ok(vec![])
    }

    /// Registers each bound MCP server with `bob mcp add-json` (scoped to
    /// this workspace) before exec'ing, and best-effort removes them again
    /// afterwards.
    async fn launch(
        &self,
        args: &[String],
        ctx: &LaunchContext,
        ui: &dyn Ui,
    ) -> anyhow::Result<std::process::ExitStatus> {
        let binary = self.validate_command()?;
        let overlay = self.env_overlay(ctx).await?;

        const SCOPE: &[&str] = &["-s", "workspace"];
        for (name, binding) in &self.bound_mcp_bindings {
            register_mcp_server(&binary, name, binding, SCOPE, ctx, ui)?;
        }

        let result = run_command(binary.clone(), &overlay, args, ctx, ui).await;

        for (name, _) in &self.bound_mcp_bindings {
            remove_mcp_server(&binary, name, SCOPE, ctx, ui);
        }

        result
    }
}

impl HasBobLauncherMetadata for BobLauncher {
    fn metadata() -> LauncherMetadata {
        LauncherMetadata {
            name: "Bob CLI".to_string(),
            description: "IBM Bob AI assistant CLI".to_string(),
            default_command: "bob".to_string(),
            supported_capabilities: HashSet::from([BindingType::Mcp]),
            tags: vec!["bob".to_string(), "ibm".to_string()],
        }
    }
}

/*-- tests --*/

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_defaults_to_bob() {
        let l = BobLauncher::new("my-bob", &serde_json::json!({}));
        assert_eq!(l.command(), "bob");
    }

    #[test]
    fn command_uses_explicit_path_when_set() {
        let l = BobLauncher::new(
            "my-bob",
            &serde_json::json!({
                "command_path": "/opt/bin/bob"
            }),
        );
        assert_eq!(l.command(), "/opt/bin/bob");
    }

    #[test]
    fn validate_command_err_for_nonexistent_explicit_path() {
        let l = BobLauncher::new(
            "my-bob",
            &serde_json::json!({
                "command_path": "/no/such/path/bob"
            }),
        );
        assert!(l.validate_command().is_err());
    }

    #[test]
    fn validate_command_falls_back_to_path_for_bare_command_name() {
        let l = BobLauncher::new(
            "my-bob",
            &serde_json::json!({
                "command_path": "ls"
            }),
        );
        assert!(l.validate_command().is_ok());
    }

    #[test]
    fn metadata_name_is_bob_cli() {
        let meta = BobLauncher::metadata();
        assert_eq!(meta.name, "Bob CLI");
        assert_eq!(meta.default_command, "bob");
    }

    #[test]
    fn config_schema_is_present() {
        use crate::launchers::base::LauncherFactory;
        let mut factory = LauncherFactory::new();
        factory.register::<BobLauncher>("bob");
        let schema = factory.config_schema("bob").unwrap();
        let props = schema.get("properties").and_then(|p| p.as_object());
        assert!(props.is_some());
        assert!(props.unwrap().contains_key("command_path"));
    }
}
