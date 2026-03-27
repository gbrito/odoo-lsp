//! Validation for Odoo data file root containers.
//!
//! Validates:
//! - Root elements (`<odoo>`, `<openerp>`, `<data>`)
//! - `<data>` container elements
//! - `noupdate` attribute values
//! - Valid child elements within containers

use roxmltree::{Document, Node};

use super::{
	attribute_name_range, node_tag_range, validate_act_window, validate_delete, validate_function, validate_menuitem,
	validate_record, validate_report, ValidationContext,
};
use crate::xml::diagnostic_codes::XmlDiagnosticCode;

/// Valid root element names for Odoo data files.
const VALID_ROOT_ELEMENTS: &[&str] = &["odoo", "openerp", "data"];

/// Valid attributes for root/data container elements.
const VALID_ROOT_ATTRS: &[&str] = &["noupdate", "auto_sequence", "uid", "context"];

/// Valid values for the `noupdate` attribute.
const VALID_NOUPDATE_VALUES: &[&str] = &["0", "1", "true", "false", "True", "False"];

/// Validate the structure of an Odoo data file.
pub fn validate_data_file(ctx: &mut ValidationContext<'_>, doc: &Document) {
	let root = doc.root_element();

	// Check if root element is valid
	let root_name = root.tag_name().name();
	if !VALID_ROOT_ELEMENTS.contains(&root_name) {
		ctx.add_diagnostic(XmlDiagnosticCode::InvalidRootElement, node_tag_range(&root));
		return;
	}

	// Warn about deprecated openerp root
	if root_name == "openerp" {
		ctx.add_diagnostic(XmlDiagnosticCode::DeprecatedOpenerp, node_tag_range(&root));
	}

	// Validate the root container and its children
	validate_data_container(ctx, &root);
}

/// Validate a data container element (`<odoo>`, `<openerp>`, or `<data>`).
fn validate_data_container(ctx: &mut ValidationContext<'_>, container: &Node) {
	// Validate all attributes are known
	for attr in container.attributes() {
		if !VALID_ROOT_ATTRS.contains(&attr.name()) {
			if let Some(range) = attribute_name_range(container, attr.name()) {
				ctx.add_diagnostic_with_message(
					XmlDiagnosticCode::InvalidRootAttribute,
					range,
					&format!(
						"Invalid attribute '{}' on '{}' element",
						attr.name(),
						container.tag_name().name()
					),
				);
			}
		}
	}

	// Validate noupdate attribute if present
	if let Some(noupdate) = container.attribute("noupdate") {
		if !VALID_NOUPDATE_VALUES.contains(&noupdate) {
			if let Some(range) = super::attribute_value_range(container, "noupdate") {
				ctx.add_diagnostic(XmlDiagnosticCode::InvalidNoupdate, range);
			}
		}
	}

	// Validate child elements
	for child in container.children().filter(|n| n.is_element()) {
		let child_name = child.tag_name().name();

		match child_name {
			"data" => {
				// Nested data container
				validate_data_container(ctx, &child);
			}
			"record" => {
				validate_record(ctx, &child);
			}
			"menuitem" => {
				validate_menuitem(ctx, &child);
			}
			"function" => {
				validate_function(ctx, &child);
			}
			"delete" => {
				validate_delete(ctx, &child);
			}
			"act_window" => {
				validate_act_window(ctx, &child);
			}
			"report" => {
				validate_report(ctx, &child);
			}
			"template" => {
				// Templates have their own validation but we allow any content inside
				validate_template(ctx, &child);
			}
			"field" | "url" | "assert" | "workflow" => {
				// These are valid but we don't perform deep validation on them
			}
			_ => {
				// Unknown element in data file
				ctx.add_diagnostic(XmlDiagnosticCode::InvalidRootChild, node_tag_range(&child));
			}
		}
	}
}

/// Validate a template element.
///
/// Templates are validated minimally - we just check for the id attribute.
fn validate_template(ctx: &mut ValidationContext<'_>, node: &Node) {
	// Template must have an id attribute (unless it's an inherited template)
	let has_id = node.attribute("id").is_some();
	let has_inherit_id = node.attribute("inherit_id").is_some();

	if !has_id && !has_inherit_id {
		ctx.add_diagnostic(XmlDiagnosticCode::TemplateMissingId, node_tag_range(node));
	}

	// Check inherit_id reference if present
	if let Some(inherit_id) = node.attribute("inherit_id") {
		if !inherit_id.is_empty() && !ctx.record_exists(inherit_id) {
			if let Some(range) = super::attribute_value_range(node, "inherit_id") {
				ctx.add_diagnostic(XmlDiagnosticCode::TemplateInheritIdNotFound, range);
			}
		}
	}

	// We don't validate template content (QWeb expressions, etc.)
}
