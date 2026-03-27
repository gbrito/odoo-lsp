//! XML diagnostic codes for Odoo data file validation.
//!
//! Code ranges follow the odoo-ls convention:
//! - OLS05XXX: XML/RNG validation diagnostics

use tower_lsp_server::ls_types::DiagnosticSeverity;

/// Diagnostic codes for XML validation errors.
///
/// Each variant corresponds to a specific validation rule and has an associated
/// code string (e.g., "OLS05001") and default severity level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u16)]
pub enum XmlDiagnosticCode {
	// === Root/Container Validation (OLS05001-OLS05010) ===
	/// Invalid root element (expected odoo, openerp, or data)
	InvalidRootElement = 5001,
	/// Invalid child element in root container
	InvalidRootChild = 5002,
	/// Deprecated openerp root element
	DeprecatedOpenerp = 5003,
	/// Invalid noupdate attribute value
	InvalidNoupdate = 5004,
	/// Invalid attribute on root/data container
	InvalidRootAttribute = 5005,

	// === Record Validation (OLS05011-OLS05030) ===
	/// Record missing required 'id' attribute
	RecordMissingId = 5011,
	/// Record missing required 'model' attribute
	RecordMissingModel = 5012,
	/// Record has invalid attribute
	RecordInvalidAttribute = 5013,
	/// Record has invalid child element
	RecordInvalidChild = 5014,
	/// Record 'model' not found in index
	RecordModelNotFound = 5015,

	// === Field Validation (OLS05031-OLS05050) ===
	/// Field missing required 'name' attribute
	FieldMissingName = 5031,
	/// Field has invalid attribute
	FieldInvalidAttribute = 5032,
	/// Field has both 'ref' and 'eval' attributes
	FieldRefAndEval = 5033,
	/// Field has both 'ref' and 'search' attributes
	FieldRefAndSearch = 5034,
	/// Field has both 'eval' and 'search' attributes
	FieldEvalAndSearch = 5035,
	/// Field 'ref' value not found
	FieldRefNotFound = 5036,
	/// Field type='base64' requires 'file' attribute
	FieldBase64MissingFile = 5037,
	/// Field has invalid 'type' value
	FieldInvalidType = 5038,
	/// Field 'name' not found in model
	FieldNameNotFound = 5039,
	/// Field has text content but also ref/eval/search
	FieldTextWithValueAttr = 5040,
	/// Field type="int" content is not a valid integer or "None"
	FieldIntContentInvalid = 5041,
	/// Field type="float" content is not a valid float
	FieldFloatContentInvalid = 5042,
	/// Field type="list"/"tuple" has invalid child (expected <value>)
	FieldListInvalidChild = 5043,
	/// Field 'model' attribute is only allowed with 'eval' or 'search'
	FieldModelWithoutEvalOrSearch = 5044,
	/// Field 'use' attribute is only allowed with 'search'
	FieldUseWithoutSearch = 5045,
	/// Field with 'file' attribute must not have text content
	FieldFileAndTextConflict = 5046,

	// === Value Validation (inside field) (OLS05051-OLS05060) ===
	/// Value element missing required 'model' attribute
	ValueMissingModel = 5051,
	/// Value element has invalid attribute
	ValueInvalidAttribute = 5052,
	/// Value element has invalid child
	ValueInvalidChild = 5053,
	/// Value 'search' attribute is invalid
	ValueInvalidSearch = 5054,
	/// Value requires either 'search' or 'eval' attribute
	ValueMissingSearchOrEval = 5055,
	/// Value 'search' conflicts with eval/type/file/text
	ValueSearchConflict = 5056,
	/// Value 'eval' conflicts with search/type/file/text
	ValueEvalConflict = 5057,
	/// Value 'type' conflicts with search/eval
	ValueTypeConflict = 5058,
	/// Value 'file' conflicts with search/eval
	ValueFileConflict = 5059,
	/// Value 'file' conflicts with text content
	ValueFileWithText = 5060,

	// === Menuitem Validation (OLS05061-OLS05080) ===
	/// Menuitem missing required 'id' attribute
	MenuitemMissingId = 5061,
	/// Menuitem has invalid attribute
	MenuitemInvalidAttribute = 5062,
	/// Menuitem should not have child elements
	MenuitemHasChildren = 5063,
	/// Menuitem action reference not found
	MenuitemActionNotFound = 5064,
	/// Menuitem parent reference not found
	MenuitemParentNotFound = 5065,
	/// Menuitem groups reference not found
	MenuitemGroupsNotFound = 5066,
	/// Menuitem has invalid 'sequence' value
	MenuitemInvalidSequence = 5067,
	/// Parent attribute is not allowed in submenuitems
	MenuitemSubmenuParentForbidden = 5068,
	/// web_icon attribute is not allowed when parent is specified or in submenus
	MenuitemSubmenuWebIconForbidden = 5069,
	/// Submenu is not allowed when action and parent attributes are defined
	MenuitemActionWithSubmenu = 5070,
	/// Invalid child node in menuitem (not a menuitem)
	MenuitemInvalidChild = 5071,

