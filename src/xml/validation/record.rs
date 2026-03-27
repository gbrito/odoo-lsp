//! Validation for `<record>` and `<field>` elements.
//!
//! Validates:
//! - Required attributes (id, model for record; name for field)
//! - Valid attribute combinations
//! - Field type values
//! - Record/field references

use roxmltree::Node;

use super::{attribute_value_range, node_tag_range, ValidationContext};
use crate::xml::diagnostic_codes::XmlDiagnosticCode;

/// Valid attributes for `<record>` element.
const VALID_RECORD_ATTRS: &[&str] = &["id", "model", "context", "forcecreate", "noupdate", "uid"];

/// Valid field type values.
const VALID_FIELD_TYPES: &[&str] = &["base64", "char", "int", "float", "list", "tuple", "html", "xml", "file"];

/// Field names that contain XML content (view arches) and should not have their children validated.
/// These fields typically contain view definitions where elements like <field>, <button>, <tree>, etc.
/// have different semantics than in data files.
const ARCH_FIELD_NAMES: &[&str] = &["arch", "arch_base", "arch_db", "arch_fs", "arch_prev"];

/// Models that have arch-like fields by convention.
const VIEW_MODELS: &[&str] = &[
	"ir.ui.view",
	"ir.actions.act_window.view",
	"website.page",
	"ir.ui.view.custom",
];

/// Validate a `<record>` element.
pub fn validate_record(ctx: &mut ValidationContext<'_>, node: &Node) {
	// Check required attributes
	let id = node.attribute("id");
	let model = node.attribute("model");

	if id.is_none() {
		ctx.add_diagnostic(XmlDiagnosticCode::RecordMissingId, node_tag_range(node));
	}

	if model.is_none() {
		ctx.add_diagnostic(XmlDiagnosticCode::RecordMissingModel, node_tag_range(node));
	}

	// Validate model exists if specified
	if let Some(model_name) = model {
		if !model_name.is_empty() && !ctx.model_exists(model_name) {
			if let Some(range) = attribute_value_range(node, "model") {
				ctx.add_diagnostic_with_message(
					XmlDiagnosticCode::RecordModelNotFound,
					range,
					&format!("Model '{}' not found", model_name),
				);
			}
		}
	}

	// Validate all attributes are known
	for attr in node.attributes() {
		if !VALID_RECORD_ATTRS.contains(&attr.name()) {
			if let Some(range) = super::attribute_name_range(node, attr.name()) {
				ctx.add_diagnostic_with_message(
					XmlDiagnosticCode::RecordInvalidAttribute,
					range,
					&format!("Unknown attribute '{}' on record element", attr.name()),
				);
			}
		}
	}

	// Validate child elements (should only be <field>)
	for child in node.children().filter(|n| n.is_element()) {
		let child_name = child.tag_name().name();
		if child_name == "field" {
			validate_field(ctx, &child, model);
		} else {
			ctx.add_diagnostic_with_message(
				XmlDiagnosticCode::RecordInvalidChild,
				node_tag_range(&child),
				&format!("Invalid child element '{}' in record; expected 'field'", child_name),
			);
		}
	}
}

