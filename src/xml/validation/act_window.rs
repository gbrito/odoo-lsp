//! Validation for `<act_window>` and `<report>` elements.
//!
//! Validates:
//! - Required attributes
//! - Model references
//! - View mode, target, binding_type, and report_type values
//! - No child elements

use roxmltree::Node;

use super::{attribute_value_range, node_tag_range, ValidationContext};
use crate::xml::diagnostic_codes::XmlDiagnosticCode;

/// Valid attributes for `<act_window>` element.
const VALID_ACT_WINDOW_ATTRS: &[&str] = &[
	"id",
	"name",
	"res_model",
	"view_id",
	"domain",
	"context",
	"view_mode",
	"view_type",
	"target",
	"groups",
	"limit",
	"search_view_id",
	"usage",
	"binding_model",
	"binding_type",
	"binding_views",
	"src_model",
	"multi",
	"key2",
];

/// Valid attributes for `<report>` element.
const VALID_REPORT_ATTRS: &[&str] = &[
	"id",
	"model",
	"name",
	"string",
	"file",
	"report_type",
	"groups",
	"attachment",
	"attachment_use",
	"usage",
	"multi",
	"menu",
	"header",
	"print_report_name",
	"paperformat",
	"binding_model",
	"binding_type",
];

/// Valid view modes for act_window.
const VALID_VIEW_MODES: &[&str] = &[
	"tree", "form", "kanban", "calendar", "pivot", "graph", "activity", "map", "cohort", "gantt", "grid", "search",
	"qweb", "list",
];

/// Valid target values.
const VALID_TARGETS: &[&str] = &["current", "new", "inline", "fullscreen", "main"];

/// Valid binding_type values.
const VALID_BINDING_TYPES: &[&str] = &["action", "action_form_only", "report"];

/// Valid report_type values.
const VALID_REPORT_TYPES: &[&str] = &["qweb-html", "qweb-pdf", "qweb-text"];

/// Validate an `<act_window>` element.
pub fn validate_act_window(ctx: &mut ValidationContext<'_>, node: &Node) {
	// Check required id attribute
	if node.attribute("id").is_none() {
		ctx.add_diagnostic(XmlDiagnosticCode::ActWindowMissingId, node_tag_range(node));
	}

	// Check required name attribute
	if node.attribute("name").is_none() {
		ctx.add_diagnostic(XmlDiagnosticCode::ActWindowMissingName, node_tag_range(node));
	}

	// Check required res_model attribute
	let res_model = node.attribute("res_model");
	if res_model.is_none() {
		ctx.add_diagnostic(XmlDiagnosticCode::ActWindowMissingResModel, node_tag_range(node));
	}

	// Validate res_model exists
	if let Some(model_name) = res_model {
		if !model_name.is_empty() && !ctx.model_exists(model_name) {
			if let Some(range) = attribute_value_range(node, "res_model") {
				ctx.add_diagnostic_with_message(
					XmlDiagnosticCode::ActWindowResModelNotFound,
					range,
					&format!("Model '{}' not found", model_name),
				);
			}
		}
	}

	// Validate src_model exists if specified
	if let Some(model_name) = node.attribute("src_model") {
		if !model_name.is_empty() && !ctx.model_exists(model_name) {
			if let Some(range) = attribute_value_range(node, "src_model") {
				ctx.add_diagnostic_with_message(
					XmlDiagnosticCode::ActWindowSrcModelNotFound,
					range,
					&format!("Model '{}' not found", model_name),
				);
			}
		}
	}

	// Validate binding_model exists if specified
	if let Some(model_name) = node.attribute("binding_model") {
		if !model_name.is_empty() && !ctx.model_exists(model_name) {
			if let Some(range) = attribute_value_range(node, "binding_model") {
				ctx.add_diagnostic_with_message(
					XmlDiagnosticCode::ActWindowSrcModelNotFound,
					range,
					&format!("Binding model '{}' not found", model_name),
				);
			}
		}
	}

	// Validate view_mode if specified
	if let Some(view_mode) = node.attribute("view_mode") {
		for mode in view_mode.split(',') {
			let mode = mode.trim();
			if !mode.is_empty() && !VALID_VIEW_MODES.contains(&mode) {
				if let Some(range) = attribute_value_range(node, "view_mode") {
					ctx.add_diagnostic_with_message(
						XmlDiagnosticCode::ActWindowInvalidViewMode,
						range,
						&format!(
							"Invalid view mode '{}'; expected one of: {}",
							mode,
							VALID_VIEW_MODES.join(", ")
						),
					);
					break;
				}
			}
		}
	}

	// Validate target if specified
	if let Some(target) = node.attribute("target") {
		if !VALID_TARGETS.contains(&target) {
			if let Some(range) = attribute_value_range(node, "target") {
				ctx.add_diagnostic(XmlDiagnosticCode::ActWindowInvalidTarget, range);
			}
		}
	}

	// Validate binding_type if specified
	if let Some(binding_type) = node.attribute("binding_type") {
		if !VALID_BINDING_TYPES.contains(&binding_type) {
			if let Some(range) = attribute_value_range(node, "binding_type") {
				ctx.add_diagnostic(XmlDiagnosticCode::ActWindowInvalidBindingType, range);
			}
		}
	}

	// Validate binding_views if specified (format: "view_type1,view_type2")
	if let Some(binding_views) = node.attribute("binding_views") {
		for view_type in binding_views.split(',') {
			let view_type = view_type.trim();
			if !view_type.is_empty() && !VALID_VIEW_MODES.contains(&view_type) {
				if let Some(range) = attribute_value_range(node, "binding_views") {
					ctx.add_diagnostic_with_message(
						XmlDiagnosticCode::ActWindowInvalidBindingViews,
						range,
						&format!("Invalid view type '{}' in binding_views", view_type),
					);
					break;
				}
			}
		}
	}

	// Validate groups attribute if present
	if let Some(groups) = node.attribute("groups") {
		validate_groups_attribute(ctx, groups, node);
	}

	// Validate all attributes are known
	for attr in node.attributes() {
		if !VALID_ACT_WINDOW_ATTRS.contains(&attr.name()) {
			if let Some(range) = super::attribute_name_range(node, attr.name()) {
				ctx.add_diagnostic_with_message(
					XmlDiagnosticCode::ActWindowInvalidAttribute,
					range,
					&format!("Unknown attribute '{}' on act_window element", attr.name()),
				);
			}
		}
	}

	// Act_window should not have child elements
	if node.children().any(|n| n.is_element()) {
		ctx.add_diagnostic(XmlDiagnosticCode::ActWindowHasChildren, node_tag_range(node));
	}
}