	// === Function Validation (OLS05081-OLS05090) ===
	/// Function missing required 'model' attribute
	FunctionMissingModel = 5081,
	/// Function missing required 'name' attribute
	FunctionMissingName = 5082,
	/// Function has invalid attribute
	FunctionInvalidAttribute = 5083,
	/// Function has invalid child element
	FunctionInvalidChild = 5084,
	/// Function 'model' not found
	FunctionModelNotFound = 5085,
	/// Function cannot have <value> children when 'eval' attribute is present
	FunctionEvalWithValueChild = 5086,
	/// Function cannot have <function> children when 'eval' attribute is present
	FunctionEvalWithFunctionChild = 5087,

	// === Delete Validation (OLS05091-OLS05100) ===
	/// Delete missing required 'model' attribute
	DeleteMissingModel = 5091,
	/// Delete missing 'id' or 'search' attribute
	DeleteMissingIdOrSearch = 5092,
	/// Delete has invalid attribute
	DeleteInvalidAttribute = 5093,
	/// Delete should not have child elements
	DeleteHasChildren = 5094,
	/// Delete 'id' reference not found
	DeleteIdNotFound = 5095,
	/// Delete 'model' not found
	DeleteModelNotFound = 5096,
	/// Delete cannot have both 'id' and 'search' attributes
	DeleteIdAndSearchConflict = 5097,

	// === Act_window Validation (OLS05101-OLS05120) ===
	/// Act_window missing required 'id' attribute
	ActWindowMissingId = 5101,
	/// Act_window missing required 'name' attribute
	ActWindowMissingName = 5102,
	/// Act_window missing required 'res_model' attribute
	ActWindowMissingResModel = 5103,
	/// Act_window has invalid attribute
	ActWindowInvalidAttribute = 5104,
	/// Act_window should not have child elements
	ActWindowHasChildren = 5105,
	/// Act_window 'res_model' not found
	ActWindowResModelNotFound = 5106,
	/// Act_window 'src_model' not found
	ActWindowSrcModelNotFound = 5107,
	/// Act_window has invalid 'view_mode' value
	ActWindowInvalidViewMode = 5108,
	/// Act_window has invalid 'target' value
	ActWindowInvalidTarget = 5109,
	/// Act_window has invalid 'binding_type' value
	ActWindowInvalidBindingType = 5110,
	/// Act_window 'binding_views' has invalid format
	ActWindowInvalidBindingViews = 5111,

	// === Report Validation (OLS05121-OLS05140) ===
	/// Report missing required 'id' attribute
	ReportMissingId = 5121,
	/// Report missing required 'model' attribute
	ReportMissingModel = 5122,
	/// Report missing required 'name' attribute
	ReportMissingName = 5123,
	/// Report missing 'file' or 'string' attribute
	ReportMissingFileOrString = 5124,
	/// Report has invalid attribute
	ReportInvalidAttribute = 5125,
	/// Report should not have child elements
	ReportHasChildren = 5126,
	/// Report 'model' not found
	ReportModelNotFound = 5127,
	/// Report has invalid 'report_type' value
	ReportInvalidReportType = 5128,

	// === Template Validation (OLS05141-OLS05150) ===
	/// Template missing required 'id' attribute
	TemplateMissingId = 5141,
	/// Template has invalid attribute
	TemplateInvalidAttribute = 5142,
	/// Template 'inherit_id' not found
	TemplateInheritIdNotFound = 5143,

	// === Groups Attribute Validation (OLS05151-OLS05160) ===
	/// Groups attribute contains invalid reference
	GroupsRefNotFound = 5151,
	/// Groups attribute has empty value
	GroupsEmptyValue = 5152,

	// === General XML Validation (OLS05161-OLS05180) ===
	/// Unknown element in data file
	UnknownElement = 5161,
	/// XML parse error
	XmlParseError = 5162,

	// === Extended Value Validation (OLS05171-OLS05175) ===
	/// Value 'type' requires 'file' attribute or text content
	ValueTypeRequiresFileOrText = 5171,
	/// Value is empty; requires search, eval, type, file, or text content
	ValueEmptyData = 5172,
}

impl XmlDiagnosticCode {
	/// Returns the diagnostic code string (e.g., "OLS05001").
	#[inline]
	pub fn code(&self) -> String {
		format!("OLS{:05}", *self as u16)
	}

