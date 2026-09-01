use std::path::PathBuf;

use super::RuntimePlan;

/// A fully compiled configuration candidate and the repository state that
/// produced it. The engine installs both only after the plan is accepted.
pub struct ConfigurationCandidate {
    pub plan: RuntimePlan,
    pub repository: Box<dyn ConfigurationRepository>,
    pub source_path: Option<PathBuf>,
}

/// Runtime-owned port for configuration persistence.
///
/// Implementations live at the application boundary. Runtime deliberately
/// knows neither TOML nor the concrete configuration document.
pub trait ConfigurationRepository: Send {
    fn source_text(&self) -> Result<String, String>;

    fn source_path(&self) -> Option<PathBuf>;

    fn reload_candidate(&self) -> Result<ConfigurationCandidate, String>;

    fn set_candidate(&self, path: &str, value: &str) -> Result<ConfigurationCandidate, String>;
}