/// Validate a `<field>` element within a record.
fn validate_field(ctx: &mut ValidationContext<'_>, node: &Node, parent_model: Option<&str>) {
	// Check required name attribute
	let name = match node.attribute("name") {
		Some(n) => n,
		None => {
			ctx.add_diagnostic(XmlDiagnosticCode::FieldMissingName, node_tag_range(node));
			return;
		}
	};

	// Detect if this field contains XML content (view arch) that shouldn't be validated
	// as regular data file content.
	let is_arch_field = is_arch_content_field(name, node.attribute("type"), parent_model);

	// Check for conflicting value attributes (ref, eval, search, type are mutually exclusive)
	let has_type = node.attribute("type").is_some();
	let has_ref = node.attribute("ref").is_some();
	let has_eval = node.attribute("eval").is_some();
	let has_search = node.attribute("search").is_some();

	let value_attr_count = [has_type, has_ref, has_eval, has_search].iter().filter(|b| **b).count();
	if value_attr_count > 1 {
		// Report the specific conflict (matching official behavior)
		if has_ref && has_eval {
			ctx.add_diagnostic(XmlDiagnosticCode::FieldRefAndEval, node_tag_range(node));
		}
		if has_ref && has_search {
			ctx.add_diagnostic(XmlDiagnosticCode::FieldRefAndSearch, node_tag_range(node));
		}
		if has_eval && has_search {
			ctx.add_diagnostic(XmlDiagnosticCode::FieldEvalAndSearch, node_tag_range(node));
		}
	}

	// Validate field type-specific content
	let mut is_xml_or_html = false;
	let mut is_iterable_child = false;
	if let Some(type_value) = node.attribute("type") {
		match type_value {
			"int" => {
				// Content must be a valid integer or "None"
				let content = get_text_content(node);
				if !content.is_empty() && content.parse::<i64>().is_err() && content != "None" {
					ctx.add_diagnostic_with_message(
						XmlDiagnosticCode::FieldIntContentInvalid,
						node.range(),
						&format!("Invalid content for int field: {}", content),
					);
				}
			}
			"float" => {
				// Content must be a valid float
				let content = get_text_content(node);
				if !content.is_empty() && content.parse::<f64>().is_err() {
					ctx.add_diagnostic_with_message(
						XmlDiagnosticCode::FieldFloatContentInvalid,
						node.range(),
						&format!("Invalid content for float field: {}", content),
					);
				}
			}
			"list" | "tuple" => {
				is_iterable_child = true;
				// Children must be <value> elements
				for child in node.children().filter(|n| n.is_element()) {
					if child.tag_name().name() != "value" {
						ctx.add_diagnostic_with_message(
							XmlDiagnosticCode::FieldListInvalidChild,
							node_tag_range(&child),
							&format!(
								"Invalid child '{}' in list/tuple field; expected 'value'",
								child.tag_name().name()
							),
						);
					} else {
						validate_value(ctx, &child);
					}
				}
			}
			"html" | "xml" => {
				is_xml_or_html = true;
			}
			"base64" | "char" | "file" => {
				// file attr + text content are mutually exclusive
				if node.attribute("file").is_some() {
					let content = get_text_content(node);
					if !content.is_empty() {
						ctx.add_diagnostic(XmlDiagnosticCode::FieldFileAndTextConflict, node.range());
					}
				}
			}
			_ => {
				if !VALID_FIELD_TYPES.contains(&type_value) {
					if let Some(range) = attribute_value_range(node, "type") {
						ctx.add_diagnostic(XmlDiagnosticCode::FieldInvalidType, range);
					}
				}
			}
		}
	}

	// Check if field has text content with value attributes (ref, eval, search)
	let has_text_content = !get_text_content(node).is_empty();

	// Validate per-attribute rules
	for attr in node.attributes() {
		match attr.name() {
			"name" | "type" | "file" => {}
			"ref" | "eval" | "search" => {
				// ref/eval/search should not co-exist with text content
				if has_text_content {
					ctx.add_diagnostic_with_message(
						XmlDiagnosticCode::FieldTextWithValueAttr,
						node.range(),
						&format!(
							"Text content is not allowed on a field with '{}' attribute",
							attr.name()
						),
					);
				}
			}
			"model" => {
				// model is only allowed when eval or search is present
				if !has_eval && !has_search {
					if let Some(range) = super::attribute_name_range(node, "model") {
						ctx.add_diagnostic(XmlDiagnosticCode::FieldModelWithoutEvalOrSearch, range);
					}
				}
			}
			"use" => {
				// use is only allowed when search is present
				if !has_search {
					if let Some(range) = super::attribute_name_range(node, "use") {
						ctx.add_diagnostic(XmlDiagnosticCode::FieldUseWithoutSearch, range);
					}
				}
			}
			"widget" | "position" | "mode" | "filter_domain" | "context" | "groups" => {}
			_ => {
				if let Some(range) = super::attribute_name_range(node, attr.name()) {
					ctx.add_diagnostic_with_message(
						XmlDiagnosticCode::FieldInvalidAttribute,
						range,
						&format!("Unknown attribute '{}' on field element", attr.name()),
					);
				}
			}
		}
	}

	// Validate ref attribute if present
	if let Some(ref_value) = node.attribute("ref") {
		if !ref_value.is_empty() && !ctx.record_exists(ref_value) {
			if let Some(range) = attribute_value_range(node, "ref") {
				ctx.add_diagnostic_with_message(
					XmlDiagnosticCode::FieldRefNotFound,
					range,
					&format!("Referenced record '{}' not found", ctx.qualify_xmlid(ref_value)),
				);
			}
		}
	}

	// Validate groups attribute if present
	if let Some(groups) = node.attribute("groups") {
		validate_groups_attribute(ctx, groups, node);
	}

	// Skip child element validation for arch fields - they contain view XML
	// where elements like <field>, <button>, <tree> have different semantics.
	if is_arch_field || is_xml_or_html || is_iterable_child {
		return;
	}

	// Validate child elements (should be <record> for relational fields)
	for child in node.children().filter(|n| n.is_element()) {
		let child_name = child.tag_name().name();
		match child_name {
			"record" => {
				// Nested records are valid for relational fields
				validate_record_nested(ctx, &child);
			}
			_ => {
				// Other non-text elements in a non-arch, non-xml/html, non-list/tuple field
				// are invalid according to the official schema
				if !node
					.attribute("type")
					.map(|t| t == "xml" || t == "html")
					.unwrap_or(false)
				{
					ctx.add_diagnostic_with_message(
						XmlDiagnosticCode::RecordInvalidChild,
						node_tag_range(&child),
						"Fields only allow 'record' children nodes",
					);
				}
			}
		}
	}
}

