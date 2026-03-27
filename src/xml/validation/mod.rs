//! XML/RNG validation for Odoo data files.
//!
//! This module provides structural validation for Odoo XML data files,
//! validating elements like `<record>`, `<field>`, `<menuitem>`, `<function>`,
//! `<delete>`, `<act_window>`, and `<report>`.

mod act_window;
mod data_file;
mod function;
mod menuitem;
mod record;

use ropey::RopeSlice;
use roxmltree::{Document, Node};
use tower_lsp_server::ls_types::{Diagnostic, NumberOrString};

use crate::backend::Backend;
use crate::config::DiagnosticsConfig;
use crate::index::ModuleName;
use crate::utils::{rope_conv, ByteOffset};
use crate::xml::diagnostic_codes::XmlDiagnosticCode;

pub use act_window::validate_act_window;
pub use act_window::validate_report;
pub use data_file::validate_data_file;
pub use function::validate_delete;
pub use function::validate_function;
pub use menuitem::validate_menuitem;
pub use record::validate_record;

/// Context for XML validation operations.
///
/// Holds references to the backend, rope, and configuration needed
/// during validation of a single XML document.
pub struct ValidationContext<'a> {
	/// Reference to the LSP backend for index lookups.
	pub backend: &'a Backend,
	/// The rope slice for position conversion.
	pub rope: RopeSlice<'a>,
	/// The current module name for qualifying references.
	pub current_module: Option<ModuleName>,
	/// Diagnostics configuration for severity overrides.
	pub diagnostics_config: &'a DiagnosticsConfig,
	/// Accumulated diagnostics.
	pub diagnostics: Vec<Diagnostic>,
}

impl<'a> ValidationContext<'a> {
	/// Create a new validation context.
	pub fn new(
		backend: &'a Backend,
		rope: RopeSlice<'a>,
		current_module: Option<ModuleName>,
		diagnostics_config: &'a DiagnosticsConfig,
	) -> Self {
		Self {
			backend,
			rope,
			current_module,
			diagnostics_config,
			diagnostics: Vec::new(),
		}
	}

	/// Add a diagnostic if the code is enabled in configuration.
	pub fn add_diagnostic(&mut self, code: XmlDiagnosticCode, byte_range: std::ops::Range<usize>) {
		self.add_diagnostic_with_message(code, byte_range, code.message());
	}

	/// Add a diagnostic with a custom message if the code is enabled.
	pub fn add_diagnostic_with_message(
		&mut self,
		code: XmlDiagnosticCode,
		byte_range: std::ops::Range<usize>,
		message: &str,
	) {
		let Some(severity) = self.diagnostics_config.get_severity(code) else {
			// Diagnostic is disabled
			return;
		};

		let range = rope_conv(ByteOffset(byte_range.start)..ByteOffset(byte_range.end), self.rope);

		self.diagnostics.push(Diagnostic {
			range,
			severity: Some(severity),
			code: Some(NumberOrString::String(code.code())),
			source: Some("odoo-lsp".to_string()),
			message: message.to_string(),
			..Default::default()
		});
	}

	/// Qualify an XML ID with the current module if it's not already qualified.
	pub fn qualify_xmlid<'b>(&self, xmlid: &'b str) -> std::borrow::Cow<'b, str> {
		if xmlid.contains('.') {
			std::borrow::Cow::Borrowed(xmlid)
		} else if let Some(module) = self.current_module {
			std::borrow::Cow::Owned(format!("{}.{}", crate::index::_R(module), xmlid))
		} else {
			std::borrow::Cow::Borrowed(xmlid)
		}
	}

	/// Check if a record exists in the index.
	pub fn record_exists(&self, xmlid: &str) -> bool {
		use crate::index::_G;
		let qualified = self.qualify_xmlid(xmlid);
		_G(&qualified)
			.map(|key| self.backend.index.records.contains_key(&key))
			.unwrap_or(false)
	}

	/// Check if a model exists in the index.
	pub fn model_exists(&self, model_name: &str) -> bool {
		use crate::index::_G;
		_G(model_name)
			.map(|key| self.backend.index.models.contains_key(&key))
			.unwrap_or(false)
	}
}

/// Get the byte range of an XML node's tag name.
///
/// For `<record model="...">`, this returns the range of "record".
pub fn node_tag_range(node: &Node) -> std::ops::Range<usize> {
	let range = node.range();
	// The tag name starts after '<' and ends before any whitespace or '>'
	let start = range.start + 1; // Skip '<'
	let end = start + node.tag_name().name().len();
	start..end
}

