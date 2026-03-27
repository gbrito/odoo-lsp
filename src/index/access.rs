//! Index for Odoo security access rules (ir.model.access.csv).
//!
//! This module provides indexing and querying capabilities for Odoo's access control
//! system defined in ir.model.access.csv files.

use std::collections::HashSet;

use crate::model::{ModelIndex, ModelName};
use crate::prelude::*;
use dashmap::DashMap;
use smart_default::SmartDefault;

use super::{ModuleName, RecordId, Symbol, _G, _I, _R};

/// Represents a single access rule from ir.model.access.csv.
///
/// CSV format: id,name,model_id:id,group_id:id,perm_read,perm_write,perm_create,perm_unlink
#[derive(Debug, Clone)]
pub struct AccessRule {
	/// The XML ID of this access rule (e.g., "access_sale_order_user")
	pub id: ImStr,
	/// The module that defines this rule
	pub module: ModuleName,
	/// Human-readable name (e.g., "sale.order user")
	pub name: ImStr,
	/// The model being secured (e.g., "sale.order" from "model_sale_order")
	pub model_id: ModelName,
	/// The group that has this access (None means public access)
	pub group_id: Option<RecordId>,
	/// Read permission
	pub perm_read: bool,
	/// Write permission
	pub perm_write: bool,
	/// Create permission
	pub perm_create: bool,
	/// Delete permission
	pub perm_unlink: bool,
	/// Location in the CSV file
	pub location: MinLoc,
	/// Whether this rule has been deleted (for incremental updates)
	pub deleted: bool,
}

impl AccessRule {
	/// Returns the qualified XML ID (module.id)
	pub fn qualified_id(&self) -> String {
		format!("{}.{}", _R(self.module), self.id)
	}

	/// Returns a human-readable permission string like "CRUD" or "R---"
	pub fn permission_string(&self) -> String {
		format!(
			"{}{}{}{}",
			if self.perm_read { "R" } else { "-" },
			if self.perm_write { "W" } else { "-" },
			if self.perm_create { "C" } else { "-" },
			if self.perm_unlink { "D" } else { "-" },
		)
	}
}

/// Type-safe symbol for access rules
pub type AccessRuleId = Symbol<AccessRule>;

/// Index for security access rules.
///
/// Provides efficient lookups by:
/// - Qualified ID (module.id)
/// - Model name (to find all rules for a model)
/// - Group ID (to find all rules for a group)
#[derive(SmartDefault)]
pub struct AccessIndex {
	/// Primary storage: qualified_id -> AccessRule
	#[default(_code = "DashMap::with_shard_amount(4)")]
	inner: DashMap<AccessRuleId, AccessRule>,
	/// Index by model: model_name -> set of access rule IDs
	#[default(_code = "DashMap::with_shard_amount(4)")]
	by_model: DashMap<ModelName, HashSet<AccessRuleId>>,
	/// Index by group: group_id -> set of access rule IDs
	#[default(_code = "DashMap::with_shard_amount(4)")]
	by_group: DashMap<RecordId, HashSet<AccessRuleId>>,
	/// Index by module: module_name -> set of access rule IDs
	#[default(_code = "DashMap::with_shard_amount(4)")]
	by_module: DashMap<ModuleName, HashSet<AccessRuleId>>,
}

impl AccessIndex {
	/// Insert an access rule into the index.
	pub fn insert(&self, rule: AccessRule) {
		let qualified_id: AccessRuleId = _I(rule.qualified_id()).into();

		// Skip if already exists
		if self.inner.contains_key(&qualified_id) {
			return;
		}

		// Update secondary indexes
		self.by_model.entry(rule.model_id).or_default().insert(qualified_id);

		if let Some(group_id) = rule.group_id {
			self.by_group.entry(group_id).or_default().insert(qualified_id);
		}

		self.by_module.entry(rule.module).or_default().insert(qualified_id);

		// Insert into primary storage
		self.inner.insert(qualified_id, rule);
	}

	/// Append multiple access rules to the index.
	pub fn append(&self, rules: impl IntoIterator<Item = AccessRule>) {
		for rule in rules {
			self.insert(rule);
		}
	}

	/// Get an access rule by its qualified ID.
	pub fn get(&self, id: &AccessRuleId) -> Option<dashmap::mapref::one::Ref<'_, AccessRuleId, AccessRule>> {
		self.inner.get(id)
	}

	/// Find all access rules for a given model.
	pub fn by_model(&self, model: &ModelName) -> Vec<AccessRuleId> {
		self.by_model
			.get(model)
			.map(|ids| ids.iter().cloned().collect())
			.unwrap_or_default()
	}

	/// Find all access rules for a given group.
	pub fn by_group(&self, group: &RecordId) -> Vec<AccessRuleId> {
		self.by_group
			.get(group)
			.map(|ids| ids.iter().cloned().collect())
			.unwrap_or_default()
	}

	/// Find all access rules defined in a module.
	pub fn by_module(&self, module: &ModuleName) -> Vec<AccessRuleId> {
		self.by_module
			.get(module)
			.map(|ids| ids.iter().cloned().collect())
			.unwrap_or_default()
	}

	/// Check if a model has any access rules defined.
	pub fn has_rules_for_model(&self, model: &ModelName) -> bool {
		self.by_model.get(model).is_some_and(|ids| !ids.is_empty())
	}

	/// Check if a module has any access rules defined at all.
	/// Used to determine if we should warn about missing access rules.
	pub fn module_has_any_rules(&self, module: ModuleName) -> bool {
		self.by_module.get(&module).is_some_and(|ids| !ids.is_empty())
	}

	/// Iterate over all access rules.
	pub fn iter(&self) -> impl Iterator<Item = dashmap::mapref::multiple::RefMulti<'_, AccessRuleId, AccessRule>> {
		self.inner.iter()
	}

	/// Retain only rules that match the predicate.
	pub fn retain<F>(&self, mut f: F)
	where
		F: FnMut(&AccessRuleId, &mut AccessRule) -> bool,
	{
		self.inner.retain(|k, v| f(k, v));
	}

	/// Get the total number of access rules indexed.
	pub fn len(&self) -> usize {
		self.inner.len()
	}

	/// Check if the index is empty.
	pub fn is_empty(&self) -> bool {
		self.inner.is_empty()
	}
}

