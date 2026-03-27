//! Validation for `<function>` and `<delete>` elements.
//!
//! Validates:
//! - Required attributes
//! - Model references
//! - Valid child elements

use roxmltree::Node;

use super::{attribute_value_range, node_tag_range, ValidationContext};
use crate::xml::diagnostic_codes::XmlDiagnosticCode;

/// Valid attributes for `<function>` element.
const VALID_FUNCTION_ATTRS: &[&str] = &["model", "name", "uid", "context", "eval"];

/// Valid attributes for `<delete>` element.
const VALID_DELETE_ATTRS: &[&str] = &["model", "id", "search", "noupdate"];

/// Validate a `<function>` element.
pub fn validate_function(ctx: &mut ValidationContext<'_>, node: &Node) {
	// Check required attributes: model and name
	for required_attr in ["model", "name"] {
		if node.attribute(required_attr).is_none() {
			let code = match required_attr {
				"model" => XmlDiagnosticCode::FunctionMissingModel,
				"name" => XmlDiagnosticCode::FunctionMissingName,
				_ => unreachable!(),
			};
			ctx.add_diagnostic(code, node_tag_range(node));
		}
	}

	// Validate model exists if specified
	if let Some(model_name) = node.attribute("model") {
		if !model_name.is_empty() && !ctx.model_exists(model_name) {
			if let Some(range) = attribute_value_range(node, "model") {
				ctx.add_diagnostic_with_message(
					XmlDiagnosticCode::FunctionModelNotFound,
					range,
					&format!("Model '{}' not found", model_name),
				);
			}
		}
	}

	let has_eval = node.attribute("eval").is_some();

	// Validate all attributes are known
	for attr in node.attributes() {
		if !VALID_FUNCTION_ATTRS.contains(&attr.name()) {
			if let Some(range) = super::attribute_name_range(node, attr.name()) {
				ctx.add_diagnostic_with_message(
					XmlDiagnosticCode::FunctionInvalidAttribute,
					range,
					&format!("Unknown attribute '{}' on function element", attr.name()),
				);
			}
		}
	}

	// Validate child elements
	for child in node.children().filter(|n| n.is_element()) {
		let child_name = child.tag_name().name();
		match child_name {
			"value" => {
				validate_function_value(ctx, &child);
				// eval + value children conflict
				if has_eval {
					ctx.add_diagnostic(XmlDiagnosticCode::FunctionEvalWithValueChild, node_tag_range(&child));
				}
			}
			"function" => {
				// Recursively validate nested function calls
				validate_function(ctx, &child);
				// eval + function children conflict
				if has_eval {
					ctx.add_diagnostic(XmlDiagnosticCode::FunctionEvalWithFunctionChild, node_tag_range(&child));
				}
			}
			_ => {
				ctx.add_diagnostic_with_message(
					XmlDiagnosticCode::FunctionInvalidChild,
					node_tag_range(&child),
					&format!(
						"Invalid child element '{}' in function; expected 'value' or 'function'",
						child_name
					),
				);
			}
		}
	}
}

/// Validate a `<value>` element within a function.
fn validate_function_value(ctx: &mut ValidationContext<'_>, node: &Node) {
	// Value elements in function can have model, search, and eval attributes
	// Validate model exists if specified
	if let Some(model_name) = node.attribute("model") {
		if !model_name.is_empty() && !ctx.model_exists(model_name) {
			if let Some(range) = attribute_value_range(node, "model") {
				ctx.add_diagnostic_with_message(
					XmlDiagnosticCode::ValueMissingModel,
					range,
					&format!("Model '{}' not found", model_name),
				);
			}
		}
	}
}

/// Validate a `<delete>` element.
pub fn validate_delete(ctx: &mut ValidationContext<'_>, node: &Node) {
	// Check required model attribute
	let model = node.attribute("model");
	if model.is_none() {
		ctx.add_diagnostic(XmlDiagnosticCode::DeleteMissingModel, node_tag_range(node));
	}

	// Check that either id or search is present, but not both
	let has_id = node.attribute("id").is_some();
	let has_search = node.attribute("search").is_some();
	if has_id && has_search {
		ctx.add_diagnostic(XmlDiagnosticCode::DeleteIdAndSearchConflict, node_tag_range(node));
	}
	if !has_id && !has_search {
		ctx.add_diagnostic(XmlDiagnosticCode::DeleteMissingIdOrSearch, node_tag_range(node));
	}

	// Validate model exists if specified
	if let Some(model_name) = model {
		if !model_name.is_empty() && !ctx.model_exists(model_name) {
			if let Some(range) = attribute_value_range(node, "model") {
				ctx.add_diagnostic_with_message(
					XmlDiagnosticCode::DeleteModelNotFound,
					range,
					&format!("Model '{}' not found", model_name),
				);
			}
		}
	}

	// Validate id reference if present
	if let Some(id) = node.attribute("id") {
		if !id.is_empty() && !ctx.record_exists(id) {
			if let Some(range) = attribute_value_range(node, "id") {
				ctx.add_diagnostic_with_message(
					XmlDiagnosticCode::DeleteIdNotFound,
					range,
					&format!("Record '{}' not found", ctx.qualify_xmlid(id)),
				);
			}
		}
	}

	// Validate all attributes are known
	for attr in node.attributes() {
		if !VALID_DELETE_ATTRS.contains(&attr.name()) {
			if let Some(range) = super::attribute_name_range(node, attr.name()) {
				ctx.add_diagnostic_with_message(
					XmlDiagnosticCode::DeleteInvalidAttribute,
					range,
					&format!("Unknown attribute '{}' on delete element", attr.name()),
				);
			}
		}
	}

	// Delete should not have child elements
	if node.children().any(|n| n.is_element()) {
		ctx.add_diagnostic(XmlDiagnosticCode::DeleteHasChildren, node_tag_range(node));
	}
}