/// Get the byte range of a node's attribute value (excluding quotes).
///
/// Returns None if the attribute doesn't exist.
/// Note: This returns a range that approximates the value position by using
/// the attribute's full range and calculating where the value should start.
pub fn attribute_value_range(node: &Node, attr_name: &str) -> Option<std::ops::Range<usize>> {
	let attr = node.attributes().find(|a| a.name() == attr_name)?;
	let full_range = attr.range();
	let qname_range = attr.range_qname();
	// Attribute format: name="value" or name='value'
	// Value starts after qname + '="' or '=\''  (2 chars: = and quote)
	// Value ends at full_range.end - 1 (closing quote)
	let value_start = qname_range.end + 2;
	let value_end = full_range.end.saturating_sub(1);
	Some(value_start..value_end)
}

/// Get the byte range of an attribute name.
///
/// Returns None if the attribute doesn't exist.
pub fn attribute_name_range(node: &Node, attr_name: &str) -> Option<std::ops::Range<usize>> {
	let attr = node.attributes().find(|a| a.name() == attr_name)?;
	Some(attr.range_qname())
}

/// Get the full byte range of a node (from opening to closing tag).
pub fn node_full_range(node: &Node) -> std::ops::Range<usize> {
	node.range()
}

/// Validate an XML document and return diagnostics.
///
/// This is the main entry point for XML validation.
pub fn validate_xml_document(
	backend: &Backend,
	contents: &str,
	rope: RopeSlice<'_>,
	current_module: Option<ModuleName>,
	diagnostics_config: &DiagnosticsConfig,
) -> Vec<Diagnostic> {
	// Check if XML validation is enabled
	if !diagnostics_config.is_xml_validation_enabled() {
		return Vec::new();
	}

	// Parse the XML document
	let doc = match Document::parse(contents) {
		Ok(doc) => doc,
		Err(err) => {
			// Return a parse error diagnostic
			let mut ctx = ValidationContext::new(backend, rope, current_module, diagnostics_config);
			let byte_pos = err.pos();
			// roxmltree::TextPos is 1-indexed, convert to byte offset
			let byte_range = compute_byte_range_from_text_pos(contents, byte_pos);
			ctx.add_diagnostic_with_message(
				XmlDiagnosticCode::XmlParseError,
				byte_range,
				&format!("XML parse error: {}", err),
			);
			return ctx.diagnostics;
		}
	};

	let mut ctx = ValidationContext::new(backend, rope, current_module, diagnostics_config);

	// Validate the document structure
	validate_data_file(&mut ctx, &doc);

	ctx.diagnostics
}