/// Parse a model reference from ir.model.access.csv format.
///
/// The model_id:id column typically contains values like:
/// - "model_sale_order" -> "sale.order"
/// - "base.model_res_partner" -> "res.partner"
pub fn parse_model_ref(model_ref: &str) -> Option<String> {
	// Strip module prefix if present (e.g., "base.model_res_partner" -> "model_res_partner")
	let model_part = model_ref.rsplit_once('.').map(|(_, m)| m).unwrap_or(model_ref);

	// Must start with "model_"
	let model_name = model_part.strip_prefix("model_")?;

	// Convert underscores to dots (model_sale_order -> sale.order)
	Some(model_name.replace('_', "."))
}

/// Resolve a CSV model reference against the model index.
///
/// The CSV format `model_xxx_yyy` is ambiguous because both `.` and `_`
/// in model names become `_`. This function resolves the ambiguity by
/// checking known models: for each model in the index, it computes the
/// CSV form and compares.
///
/// Returns `None` if the reference doesn't start with `model_`.
pub fn resolve_model_from_csv_ref(model_ref: &str, models: &ModelIndex) -> Option<String> {
	let model_part = model_ref.rsplit_once('.').map(|(_, m)| m).unwrap_or(model_ref);
	let csv_name = model_part.strip_prefix("model_")?;

	let naive = csv_name.replace('_', ".");
	if _G(&naive).map(|k| models.contains_key(&k)).unwrap_or(false) {
		return Some(naive);
	}

	for entry in models.iter() {
		let model_name = _R(*entry.key());
		let csv_form = model_name.replace('.', "_");
		if csv_form == csv_name {
			return Some(model_name.to_string());
		}
	}

	Some(naive)
}

/// Parse a group reference from ir.model.access.csv format.
///
/// The group_id:id column typically contains values like:
/// - "base.group_user" -> "base.group_user"
/// - "group_sale_manager" -> needs module context
pub fn parse_group_ref(group_ref: &str, current_module: ModuleName) -> String {
	if group_ref.contains('.') {
		group_ref.to_string()
	} else {
		format!("{}.{}", _R(current_module), group_ref)
	}
}

/// Parse a boolean value from CSV (0, 1, True, False, etc.)
pub fn parse_csv_bool(value: &str) -> bool {
	matches!(value.trim().to_lowercase().as_str(), "1" | "true" | "yes")
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_parse_model_ref() {
		assert_eq!(parse_model_ref("model_sale_order"), Some("sale.order".to_string()));
		assert_eq!(parse_model_ref("model_res_partner"), Some("res.partner".to_string()));
		assert_eq!(
			parse_model_ref("base.model_res_partner"),
			Some("res.partner".to_string())
		);
		assert_eq!(parse_model_ref("model_ir_ui_view"), Some("ir.ui.view".to_string()));
		assert_eq!(parse_model_ref("invalid"), None);
	}

	#[test]
	fn test_parse_csv_bool() {
		assert!(parse_csv_bool("1"));
		assert!(parse_csv_bool("True"));
		assert!(parse_csv_bool("true"));
		assert!(parse_csv_bool("yes"));
		assert!(!parse_csv_bool("0"));
		assert!(!parse_csv_bool("False"));
		assert!(!parse_csv_bool("false"));
		assert!(!parse_csv_bool(""));
	}

	#[test]
	fn test_resolve_model_from_csv_ref_simple() {
		let models = ModelIndex::default();
		let name = _I("sale.order");
		models.insert(name.into(), Default::default());

		assert_eq!(
			resolve_model_from_csv_ref("model_sale_order", &models),
			Some("sale.order".to_string())
		);
	}

	#[test]
	fn test_resolve_model_from_csv_ref_with_module_prefix() {
		let models = ModelIndex::default();
		let name = _I("res.partner");
		models.insert(name.into(), Default::default());

		assert_eq!(
			resolve_model_from_csv_ref("base.model_res_partner", &models),
			Some("res.partner".to_string())
		);
	}

	#[test]
	fn test_resolve_model_from_csv_ref_ambiguous() {
		let models = ModelIndex::default();
		// Model with underscores in its technical name
		let name = _I("vias.atp.gov.status_history");
		models.insert(name.into(), Default::default());

		// The CSV form is model_vias_atp_gov_status_history
		// Naive conversion would give vias.atp.gov.status.history (wrong)
		// Reverse lookup should find the actual model
		assert_eq!(
			resolve_model_from_csv_ref("model_vias_atp_gov_status_history", &models),
			Some("vias.atp.gov.status_history".to_string())
		);
	}

	#[test]
	fn test_resolve_model_from_csv_ref_invalid() {
		let models = ModelIndex::default();
		assert_eq!(resolve_model_from_csv_ref("invalid", &models), None);
	}

	#[test]
	fn test_resolve_model_from_csv_ref_unknown_model() {
		let models = ModelIndex::default();
		// No models indexed, should fall back to naive conversion
		assert_eq!(
			resolve_model_from_csv_ref("model_unknown_model", &models),
			Some("unknown.model".to_string())
		);
	}
}