/// Get the trimmed text content of a node.
fn get_text_content(node: &Node) -> String {
	node.text().unwrap_or("").trim().to_string()
}

/// Check if a field contains XML/view arch content that shouldn't be validated
/// as regular data file elements.
///
/// This returns true for:
/// - Fields with type="xml" or type="html"
/// - Fields named "arch", "arch_base", "arch_db", etc.
/// - Fields on view-related models (ir.ui.view, etc.)
fn is_arch_content_field(name: &str, type_attr: Option<&str>, parent_model: Option<&str>) -> bool {
	// Check field type
	if let Some(t) = type_attr {
		if t == "xml" || t == "html" {
			return true;
		}
	}

	// Check field name
	if ARCH_FIELD_NAMES.contains(&name) {
		return true;
	}

	// Check parent model - some models are known to contain arch content
	if let Some(model) = parent_model {
		if VIEW_MODELS.contains(&model) {
			// For view models, assume most XML content is arch
			return true;
		}
	}

	false
}

/// Validate a nested `<record>` element (within a field).
fn validate_record_nested(ctx: &mut ValidationContext<'_>, node: &Node) {
	// Nested records must have a model attribute but id is optional
	let model = node.attribute("model");

	if model.is_none() {
		ctx.add_diagnostic(XmlDiagnosticCode::RecordMissingModel, node_tag_range(node));
	}

	// Validate model exists if specified
	if let Some(model_name) = model {
		if !model_name.is_empty() && !ctx.model_exists(model_name) {
			if let Some(range) = attribute_value_range(node, "model") {
				ctx.add_diagnostic_with_message(
					XmlDiagnosticCode::RecordModelNotFound,
					range,
					&format!("Model '{}' not found", model_name),
				);
			}
		}
	}

	// Validate child elements
	for child in node.children().filter(|n| n.is_element()) {
		let child_name = child.tag_name().name();
		if child_name == "field" {
			validate_field(ctx, &child, model);
		} else {
			ctx.add_diagnostic_with_message(
				XmlDiagnosticCode::RecordInvalidChild,
				node_tag_range(&child),
				&format!("Invalid child element '{}' in record; expected 'field'", child_name),
			);
		}
	}
}

