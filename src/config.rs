//! Configuration keys available to `.odoo_lsp`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use tower_lsp_server::ls_types::DiagnosticSeverity;

use crate::xml::diagnostic_codes::XmlDiagnosticCode;

/// Configuration is changed via [`on_change_config`][crate::backend::Backend::on_change_config].
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Config {
	pub module: Option<ModuleConfig>,
	pub symbols: Option<SymbolsConfig>,
	pub references: Option<ReferencesConfig>,
	pub completions: Option<CompletionsConfig>,
	pub diagnostics: Option<DiagnosticsConfig>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct ModuleConfig {
	pub roots: Option<Vec<String>>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct SymbolsConfig {
	pub limit: Option<usize>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct ReferencesConfig {
	pub limit: Option<usize>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct CompletionsConfig {
	pub limit: Option<usize>,
}

/// Diagnostic severity level configuration.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DiagnosticLevel {
	/// Error severity
	Error,
	/// Warning severity
	Warning,
	/// Information severity
	Information,
	/// Hint severity
	Hint,
	/// Disable this diagnostic
	Off,
}

impl DiagnosticLevel {
	/// Convert to LSP DiagnosticSeverity, or None if disabled.
	pub fn to_severity(self) -> Option<DiagnosticSeverity> {
		match self {
			DiagnosticLevel::Error => Some(DiagnosticSeverity::ERROR),
			DiagnosticLevel::Warning => Some(DiagnosticSeverity::WARNING),
			DiagnosticLevel::Information => Some(DiagnosticSeverity::INFORMATION),
			DiagnosticLevel::Hint => Some(DiagnosticSeverity::HINT),
			DiagnosticLevel::Off => None,
		}
	}

	/// Create from LSP DiagnosticSeverity.
	pub fn from_severity(severity: DiagnosticSeverity) -> Self {
		match severity {
			DiagnosticSeverity::ERROR => DiagnosticLevel::Error,
			DiagnosticSeverity::WARNING => DiagnosticLevel::Warning,
			DiagnosticSeverity::INFORMATION => DiagnosticLevel::Information,
			DiagnosticSeverity::HINT => DiagnosticLevel::Hint,
			_ => DiagnosticLevel::Warning,
		}
	}
}

/// Configuration for diagnostics.
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct DiagnosticsConfig {
	/// Enable/disable all XML validation diagnostics.
	#[serde(default)]
	pub xml_validation: Option<bool>,

	/// Override severity for specific diagnostic codes.
	/// Keys are diagnostic codes (e.g., "OLS05001") and values are severity levels.
	#[serde(default)]
	pub severity_overrides: Option<HashMap<String, DiagnosticLevel>>,
}

impl DiagnosticsConfig {
	/// Check if XML validation is enabled (defaults to true).
	///
	/// Structural XML validation checks for common issues in Odoo data files
	/// like missing required attributes, invalid element structure, etc.
	/// Users can disable it in their `.odoo_lsp` config if needed.
	pub fn is_xml_validation_enabled(&self) -> bool {
		self.xml_validation.unwrap_or(true)
	}

	/// Get the severity for a diagnostic code, applying any overrides.
	pub fn get_severity(&self, code: XmlDiagnosticCode) -> Option<DiagnosticSeverity> {
		let code_str = code.code();

		// Check for override first
		if let Some(overrides) = &self.severity_overrides
			&& let Some(level) = overrides.get(&code_str)
		{
			return level.to_severity();
		}

		// Fall back to default severity
		Some(code.default_severity())
	}
}
