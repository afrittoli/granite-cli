pub mod capabilities;
pub mod hardware;
// Callers of the install module arrive in spec 0028 sub-tasks 2 to 4.
#[allow(dead_code)]
pub(crate) mod install;
pub mod shell;
pub mod subserver;
pub mod traits;
pub mod ui;

#[cfg(test)]
pub(crate) mod test_support;

pub use capabilities::capability_model_ids;
pub use hardware::{HardwareProfile, detect_hardware};
pub use shell::resolve_shell_command;
pub use traits::Searchable;
pub use ui::prompt_from_schema;