/// Validate a `<report>` element.
pub fn validate_report(ctx: &mut ValidationContext<'_>, node: &Node) {
	// Check required id attribute
	if node.attribute("id").is_none() {
		ctx.add_diagnostic(XmlDiagnosticCode::ReportMissingId, node_tag_range(node));
	}

	// Check required model attribute
	let model = node.attribute("model");
	if model.is_none() {
		ctx.add_diagnostic(XmlDiagnosticCode::ReportMissingModel, node_tag_range(node));
	}

	// Check required name attribute
	if node.attribute("name").is_none() {
		ctx.add_diagnostic(XmlDiagnosticCode::ReportMissingName, node_tag_range(node));
	}

	// Check that either file or string is present
	let has_file = node.attribute("file").is_some();
	let has_string = node.attribute("string").is_some();
	if !has_file && !has_string {
		ctx.add_diagnostic(XmlDiagnosticCode::ReportMissingFileOrString, node_tag_range(node));
	}

	// Validate model exists
	if let Some(model_name) = model {
		if !model_name.is_empty() && !ctx.model_exists(model_name) {
			if let Some(range) = attribute_value_range(node, "model") {
				ctx.add_diagnostic_with_message(
					XmlDiagnosticCode::ReportModelNotFound,
					range,
					&format!("Model '{}' not found", model_name),
				);
			}
		}
	}

	// Validate binding_model exists if specified
	if let Some(model_name) = node.attribute("binding_model") {
		if !model_name.is_empty() && !ctx.model_exists(model_name) {
			if let Some(range) = attribute_value_range(node, "binding_model") {
				ctx.add_diagnostic_with_message(
					XmlDiagnosticCode::ReportModelNotFound,
					range,
					&format!("Binding model '{}' not found", model_name),
				);
			}
		}
	}

	// Validate report_type if specified
	if let Some(report_type) = node.attribute("report_type") {
		if !VALID_REPORT_TYPES.contains(&report_type) {
			if let Some(range) = attribute_value_range(node, "report_type") {
				ctx.add_diagnostic(XmlDiagnosticCode::ReportInvalidReportType, range);
			}
		}
	}

	// Validate binding_type if specified
	if let Some(binding_type) = node.attribute("binding_type") {
		if !VALID_BINDING_TYPES.contains(&binding_type) {
			if let Some(range) = attribute_value_range(node, "binding_type") {
				// Reuse the act_window invalid binding type error
				ctx.add_diagnostic(XmlDiagnosticCode::ActWindowInvalidBindingType, range);
			}
		}
	}

	// Validate groups attribute if present
	if let Some(groups) = node.attribute("groups") {
		validate_groups_attribute(ctx, groups, node);
	}

	// Validate all attributes are known
	for attr in node.attributes() {
		if !VALID_REPORT_ATTRS.contains(&attr.name()) {
			if let Some(range) = super::attribute_name_range(node, attr.name()) {
				ctx.add_diagnostic_with_message(
					XmlDiagnosticCode::ReportInvalidAttribute,
					range,
					&format!("Unknown attribute '{}' on report element", attr.name()),
				);
			}
		}
	}

	// Report should not have child elements
	if node.children().any(|n| n.is_element()) {
		ctx.add_diagnostic(XmlDiagnosticCode::ReportHasChildren, node_tag_range(node));
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
