/// The current Codex CLI version as embedded at compile time.
#[cfg(not(test))]
pub const CODEX_CLI_VERSION: &str = env!("CARGO_PKG_VERSION");

// Upstream's UI snapshots use the unstamped development version, including its display width.
// Keep release-tag unit tests reproducible without changing the installed binary's version.
#[cfg(test)]
pub const CODEX_CLI_VERSION: &str = "0.0.0";
