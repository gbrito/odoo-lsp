//! Validation for `<menuitem>` elements.
//!
//! Validates:
//! - Required attributes (id)
//! - Valid attribute names
//! - Action, parent, and groups references
//! - Sequence value format
//! - Recursive submenu support (matching official odoo-ls behavior)
//! - Context-sensitive rules: parent in submenu, web_icon with parent, action+parent+children

use roxmltree::Node;

use super::{attribute_value_range, node_tag_range, ValidationContext};
use crate::xml::diagnostic_codes::XmlDiagnosticCode;

/// Validate a `<menuitem>` element.
///
/// `is_submenu` indicates whether this menuitem is nested inside another menuitem.
pub fn validate_menuitem(ctx: &mut ValidationContext<'_>, node: &Node) {
	validate_menuitem_inner(ctx, node, false);
}

fn validate_menuitem_inner(ctx: &mut ValidationContext<'_>, node: &Node, is_submenu: bool) {
	// Check required id attribute
	if node.attribute("id").is_none() {
		ctx.add_diagnostic(XmlDiagnosticCode::MenuitemMissingId, node_tag_range(node));
	}

	let has_parent = node.attribute("parent").is_some();

	// Validate each attribute
	for attr in node.attributes() {
		match attr.name() {
			"id" => {}
			"sequence" => {
				if attr.value().parse::<i64>().is_err() {
					if let Some(range) = attribute_value_range(node, "sequence") {
						ctx.add_diagnostic(XmlDiagnosticCode::MenuitemInvalidSequence, range);
					}
				}
			}
			"groups" => {
				validate_groups_attribute(ctx, attr.value(), node);
			}
			"name" | "active" => {}
			"action" => {
				// Check for submenu conflict: action+parent with child menuitems
				if (has_parent || is_submenu)
					&& node
						.children()
						.any(|c| c.is_element() && c.tag_name().name() == "menuitem")
				{
					for sub_menu in node
						.children()
						.filter(|c| c.is_element() && c.tag_name().name() == "menuitem")
					{
						ctx.add_diagnostic(XmlDiagnosticCode::MenuitemActionWithSubmenu, node_tag_range(&sub_menu));
					}
				}
				// Validate action reference exists
				let action = attr.value();
				if !action.is_empty() && !ctx.record_exists(action) {
					if let Some(range) = attribute_value_range(node, "action") {
						ctx.add_diagnostic_with_message(
							XmlDiagnosticCode::MenuitemActionNotFound,
							range,
							&format!("Action '{}' not found", ctx.qualify_xmlid(action)),
						);
					}
				}
			}
			"parent" => {
				// parent attribute is not allowed in submenus
				if is_submenu {
					if let Some(range) = super::attribute_name_range(node, "parent") {
						ctx.add_diagnostic(XmlDiagnosticCode::MenuitemSubmenuParentForbidden, range);
					}
				} else {
					// Validate parent reference exists
					let parent = attr.value();
					if !parent.is_empty() && !ctx.record_exists(parent) {
						if let Some(range) = attribute_value_range(node, "parent") {
							ctx.add_diagnostic_with_message(
								XmlDiagnosticCode::MenuitemParentNotFound,
								range,
								&format!("Parent menu '{}' not found", ctx.qualify_xmlid(parent)),
							);
						}
					}
				}
			}
			"web_icon" => {
				// web_icon is not allowed when parent is specified or in submenus
				if has_parent || is_submenu {
					if let Some(range) = super::attribute_name_range(node, "web_icon") {
						ctx.add_diagnostic(XmlDiagnosticCode::MenuitemSubmenuWebIconForbidden, range);
					}
				}
			}
			"web_icon_data" => {}
			_ => {
				if let Some(range) = super::attribute_name_range(node, attr.name()) {
					ctx.add_diagnostic_with_message(
						XmlDiagnosticCode::MenuitemInvalidAttribute,
						range,
						&format!("Unknown attribute '{}' on menuitem element", attr.name()),
					);
				}
			}
		}
	}

	// Validate child elements: only <menuitem> children are allowed (recursive submenus)
	for child in node.children().filter(|n| n.is_element()) {
		if child.tag_name().name() != "menuitem" {
			ctx.add_diagnostic_with_message(
				XmlDiagnosticCode::MenuitemInvalidChild,
				node_tag_range(&child),
				&format!("Invalid child element '{}' in menuitem", child.tag_name().name()),
			);
		} else {
			// Recursively validate submenu
			validate_menuitem_inner(ctx, &child, true);
		}
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
				XmlDiagnosticCode::MenuitemGroupsNotFound,
				group_start..group_end,
				&format!("Group '{}' not found", ctx.qualify_xmlid(group_ref)),
			);
		}
	}
}