/// Validate a `<value>` element within a field or function.
///
/// Implements the full mutual-exclusion logic matching the official odoo-ls:
/// - search, eval, type/file/text are mutually exclusive
/// - file and text content are mutually exclusive
/// - empty value data is flagged
fn validate_value(ctx: &mut ValidationContext<'_>, node: &Node) {
	let has_text = !get_text_content(node).is_empty();
	let mut has_search = false;
	let mut has_eval = false;
	let mut has_type_or_file_or_text = has_text;

	for attr in node.attributes() {
		match attr.name() {
			"name" | "model" | "use" => {}
			"search" => {
				has_search = true;
				if has_eval || has_type_or_file_or_text {
					if let Some(range) = super::attribute_name_range(node, "search") {
						ctx.add_diagnostic(XmlDiagnosticCode::ValueSearchConflict, range);
					}
				}
			}
			"eval" => {
				has_eval = true;
				if has_search || has_type_or_file_or_text {
					if let Some(range) = super::attribute_name_range(node, "eval") {
						ctx.add_diagnostic(XmlDiagnosticCode::ValueEvalConflict, range);
					}
				}
			}
			"type" => {
				has_type_or_file_or_text = true;
				if has_search || has_eval {
					if let Some(range) = super::attribute_name_range(node, "type") {
						ctx.add_diagnostic(XmlDiagnosticCode::ValueTypeConflict, range);
					}
					continue;
				}
				// type requires file or text content
				if node.attribute("file").is_none() && !has_text {
					if let Some(range) = super::attribute_name_range(node, "type") {
						ctx.add_diagnostic(XmlDiagnosticCode::ValueTypeRequiresFileOrText, range);
					}
				}
			}
			"file" => {
				has_type_or_file_or_text = true;
				// file + text conflict
				if has_text {
					if let Some(range) = super::attribute_name_range(node, "file") {
						ctx.add_diagnostic(XmlDiagnosticCode::ValueFileWithText, range);
					}
					continue;
				}
				// file + search/eval conflict
				if has_search || has_eval {
					if let Some(range) = super::attribute_name_range(node, "file") {
						ctx.add_diagnostic(XmlDiagnosticCode::ValueFileConflict, range);
					}
				}
			}
			_ => {
				if let Some(range) = super::attribute_name_range(node, attr.name()) {
					ctx.add_diagnostic_with_message(
						XmlDiagnosticCode::ValueInvalidAttribute,
						range,
						&format!("Invalid attribute '{}' in value node", attr.name()),
					);
				}
			}
		}
	}

	// Check for empty value data
	if !has_search && !has_eval && !has_type_or_file_or_text {
		ctx.add_diagnostic(XmlDiagnosticCode::ValueEmptyData, node.range());
	}
}

/// Validate a `groups` attribute value.
fn validate_groups_attribute(ctx: &mut ValidationContext<'_>, groups: &str, node: &Node) {
	if groups.is_empty() {
		if let Some(range) = attribute_value_range(node, "groups") {
			ctx.add_diagnostic(XmlDiagnosticCode::GroupsEmptyValue, range);
		}
		return;
	}

	// Groups is a comma-separated list of XML IDs
	let Some(attr_range) = attribute_value_range(node, "groups") else {
		return;
	};

	let mut offset = 0;
	for group in groups.split(',') {
		let group = group.trim();
		let group_start_in_value = groups[offset..].find(group).unwrap_or(0) + offset;
		let group_start = attr_range.start + group_start_in_value;
		let group_end = group_start + group.len();
		offset = group_start_in_value + group.len();

		if group.is_empty() {
			continue;
		}

		// Skip negated groups (e.g., "!base.group_user")
		let group_ref = if let Some(stripped) = group.strip_prefix('!') {
			stripped
		} else {
			group
		};

		if !ctx.record_exists(group_ref) {
			ctx.add_diagnostic_with_message(
				XmlDiagnosticCode::GroupsRefNotFound,
				group_start..group_end,
				&format!("Group '{}' not found", ctx.qualify_xmlid(group_ref)),
			);
		}
	}
}