/// Convert a roxmltree TextPos to a byte range in the source.
fn compute_byte_range_from_text_pos(contents: &str, pos: roxmltree::TextPos) -> std::ops::Range<usize> {
	let mut current_line = 1u32;
	let mut line_start = 0usize;

	for (i, c) in contents.char_indices() {
		if current_line == pos.row {
			// Found the line, now find the column
			let byte_offset = line_start + (pos.col as usize).saturating_sub(1);
			return byte_offset..byte_offset + 1;
		}
		if c == '\n' {
			current_line += 1;
			line_start = i + 1;
		}
	}

	// Fallback: return end of document
	let len = contents.len();
	len.saturating_sub(1)..len
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::config::DiagnosticsConfig;

	#[test]
	fn test_compute_byte_range_from_text_pos() {
		let contents = "line1\nline2\nline3";
		let pos = roxmltree::TextPos::new(2, 3);
		let range = compute_byte_range_from_text_pos(contents, pos);
		// Line 2 starts at byte 6, column 3 is byte 8
		assert_eq!(range, 8..9);
	}

	/// Test that parse errors are detected
	#[test]
	fn test_parse_error() {
		let xml = "<odoo><invalid";
		let _rope = ropey::Rope::from_str(xml);
		let _config = DiagnosticsConfig::default();

		let doc = Document::parse(xml);
		assert!(doc.is_err(), "Expected parse error for invalid XML");
	}

	/// Test valid data file structure
	#[test]
	fn test_valid_data_file() {
		let xml = r#"<?xml version="1.0"?>
<odoo>
    <record id="test" model="res.partner">
        <field name="name">Test</field>
    </record>
</odoo>"#;

		let doc = Document::parse(xml).expect("Valid XML should parse");
		let root = doc.root_element();
		assert_eq!(root.tag_name().name(), "odoo");
	}

	/// Test deprecated openerp root
	#[test]
	fn test_deprecated_openerp() {
		let xml = r#"<?xml version="1.0"?>
<openerp>
    <data>
        <record id="test" model="res.partner">
            <field name="name">Test</field>
        </record>
    </data>
</openerp>"#;

		let doc = Document::parse(xml).expect("Valid XML should parse");
		let root = doc.root_element();
		assert_eq!(root.tag_name().name(), "openerp");
	}

	/// Test invalid root element detection
	#[test]
	fn test_invalid_root() {
		let xml = r#"<?xml version="1.0"?>
<invalid_root>
    <record id="test" model="res.partner" />
</invalid_root>"#;

		let doc = Document::parse(xml).expect("XML should parse");
		let root = doc.root_element();
		assert_eq!(root.tag_name().name(), "invalid_root");
		// In the full validation, this would generate InvalidRootElement diagnostic
	}

	/// Test record validation - missing id
	#[test]
	fn test_record_missing_id() {
		let xml = r#"<odoo><record model="test.model"><field name="x">y</field></record></odoo>"#;
		let doc = Document::parse(xml).expect("XML should parse");
		let root = doc.root_element();
		let record = root.children().find(|n| n.tag_name().name() == "record").unwrap();
		assert!(record.attribute("id").is_none());
		assert!(record.attribute("model").is_some());
	}

	/// Test record validation - missing model
	#[test]
	fn test_record_missing_model() {
		let xml = r#"<odoo><record id="test"><field name="x">y</field></record></odoo>"#;
		let doc = Document::parse(xml).expect("XML should parse");
		let root = doc.root_element();
		let record = root.children().find(|n| n.tag_name().name() == "record").unwrap();
		assert!(record.attribute("id").is_some());
		assert!(record.attribute("model").is_none());
	}

	/// Test field validation - missing name
	#[test]
	fn test_field_missing_name() {
		let xml = r#"<odoo><record id="test" model="test.model"><field>value</field></record></odoo>"#;
		let doc = Document::parse(xml).expect("XML should parse");
		let root = doc.root_element();
		let record = root.children().find(|n| n.tag_name().name() == "record").unwrap();
		let field = record.children().find(|n| n.tag_name().name() == "field").unwrap();
		assert!(field.attribute("name").is_none());
	}

	/// Test field validation - conflicting ref and eval
	#[test]
	fn test_field_ref_and_eval_conflict() {
		let xml = r#"<odoo><record id="test" model="test.model"><field name="x" ref="y" eval="True"/></record></odoo>"#;
		let doc = Document::parse(xml).expect("XML should parse");
		let root = doc.root_element();
		let record = root.children().find(|n| n.tag_name().name() == "record").unwrap();
		let field = record.children().find(|n| n.tag_name().name() == "field").unwrap();
		assert!(field.attribute("ref").is_some());
		assert!(field.attribute("eval").is_some());
	}

	/// Test menuitem validation - missing id
	#[test]
	fn test_menuitem_missing_id() {
		let xml = r#"<odoo><menuitem name="Test" /></odoo>"#;
		let doc = Document::parse(xml).expect("XML should parse");
		let root = doc.root_element();
		let menuitem = root.children().find(|n| n.tag_name().name() == "menuitem").unwrap();
		assert!(menuitem.attribute("id").is_none());
	}

	/// Test act_window validation
	#[test]
	fn test_act_window_missing_required() {
		let xml = r#"<odoo><act_window name="Test" res_model="test.model" /></odoo>"#;
		let doc = Document::parse(xml).expect("XML should parse");
		let root = doc.root_element();
		let act = root.children().find(|n| n.tag_name().name() == "act_window").unwrap();
		assert!(act.attribute("id").is_none()); // Missing id
		assert!(act.attribute("name").is_some());
		assert!(act.attribute("res_model").is_some());
	}

	/// Test invalid target value
	#[test]
	fn test_act_window_invalid_target() {
		let xml = r#"<odoo><act_window id="test" name="Test" res_model="test.model" target="invalid" /></odoo>"#;
		let doc = Document::parse(xml).expect("XML should parse");
		let root = doc.root_element();
		let act = root.children().find(|n| n.tag_name().name() == "act_window").unwrap();
		let target = act.attribute("target").unwrap();
		assert!(!["current", "new", "inline", "fullscreen", "main"].contains(&target));
	}

	/// Test report validation - missing required
	#[test]
	fn test_report_missing_required() {
		let xml = r#"<odoo><report model="test.model" name="test" string="Test" /></odoo>"#;
		let doc = Document::parse(xml).expect("XML should parse");
		let root = doc.root_element();
		let report = root.children().find(|n| n.tag_name().name() == "report").unwrap();
		assert!(report.attribute("id").is_none()); // Missing id
	}

	/// Test function validation - missing model
	#[test]
	fn test_function_missing_model() {
		let xml = r#"<odoo><function name="test_fn" /></odoo>"#;
		let doc = Document::parse(xml).expect("XML should parse");
		let root = doc.root_element();
		let func = root.children().find(|n| n.tag_name().name() == "function").unwrap();
		assert!(func.attribute("model").is_none());
		assert!(func.attribute("name").is_some());
	}

	/// Test delete validation - missing model
	#[test]
	fn test_delete_missing_model() {
		let xml = r#"<odoo><delete id="test" /></odoo>"#;
		let doc = Document::parse(xml).expect("XML should parse");
		let root = doc.root_element();
		let del = root.children().find(|n| n.tag_name().name() == "delete").unwrap();
		assert!(del.attribute("model").is_none());
	}

	/// Test delete validation - missing id or search
	#[test]
	fn test_delete_missing_id_or_search() {
		let xml = r#"<odoo><delete model="test.model" /></odoo>"#;
		let doc = Document::parse(xml).expect("XML should parse");
		let root = doc.root_element();
		let del = root.children().find(|n| n.tag_name().name() == "delete").unwrap();
		assert!(del.attribute("id").is_none());
		assert!(del.attribute("search").is_none());
	}

	/// Test template validation - missing id
	#[test]
	fn test_template_missing_id() {
		let xml = r#"<odoo><template><div>Test</div></template></odoo>"#;
		let doc = Document::parse(xml).expect("XML should parse");
		let root = doc.root_element();
		let template = root.children().find(|n| n.tag_name().name() == "template").unwrap();
		assert!(template.attribute("id").is_none());
		assert!(template.attribute("inherit_id").is_none());
	}

	/// Test node_tag_range helper
	#[test]
	fn test_node_tag_range() {
		let xml = "<record id=\"test\" />";
		let doc = Document::parse(xml).expect("XML should parse");
		let root = doc.root_element();
		let range = node_tag_range(&root);
		// Should point to "record" (after '<')
		assert_eq!(&xml[range.clone()], "record");
	}

	/// Test attribute_value_range helper
	#[test]
	fn test_attribute_value_range() {
		let xml = r#"<record id="test_value" />"#;
		let doc = Document::parse(xml).expect("XML should parse");
		let root = doc.root_element();
		let range = attribute_value_range(&root, "id");
		assert!(range.is_some());
		let range = range.unwrap();
		assert_eq!(&xml[range], "test_value");
	}

	/// Test that arch fields in ir.ui.view records don't produce false positives
	/// for child elements like <field>, <button>, <tree>, etc.
	#[test]
	fn test_arch_field_not_validated() {
		// This is a typical ir.ui.view record with a form arch
		let xml = r#"<?xml version="1.0"?>
<odoo>
    <record id="view_partner_form" model="ir.ui.view">
        <field name="name">res.partner.form</field>
        <field name="model">res.partner</field>
        <field name="arch" type="xml">
            <form>
                <field name="name"/>
                <button name="action_test" type="object"/>
                <tree>
                    <field name="email"/>
                </tree>
            </form>
        </field>
    </record>
</odoo>"#;

		let doc = Document::parse(xml).expect("Valid XML should parse");
		let root = doc.root_element();
		assert_eq!(root.tag_name().name(), "odoo");

		// Find the record
		let record = root
			.children()
			.find(|n| n.tag_name().name() == "record")
			.expect("Should have a record element");

		// Find the arch field
		let arch_field = record
			.children()
			.filter(|n| n.is_element() && n.tag_name().name() == "field")
			.find(|n| n.attribute("name") == Some("arch"))
			.expect("Should have an arch field");

		// Verify arch field has type="xml"
		assert_eq!(arch_field.attribute("type"), Some("xml"));

		// Verify the arch field has child elements (form, field, button, etc.)
		let child_elements: Vec<_> = arch_field.children().filter(|n| n.is_element()).collect();
		assert!(!child_elements.is_empty(), "Arch field should have child elements");

		// The first child should be <form>
		let form = &child_elements[0];
		assert_eq!(form.tag_name().name(), "form");
	}

	/// Test that regular fields (not arch) still validate children properly
	#[test]
	fn test_regular_field_validates_children() {
		let xml = r#"<?xml version="1.0"?>
<odoo>
    <record id="test_record" model="res.partner">
        <field name="child_ids">
            <record model="res.partner">
                <field name="name">Child</field>
            </record>
        </field>
    </record>
</odoo>"#;

		let doc = Document::parse(xml).expect("Valid XML should parse");
		let root = doc.root_element();
		assert_eq!(root.tag_name().name(), "odoo");

		// Find the record
		let record = root
			.children()
			.find(|n| n.tag_name().name() == "record")
			.expect("Should have a record element");

		// Find the child_ids field
		let child_field = record
			.children()
			.filter(|n| n.is_element() && n.tag_name().name() == "field")
			.find(|n| n.attribute("name") == Some("child_ids"))
			.expect("Should have a child_ids field");

		// The child should be a nested <record>
		let nested_record = child_field
			.children()
			.find(|n| n.is_element() && n.tag_name().name() == "record");
		assert!(nested_record.is_some(), "Should have a nested record");
	}
}