	/// Returns the default severity for this diagnostic.
	pub fn default_severity(&self) -> DiagnosticSeverity {
		use XmlDiagnosticCode::*;
		match self {
			// Warnings (style/deprecation issues)
			DeprecatedOpenerp => DiagnosticSeverity::WARNING,
			GroupsEmptyValue => DiagnosticSeverity::WARNING,

			// Hints (optional improvements)
			TemplateInvalidAttribute => DiagnosticSeverity::HINT,

			// Errors (everything else - includes all new codes)
			_ => DiagnosticSeverity::ERROR,
		}
	}

	/// Returns the human-readable message for this diagnostic.
	pub fn message(&self) -> &'static str {
		use XmlDiagnosticCode::*;
		match self {
            // Root/Container
            InvalidRootElement => "Invalid root element; expected 'odoo', 'openerp', or 'data'",
            InvalidRootChild => "Invalid child element in data container",
            DeprecatedOpenerp => "'openerp' root element is deprecated; use 'odoo' instead",
            InvalidNoupdate => "Invalid 'noupdate' attribute value; expected '0', '1', 'true', or 'false'",
            InvalidRootAttribute => "Invalid attribute on data container",

            // Record
            RecordMissingId => "Record element missing required 'id' attribute",
            RecordMissingModel => "Record element missing required 'model' attribute",
            RecordInvalidAttribute => "Record element has invalid attribute",
            RecordInvalidChild => "Record element has invalid child; expected 'field'",
            RecordModelNotFound => "Model not found",

            // Field
            FieldMissingName => "Field element missing required 'name' attribute",
            FieldInvalidAttribute => "Field element has invalid attribute",
            FieldRefAndEval => "Field cannot have both 'ref' and 'eval' attributes",
            FieldRefAndSearch => "Field cannot have both 'ref' and 'search' attributes",
            FieldEvalAndSearch => "Field cannot have both 'eval' and 'search' attributes",
            FieldRefNotFound => "Referenced record not found",
            FieldBase64MissingFile => "Field with type='base64' requires 'file' attribute",
            FieldInvalidType => "Invalid field 'type' value; expected 'base64', 'char', 'int', 'float', 'list', 'tuple', 'html', or 'xml'",
            FieldNameNotFound => "Field name not found in model",
            FieldTextWithValueAttr => "Field has text content but also has 'ref', 'eval', or 'search' attribute",
            FieldIntContentInvalid => "Invalid content for int field",
            FieldFloatContentInvalid => "Invalid content for float field",
            FieldListInvalidChild => "Invalid child in list/tuple field; expected 'value'",
            FieldModelWithoutEvalOrSearch => "'model' attribute is only allowed on field with 'eval' or 'search' attribute",
            FieldUseWithoutSearch => "'use' attribute is only allowed on field with 'search' attribute",
            FieldFileAndTextConflict => "Text content is not allowed on a field that contains a 'file' attribute",

            // Value
            ValueMissingModel => "Value element missing required 'model' attribute",
            ValueInvalidAttribute => "Value element has invalid attribute",
            ValueInvalidChild => "Value element has invalid child",
            ValueInvalidSearch => "Value 'search' attribute has invalid domain syntax",
            ValueMissingSearchOrEval => "Value element requires 'search' or 'eval' attribute",
            ValueSearchConflict => "'search' attribute is not allowed when eval, type, file, or text content is present",
            ValueEvalConflict => "'eval' attribute is not allowed when search, type, file, or text content is present",
            ValueTypeConflict => "'type' attribute is not allowed when search or eval attribute is present",
            ValueFileConflict => "'file' attribute is not allowed when search or eval attribute is present",
            ValueFileWithText => "Text content is not allowed on a value that contains a 'file' attribute",

            // Menuitem
            MenuitemMissingId => "Menuitem element missing required 'id' attribute",
            MenuitemInvalidAttribute => "Menuitem element has invalid attribute",
            MenuitemHasChildren => "Menuitem element should not have child elements",
            MenuitemActionNotFound => "Menuitem action reference not found",
            MenuitemParentNotFound => "Menuitem parent reference not found",
            MenuitemGroupsNotFound => "Menuitem groups reference not found",
            MenuitemInvalidSequence => "Invalid 'sequence' value; expected an integer",
            MenuitemSubmenuParentForbidden => "'parent' attribute is not allowed in submenuitems",
            MenuitemSubmenuWebIconForbidden => "'web_icon' attribute is not allowed when parent is specified",
            MenuitemActionWithSubmenu => "Submenuitem is not allowed when action and parent attributes are defined on a menuitem",
            MenuitemInvalidChild => "Invalid child element in menuitem; only 'menuitem' children are allowed",

            // Function
            FunctionMissingModel => "Function element missing required 'model' attribute",
            FunctionMissingName => "Function element missing required 'name' attribute",
            FunctionInvalidAttribute => "Function element has invalid attribute",
            FunctionInvalidChild => "Function element has invalid child; expected 'value' or 'function'",
            FunctionModelNotFound => "Function model not found",
            FunctionEvalWithValueChild => "Function cannot have 'value' children when 'eval' attribute is present",
            FunctionEvalWithFunctionChild => "Function cannot have 'function' children when 'eval' attribute is present",

            // Delete
            DeleteMissingModel => "Delete element missing required 'model' attribute",
            DeleteMissingIdOrSearch => "Delete element requires 'id' or 'search' attribute",
            DeleteInvalidAttribute => "Delete element has invalid attribute",
            DeleteHasChildren => "Delete element should not have child elements",
            DeleteIdNotFound => "Delete 'id' reference not found",
            DeleteModelNotFound => "Delete model not found",
            DeleteIdAndSearchConflict => "Delete element cannot have both 'id' and 'search' attributes",

            // Act_window
            ActWindowMissingId => "Act_window element missing required 'id' attribute",
            ActWindowMissingName => "Act_window element missing required 'name' attribute",
            ActWindowMissingResModel => "Act_window element missing required 'res_model' attribute",
            ActWindowInvalidAttribute => "Act_window element has invalid attribute",
            ActWindowHasChildren => "Act_window element should not have child elements",
            ActWindowResModelNotFound => "Act_window 'res_model' not found",
            ActWindowSrcModelNotFound => "Act_window 'src_model' not found",
            ActWindowInvalidViewMode => "Invalid 'view_mode' value",
            ActWindowInvalidTarget => "Invalid 'target' value; expected 'current', 'new', 'inline', 'fullscreen', or 'main'",
            ActWindowInvalidBindingType => "Invalid 'binding_type' value; expected 'action', 'action_form_only', or 'report'",
            ActWindowInvalidBindingViews => "Invalid 'binding_views' format; expected comma-separated view types",

            // Report
            ReportMissingId => "Report element missing required 'id' attribute",
            ReportMissingModel => "Report element missing required 'model' attribute",
            ReportMissingName => "Report element missing required 'name' attribute",
            ReportMissingFileOrString => "Report element requires 'file' or 'string' attribute",
            ReportInvalidAttribute => "Report element has invalid attribute",
            ReportHasChildren => "Report element should not have child elements",
            ReportModelNotFound => "Report model not found",
            ReportInvalidReportType => "Invalid 'report_type' value; expected 'qweb-html', 'qweb-pdf', or 'qweb-text'",

            // Template
            TemplateMissingId => "Template element missing required 'id' attribute",
            TemplateInvalidAttribute => "Template element has invalid attribute",
            TemplateInheritIdNotFound => "Template 'inherit_id' reference not found",

            // Groups
            GroupsRefNotFound => "Group reference not found",
            GroupsEmptyValue => "Groups attribute has empty value",

            // General
            UnknownElement => "Unknown element in data file",
            XmlParseError => "XML parse error",

            // Extended Value
            ValueTypeRequiresFileOrText => "Empty value data; text data or 'file' attribute has to be provided when 'type' attribute is present",
            ValueEmptyData => "Empty value data; one of text data, 'file', 'eval', or 'search' has to be provided",
        }
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_code_format() {
		assert_eq!(XmlDiagnosticCode::InvalidRootElement.code(), "OLS05001");
		assert_eq!(XmlDiagnosticCode::InvalidRootAttribute.code(), "OLS05005");
		assert_eq!(XmlDiagnosticCode::RecordMissingId.code(), "OLS05011");
		assert_eq!(XmlDiagnosticCode::FieldIntContentInvalid.code(), "OLS05041");
		assert_eq!(XmlDiagnosticCode::ValueSearchConflict.code(), "OLS05056");
		assert_eq!(XmlDiagnosticCode::MenuitemSubmenuParentForbidden.code(), "OLS05068");
		assert_eq!(XmlDiagnosticCode::FunctionEvalWithValueChild.code(), "OLS05086");
		assert_eq!(XmlDiagnosticCode::DeleteIdAndSearchConflict.code(), "OLS05097");
		assert_eq!(XmlDiagnosticCode::XmlParseError.code(), "OLS05162");
		assert_eq!(XmlDiagnosticCode::ValueTypeRequiresFileOrText.code(), "OLS05171");
		assert_eq!(XmlDiagnosticCode::ValueEmptyData.code(), "OLS05172");
	}

	#[test]
	fn test_default_severity() {
		assert_eq!(
			XmlDiagnosticCode::InvalidRootElement.default_severity(),
			DiagnosticSeverity::ERROR
		);
		assert_eq!(
			XmlDiagnosticCode::DeprecatedOpenerp.default_severity(),
			DiagnosticSeverity::WARNING
		);
	}
}
