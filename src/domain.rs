//! Domain expression support for Odoo.
//!
//! This module provides utilities for validating and processing Odoo domain expressions.
//! Domain expressions are used in Odoo to filter records, with syntax like:
//!
//! ```python
//! [('field', 'operator', value)]
//! ['&', ('field1', '=', 'a'), ('field2', '=', 'b')]
//! [('relational_field', 'any', [('subfield', '=', 'value')])]
//! ```
//!
//! Reference: `/opt/odoo18/odoo/odoo/osv/expression.py`

use phf::{phf_map, phf_set};
use tower_lsp_server::ls_types::CompletionItem;

/// Valid domain term operators.
///
/// These operators are used in domain tuples: `(field_name, operator, value)`
///
/// From Odoo's `expression.py`:
/// ```python
/// TERM_OPERATORS = ('=', '!=', '<=', '<', '>', '>=', '=?', '=like', '=ilike',
///                   'like', 'not like', 'ilike', 'not ilike', 'in', 'not in',
///                   'child_of', 'parent_of', 'any', 'not any')
/// ```
pub static TERM_OPERATORS: phf::Set<&'static str> = phf_set! {
    "=",
    "!=",
    "<>",  // Legacy, normalized to "!="
    "<=",
    "<",
    ">",
    ">=",
    "=?",
    "=like",
    "=ilike",
    "like",
    "not like",
    "ilike",
    "not ilike",
    "in",
    "not in",
    "child_of",
    "parent_of",
    "any",
    "not any",
};

/// Operators that require a subdomain (list) as the value.
///
/// These operators are used to filter on relational fields (Many2one, One2many, Many2many)
/// where the value is itself a domain expression applied to the related model.
///
/// Example:
/// ```python
/// [('child_ids', 'any', [('name', '=', 'John')])]
/// [('tag_ids', 'not any', [('color', '>', 5)])]
/// ```
pub static SUBDOMAIN_OPERATORS: phf::Set<&'static str> = phf_set! {
    "any",
    "not any",
};

/// Domain-level boolean operators (prefix notation).
///
/// These operators combine domain terms:
/// - `&`: AND (default, arity 2)
/// - `|`: OR (arity 2)
/// - `!`: NOT (arity 1)
///
/// Example:
/// ```python
/// ['&', ('state', '=', 'draft'), ('active', '=', True)]
/// ['|', ('state', '=', 'draft'), ('state', '=', 'sent')]
/// ['!', ('active', '=', True)]
/// ```
pub static DOMAIN_OPERATORS: phf::Set<&'static str> = phf_set! {
    "!",
    "|",
    "&",
};

/// Default maximum nesting depth for subdomain recursion.
///
/// This prevents infinite recursion in case of malformed domain expressions
/// or circular references.
pub const DEFAULT_MAX_DOMAIN_DEPTH: usize = 10;

// ============================================================================
// Operator Categories for Field Type Compatibility
// ============================================================================

/// String pattern matching operators - primarily for Char/Text fields.
/// Can work on other types but most useful with strings.
pub static STRING_OPERATORS: phf::Set<&'static str> = phf_set! {
    "like",
    "not like",
    "ilike",
    "not ilike",
    "=like",
    "=ilike",
};

/// Hierarchical operators - only for relational fields with parent/child hierarchy.
/// Typically used with Many2one fields that have parent_path or nested set.
pub static HIERARCHY_OPERATORS: phf::Set<&'static str> = phf_set! {
    "child_of",
    "parent_of",
};

/// List operators - require a list/tuple as value.
pub static LIST_VALUE_OPERATORS: phf::Set<&'static str> = phf_set! {
    "in",
    "not in",
};

/// Comparison operators - work with ordered types (numbers, dates).
pub static COMPARISON_OPERATORS: phf::Set<&'static str> = phf_set! {
    "<",
    "<=",
    ">",
    ">=",
};

/// Universal operators - work with all field types.
pub static UNIVERSAL_OPERATORS: phf::Set<&'static str> = phf_set! {
    "=",
    "!=",
    "=?",
};

// ============================================================================
// Operator Documentation
// ============================================================================

/// Detailed information about each domain operator.
#[derive(Debug, Clone, Copy)]
pub struct OperatorInfo {
    /// The operator string
    pub operator: &'static str,
    /// Human-readable description
    pub description: &'static str,
    /// Example usage
    pub example: &'static str,
    /// Expected value type
    pub value_type: &'static str,
    /// Category for sorting/grouping
    pub category: OperatorCategory,
}

/// Categories for grouping operators in completions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum OperatorCategory {
    /// Basic comparison (=, !=)
    Equality,
    /// Ordered comparison (<, <=, >, >=)
    Comparison,
    /// String pattern matching (like, ilike)
    Pattern,
    /// Set membership (in, not in)
    Membership,
    /// Hierarchy traversal (child_of, parent_of)
    Hierarchy,
    /// Subdomain operators (any, not any)
    Subdomain,
    /// Special operators (=?)
    Special,
}

impl OperatorCategory {
    pub fn label(&self) -> &'static str {
        match self {
            OperatorCategory::Equality => "Equality",
            OperatorCategory::Comparison => "Comparison",
            OperatorCategory::Pattern => "Pattern matching",
            OperatorCategory::Membership => "Set membership",
            OperatorCategory::Hierarchy => "Hierarchy",
            OperatorCategory::Subdomain => "Subdomain",
            OperatorCategory::Special => "Special",
        }
    }
}

/// Complete operator documentation with all details.
pub static OPERATOR_INFO: phf::Map<&'static str, OperatorInfo> = phf_map! {
    "=" => OperatorInfo {
        operator: "=",
        description: "Equals - checks if field value equals the given value",
        example: r#"('state', '=', 'draft')"#,
        value_type: "any",
        category: OperatorCategory::Equality,
    },
    "!=" => OperatorInfo {
        operator: "!=",
        description: "Not equals - checks if field value differs from given value",
        example: r#"('state', '!=', 'cancelled')"#,
        value_type: "any",
        category: OperatorCategory::Equality,
    },
    "<" => OperatorInfo {
        operator: "<",
        description: "Less than - checks if field value is less than given value",
        example: r#"('amount', '<', 1000)"#,
        value_type: "number, date, datetime",
        category: OperatorCategory::Comparison,
    },
    "<=" => OperatorInfo {
        operator: "<=",
        description: "Less than or equal - checks if field value is at most given value",
        example: r#"('date', '<=', '2024-12-31')"#,
        value_type: "number, date, datetime",
        category: OperatorCategory::Comparison,
    },
    ">" => OperatorInfo {
        operator: ">",
        description: "Greater than - checks if field value exceeds given value",
        example: r#"('quantity', '>', 0)"#,
        value_type: "number, date, datetime",
        category: OperatorCategory::Comparison,
    },
    ">=" => OperatorInfo {
        operator: ">=",
        description: "Greater than or equal - checks if field value is at least given value",
        example: r#"('date', '>=', '2024-01-01')"#,
        value_type: "number, date, datetime",
        category: OperatorCategory::Comparison,
    },
    "=?" => OperatorInfo {
        operator: "=?",
        description: "Equals if set - returns True if value is False/None, otherwise acts like '='",
        example: r#"('partner_id', '=?', partner_id)"#,
        value_type: "any or False/None",
        category: OperatorCategory::Special,
    },
    "like" => OperatorInfo {
        operator: "like",
        description: "SQL LIKE - case-sensitive pattern match with % and _ wildcards",
        example: r#"('name', 'like', 'John%')"#,
        value_type: "string pattern",
        category: OperatorCategory::Pattern,
    },
    "not like" => OperatorInfo {
        operator: "not like",
        description: "SQL NOT LIKE - negated case-sensitive pattern match",
        example: r#"('email', 'not like', '%@spam.com')"#,
        value_type: "string pattern",
        category: OperatorCategory::Pattern,
    },
    "ilike" => OperatorInfo {
        operator: "ilike",
        description: "Case-insensitive LIKE - pattern match ignoring case",
        example: r#"('name', 'ilike', 'john%')"#,
        value_type: "string pattern",
        category: OperatorCategory::Pattern,
    },
    "not ilike" => OperatorInfo {
        operator: "not ilike",
        description: "Case-insensitive NOT LIKE - negated pattern match ignoring case",
        example: r#"('name', 'not ilike', 'test%')"#,
        value_type: "string pattern",
        category: OperatorCategory::Pattern,
    },
    "=like" => OperatorInfo {
        operator: "=like",
        description: "Exact LIKE - pattern match without auto-wrapping value in %",
        example: r#"('code', '=like', 'ABC%')"#,
        value_type: "string pattern",
        category: OperatorCategory::Pattern,
    },
    "=ilike" => OperatorInfo {
        operator: "=ilike",
        description: "Exact case-insensitive LIKE - pattern match without auto-wrapping",
        example: r#"('code', '=ilike', 'abc%')"#,
        value_type: "string pattern",
        category: OperatorCategory::Pattern,
    },
    "in" => OperatorInfo {
        operator: "in",
        description: "In list - checks if field value is in the given list",
        example: r#"('state', 'in', ['draft', 'sent'])"#,
        value_type: "list/tuple",
        category: OperatorCategory::Membership,
    },
    "not in" => OperatorInfo {
        operator: "not in",
        description: "Not in list - checks if field value is not in the given list",
        example: r#"('state', 'not in', ['cancelled', 'done'])"#,
        value_type: "list/tuple",
        category: OperatorCategory::Membership,
    },
    "child_of" => OperatorInfo {
        operator: "child_of",
        description: "Child of - matches records that are descendants of given parent(s)",
        example: r#"('parent_id', 'child_of', parent.id)"#,
        value_type: "id, list of ids, or False",
        category: OperatorCategory::Hierarchy,
    },
    "parent_of" => OperatorInfo {
        operator: "parent_of",
        description: "Parent of - matches records that are ancestors of given child(ren)",
        example: r#"('id', 'parent_of', child_ids.ids)"#,
        value_type: "id, list of ids, or False",
        category: OperatorCategory::Hierarchy,
    },
    "any" => OperatorInfo {
        operator: "any",
        description: "Any match - checks if any related record matches the subdomain",
        example: r#"('order_line_ids', 'any', [('price', '>', 100)])"#,
        value_type: "domain (list)",
        category: OperatorCategory::Subdomain,
    },
    "not any" => OperatorInfo {
        operator: "not any",
        description: "No match - checks that no related record matches the subdomain",
        example: r#"('tag_ids', 'not any', [('name', '=', 'Urgent')])"#,
        value_type: "domain (list)",
        category: OperatorCategory::Subdomain,
    },
};

// ============================================================================
// Field Type to Valid Operators Mapping
// ============================================================================

/// Odoo field type categories for operator validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldTypeCategory {
    /// Boolean fields
    Boolean,
    /// Integer, Float, Monetary fields
    Numeric,
    /// Char, Text, Html fields
    String,
    /// Date fields
    Date,
    /// Datetime fields
    Datetime,
    /// Selection fields
    Selection,
    /// Binary fields
    Binary,
    /// Many2one, One2many, Many2many fields
    Relational,
    /// Properties, Json fields
    Structured,
    /// Unknown/other field types
    Unknown,
}

impl FieldTypeCategory {
    /// Categorize a field type string from Odoo.
    pub fn from_field_type(type_str: &str) -> Self {
        match type_str {
            "Boolean" => Self::Boolean,
            "Integer" | "Float" | "Monetary" => Self::Numeric,
            "Char" | "Text" | "Html" => Self::String,
            "Date" => Self::Date,
            "Datetime" => Self::Datetime,
            "Selection" => Self::Selection,
            "Binary" | "Image" => Self::Binary,
            "Many2one" | "One2many" | "Many2many" => Self::Relational,
            "Properties" | "PropertiesDefinition" | "Json" => Self::Structured,
            _ => Self::Unknown,
        }
    }

    /// Get the operators that are valid/recommended for this field type.
    /// Returns (valid_operators, warning_operators) where warning_operators
    /// are technically valid but unusual for this field type.
    pub fn valid_operators(&self) -> (&'static [&'static str], &'static [&'static str]) {
        match self {
            Self::Boolean => (
                // Boolean: mainly equality
                &["=", "!=", "in", "not in"],
                &[], // No warnings
            ),
            Self::Numeric => (
                // Numbers: equality, comparison, membership
                &["=", "!=", "<", "<=", ">", ">=", "=?", "in", "not in"],
                &["like", "ilike", "not like", "not ilike"], // Unusual for numbers
            ),
            Self::String => (
                // Strings: all operators except subdomain/hierarchy
                &["=", "!=", "=?", "like", "not like", "ilike", "not ilike", 
                  "=like", "=ilike", "in", "not in", "<", "<=", ">", ">="],
                &[], // All make sense for strings
            ),
            Self::Date | Self::Datetime => (
                // Dates: equality, comparison, membership
                &["=", "!=", "<", "<=", ">", ">=", "=?", "in", "not in"],
                &["like", "ilike", "not like", "not ilike"], // Unusual for dates
            ),
            Self::Selection => (
                // Selection: equality, membership
                &["=", "!=", "=?", "in", "not in"],
                &["like", "ilike", "not like", "not ilike", "<", "<=", ">", ">="],
            ),
            Self::Binary => (
                // Binary: limited operators
                &["=", "!="],
                &["in", "not in"],
            ),
            Self::Relational => (
                // Relational: equality, membership, hierarchy, subdomain
                &["=", "!=", "=?", "in", "not in", "child_of", "parent_of", "any", "not any"],
                &["like", "ilike", "not like", "not ilike"], // Very unusual
            ),
            Self::Structured => (
                // Json/Properties: limited
                &["=", "!="],
                &[],
            ),
            Self::Unknown => (
                // Unknown: allow all, no warnings
                &["=", "!=", "<", "<=", ">", ">=", "=?", "like", "not like", 
                  "ilike", "not ilike", "=like", "=ilike", "in", "not in",
                  "child_of", "parent_of", "any", "not any"],
                &[],
            ),
        }
    }

    /// Check if an operator is valid for this field type.
    /// Returns (is_valid, is_warning) where is_warning means it's unusual but allowed.
    pub fn check_operator(&self, op: &str) -> (bool, bool) {
        let op_lower = op.to_lowercase();
        let op_str = normalize_operator(&op_lower);
        
        let (valid, warning) = self.valid_operators();
        
        if valid.iter().any(|&v| v == op_str) {
            // Operator is recommended for this field type
            (true, false)
        } else if warning.iter().any(|&w| w == op_str) {
            // Operator is explicitly marked as unusual/warning for this field type
            (true, true)
        } else if is_valid_operator(op) {
            // Operator is valid globally but not recommended/warned for this field type
            // This means it's unusual - emit a warning
            (true, true)
        } else {
            // Operator is completely invalid
            (false, false)
        }
    }
}

// ============================================================================
// Domain-Level Operator Arity
// ============================================================================

/// Get the arity (number of operands) for a domain-level operator.
/// Returns `None` for invalid operators.
pub fn domain_operator_arity(op: &str) -> Option<usize> {
    match op {
        "!" => Some(1),
        "&" | "|" => Some(2),
        _ => None,
    }
}

// ============================================================================
// Validation Functions
// ============================================================================

/// Check if an operator string is a valid domain term operator.
///
/// The check is case-insensitive and handles the legacy `<>` operator.
#[inline]
pub fn is_valid_operator(op: &str) -> bool {
    let normalized = op.to_lowercase();
    TERM_OPERATORS.contains(normalized.as_str())
}

/// Check if an operator requires a subdomain (list) as its value.
///
/// Returns `true` for `any` and `not any` operators.
#[inline]
pub fn is_subdomain_operator(op: &str) -> bool {
    let normalized = op.to_lowercase();
    SUBDOMAIN_OPERATORS.contains(normalized.as_str())
}

/// Check if a string is a domain-level boolean operator.
///
/// Returns `true` for `&`, `|`, and `!`.
#[inline]
pub fn is_domain_operator(op: &str) -> bool {
    DOMAIN_OPERATORS.contains(op)
}

/// Check if an operator is a string pattern operator.
#[inline]
pub fn is_string_operator(op: &str) -> bool {
    let normalized = op.to_lowercase();
    STRING_OPERATORS.contains(normalized.as_str())
}

/// Check if an operator is a hierarchy operator.
#[inline]
pub fn is_hierarchy_operator(op: &str) -> bool {
    let normalized = op.to_lowercase();
    HIERARCHY_OPERATORS.contains(normalized.as_str())
}

/// Check if an operator requires a list value.
#[inline]
pub fn is_list_value_operator(op: &str) -> bool {
    let normalized = op.to_lowercase();
    LIST_VALUE_OPERATORS.contains(normalized.as_str()) || SUBDOMAIN_OPERATORS.contains(normalized.as_str())
}

/// Normalize an operator to its canonical form.
///
/// Currently only handles the legacy `<>` operator, converting it to `!=`.
#[inline]
pub fn normalize_operator(op: &str) -> &str {
    if op == "<>" {
        "!="
    } else {
        op
    }
}

/// Format a human-readable list of valid operators for error messages.
pub fn format_valid_operators() -> String {
    let mut operators: Vec<_> = TERM_OPERATORS
        .iter()
        .filter(|&&op| op != "<>") // Exclude legacy operator from display
        .copied()
        .collect();
    operators.sort();
    operators.join(", ")
}

// ============================================================================
// Completion Support
// ============================================================================

/// Get operator completion items, optionally filtered by field type.
/// 
/// Returns a list of completion items sorted by category and relevance.
pub fn get_operator_completions(field_type: Option<FieldTypeCategory>) -> Vec<CompletionItem> {
    use tower_lsp_server::ls_types::{CompletionItemKind, CompletionItemLabelDetails, Documentation, MarkupContent, MarkupKind};
    
    let mut items: Vec<CompletionItem> = Vec::with_capacity(20);
    
    // Collect all operators with their info, sorted by category then operator
    let mut operators: Vec<_> = OPERATOR_INFO.entries().collect();
    operators.sort_by(|a, b| {
        a.1.category.cmp(&b.1.category)
            .then_with(|| a.0.cmp(b.0))
    });
    
    for (op, info) in operators {
        // Skip legacy operator
        if *op == "<>" {
            continue;
        }
        
        // Check if operator is recommended for the field type
        let (is_recommended, is_warning) = field_type
            .map(|ft| ft.check_operator(op))
            .unwrap_or((true, false));
        
        // Calculate sort text - recommended operators first, then by category
        let sort_prefix = if is_warning {
            "2" // Unusual operators
        } else if !is_recommended {
            "3" // Not recommended
        } else {
            "1" // Recommended
        };
        let sort_text = Some(format!("{}{:02}{}", sort_prefix, info.category as u8, op));
        
        // Build documentation
        let mut doc = format!("**{}**\n\n{}", info.description, info.example);
        if is_warning {
            doc.push_str("\n\n⚠️ *Unusual for this field type*");
        }
        doc.push_str(&format!("\n\n**Expected value:** {}", info.value_type));
        
        items.push(CompletionItem {
            label: op.to_string(),
            kind: Some(CompletionItemKind::OPERATOR),
            detail: Some(info.category.label().to_string()),
            label_details: Some(CompletionItemLabelDetails {
                detail: None,
                description: Some(info.description.split(" - ").next().unwrap_or(info.description).to_string()),
            }),
            documentation: Some(Documentation::MarkupContent(MarkupContent {
                kind: MarkupKind::Markdown,
                value: doc,
            })),
            sort_text,
            deprecated: Some(*op == "<>"),
            ..Default::default()
        });
    }
    
    items
}

/// Get hover documentation for a domain operator.
pub fn get_operator_hover(op: &str) -> Option<String> {
    let normalized = op.to_lowercase();
    let normalized_str = normalize_operator(&normalized);
    
    OPERATOR_INFO.get(normalized_str).map(|info| {
        format!(
            "**Domain Operator: `{}`**\n\n\
            {}\n\n\
            **Category:** {}\n\n\
            **Example:**\n```python\n{}\n```\n\n\
            **Expected value:** {}",
            info.operator,
            info.description,
            info.category.label(),
            info.example,
            info.value_type
        )
    })
}

/// Get hover documentation for a domain-level boolean operator.
pub fn get_domain_operator_hover(op: &str) -> Option<String> {
    match op {
        "&" => Some(
            "**Domain Operator: `&` (AND)**\n\n\
            Binary AND operator in Polish (prefix) notation.\n\
            Combines the next two terms/operators with logical AND.\n\n\
            **Arity:** 2\n\n\
            **Example:**\n```python\n['&', ('state', '=', 'draft'), ('active', '=', True)]\n```\n\n\
            Equivalent to: `state = 'draft' AND active = True`"
            .to_string()
        ),
        "|" => Some(
            "**Domain Operator: `|` (OR)**\n\n\
            Binary OR operator in Polish (prefix) notation.\n\
            Combines the next two terms/operators with logical OR.\n\n\
            **Arity:** 2\n\n\
            **Example:**\n```python\n['|', ('state', '=', 'draft'), ('state', '=', 'sent')]\n```\n\n\
            Equivalent to: `state = 'draft' OR state = 'sent`"
            .to_string()
        ),
        "!" => Some(
            "**Domain Operator: `!` (NOT)**\n\n\
            Unary NOT operator in Polish (prefix) notation.\n\
            Negates the next term/operator.\n\n\
            **Arity:** 1\n\n\
            **Example:**\n```python\n['!', ('active', '=', True)]\n```\n\n\
            Equivalent to: `NOT active = True` (i.e., `active = False`)"
            .to_string()
        ),
        _ => None,
    }
}

// ============================================================================
// Domain Structure Validation
// ============================================================================

/// Result of validating domain structure.
#[derive(Debug)]
pub struct DomainValidation {
    /// Whether the domain structure is valid
    pub is_valid: bool,
    /// Error message if invalid
    pub error: Option<String>,
    /// Position of the error (if applicable)
    pub error_position: Option<usize>,
}

/// Validate the structure of a domain expression.
/// 
/// This checks:
/// - Correct arity for domain-level operators (&, |, !)
/// - Proper nesting of terms and operators
/// - No dangling operators
/// 
/// Returns the expected number of remaining terms (should be 0 for valid domain).
pub fn validate_domain_structure(elements: &[DomainElement]) -> DomainValidation {
    if elements.is_empty() {
        return DomainValidation {
            is_valid: true,
            error: None,
            error_position: None,
        };
    }
    
    let mut expected: i32 = 1; // Expected number of expressions
    
    for (i, element) in elements.iter().enumerate() {
        if expected == 0 {
            // More elements than expected - implicit AND needed
            expected = 1;
        }
        
        match element {
            DomainElement::Term => {
                expected -= 1;
            }
            DomainElement::Operator(op) => {
                match domain_operator_arity(op) {
                    Some(arity) => {
                        expected += arity as i32 - 1;
                    }
                    None => {
                        return DomainValidation {
                            is_valid: false,
                            error: Some(format!("Invalid domain operator: '{}'", op)),
                            error_position: Some(i),
                        };
                    }
                }
            }
        }
    }
    
    if expected != 0 {
        DomainValidation {
            is_valid: false,
            error: Some(format!(
                "Domain is syntactically incorrect: {} more term(s) expected",
                expected
            )),
            error_position: None,
        }
    } else {
        DomainValidation {
            is_valid: true,
            error: None,
            error_position: None,
        }
    }
}

/// Represents an element in a domain expression for structure validation.
#[derive(Debug, Clone)]
pub enum DomainElement {
    /// A domain term (tuple like ('field', 'op', value))
    Term,
    /// A domain-level operator (&, |, !)
    Operator(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_operators() {
        // Standard operators
        assert!(is_valid_operator("="));
        assert!(is_valid_operator("!="));
        assert!(is_valid_operator("<"));
        assert!(is_valid_operator("<="));
        assert!(is_valid_operator(">"));
        assert!(is_valid_operator(">="));
        assert!(is_valid_operator("like"));
        assert!(is_valid_operator("ilike"));
        assert!(is_valid_operator("in"));
        assert!(is_valid_operator("not in"));

        // Legacy operator
        assert!(is_valid_operator("<>"));

        // Case insensitivity
        assert!(is_valid_operator("LIKE"));
        assert!(is_valid_operator("NOT IN"));
        assert!(is_valid_operator("Any"));
        assert!(is_valid_operator("NOT ANY"));

        // Subdomain operators
        assert!(is_valid_operator("any"));
        assert!(is_valid_operator("not any"));

        // Hierarchical operators
        assert!(is_valid_operator("child_of"));
        assert!(is_valid_operator("parent_of"));

        // Invalid operators
        assert!(!is_valid_operator("foo"));
        assert!(!is_valid_operator("equals"));
        assert!(!is_valid_operator("contains"));
        assert!(!is_valid_operator(""));
    }

    #[test]
    fn test_subdomain_operators() {
        assert!(is_subdomain_operator("any"));
        assert!(is_subdomain_operator("not any"));
        assert!(is_subdomain_operator("ANY"));
        assert!(is_subdomain_operator("NOT ANY"));

        assert!(!is_subdomain_operator("="));
        assert!(!is_subdomain_operator("in"));
        assert!(!is_subdomain_operator("child_of"));
    }

    #[test]
    fn test_domain_operators() {
        assert!(is_domain_operator("&"));
        assert!(is_domain_operator("|"));
        assert!(is_domain_operator("!"));

        assert!(!is_domain_operator("="));
        assert!(!is_domain_operator("and"));
        assert!(!is_domain_operator("or"));
    }

    #[test]
    fn test_normalize_operator() {
        assert_eq!(normalize_operator("<>"), "!=");
        assert_eq!(normalize_operator("="), "=");
        assert_eq!(normalize_operator("like"), "like");
    }

    #[test]
    fn test_field_type_category() {
        assert_eq!(FieldTypeCategory::from_field_type("Boolean"), FieldTypeCategory::Boolean);
        assert_eq!(FieldTypeCategory::from_field_type("Integer"), FieldTypeCategory::Numeric);
        assert_eq!(FieldTypeCategory::from_field_type("Float"), FieldTypeCategory::Numeric);
        assert_eq!(FieldTypeCategory::from_field_type("Char"), FieldTypeCategory::String);
        assert_eq!(FieldTypeCategory::from_field_type("Text"), FieldTypeCategory::String);
        assert_eq!(FieldTypeCategory::from_field_type("Date"), FieldTypeCategory::Date);
        assert_eq!(FieldTypeCategory::from_field_type("Datetime"), FieldTypeCategory::Datetime);
        assert_eq!(FieldTypeCategory::from_field_type("Selection"), FieldTypeCategory::Selection);
        assert_eq!(FieldTypeCategory::from_field_type("Many2one"), FieldTypeCategory::Relational);
        assert_eq!(FieldTypeCategory::from_field_type("One2many"), FieldTypeCategory::Relational);
        assert_eq!(FieldTypeCategory::from_field_type("Many2many"), FieldTypeCategory::Relational);
    }

    #[test]
    fn test_operator_field_compatibility() {
        // String operators should warn on numeric fields
        let (valid, warning) = FieldTypeCategory::Numeric.check_operator("like");
        assert!(valid);
        assert!(warning);

        // Equality should be fine everywhere
        let (valid, warning) = FieldTypeCategory::Numeric.check_operator("=");
        assert!(valid);
        assert!(!warning);

        // Subdomain operators only for relational
        let (valid, _) = FieldTypeCategory::Relational.check_operator("any");
        assert!(valid);

        // 'any' is not in the valid list for Boolean but is_valid_operator returns true
        let (valid, _) = FieldTypeCategory::Boolean.check_operator("any");
        assert!(valid); // It's a valid operator in general
    }

    #[test]
    fn test_domain_structure_validation() {
        // Valid: simple term
        let result = validate_domain_structure(&[DomainElement::Term]);
        assert!(result.is_valid);

        // Valid: AND with two terms
        let result = validate_domain_structure(&[
            DomainElement::Operator("&".to_string()),
            DomainElement::Term,
            DomainElement::Term,
        ]);
        assert!(result.is_valid);

        // Valid: NOT with one term
        let result = validate_domain_structure(&[
            DomainElement::Operator("!".to_string()),
            DomainElement::Term,
        ]);
        assert!(result.is_valid);

        // Valid: complex nested
        let result = validate_domain_structure(&[
            DomainElement::Operator("&".to_string()),
            DomainElement::Operator("!".to_string()),
            DomainElement::Term,
            DomainElement::Operator("|".to_string()),
            DomainElement::Term,
            DomainElement::Term,
        ]);
        assert!(result.is_valid);

        // Invalid: AND with only one term
        let result = validate_domain_structure(&[
            DomainElement::Operator("&".to_string()),
            DomainElement::Term,
        ]);
        assert!(!result.is_valid);

        // Invalid: NOT with no term
        let result = validate_domain_structure(&[
            DomainElement::Operator("!".to_string()),
        ]);
        assert!(!result.is_valid);
    }

    #[test]
    fn test_operator_hover() {
        let hover = get_operator_hover("=");
        assert!(hover.is_some());
        assert!(hover.unwrap().contains("Equals"));

        let hover = get_operator_hover("any");
        assert!(hover.is_some());
        assert!(hover.unwrap().contains("subdomain"));

        let hover = get_operator_hover("invalid");
        assert!(hover.is_none());
    }

    #[test]
    fn test_domain_operator_hover() {
        let hover = get_domain_operator_hover("&");
        assert!(hover.is_some());
        assert!(hover.unwrap().contains("AND"));

        let hover = get_domain_operator_hover("|");
        assert!(hover.is_some());
        assert!(hover.unwrap().contains("OR"));

        let hover = get_domain_operator_hover("!");
        assert!(hover.is_some());
        assert!(hover.unwrap().contains("NOT"));

        let hover = get_domain_operator_hover("=");
        assert!(hover.is_none());
    }

    #[test]
    fn test_operator_completions() {
        let completions = get_operator_completions(None);
        assert!(!completions.is_empty());
        
        // Should have all operators except <>
        assert!(completions.iter().any(|c| c.label == "="));
        assert!(completions.iter().any(|c| c.label == "any"));
        assert!(!completions.iter().any(|c| c.label == "<>"));
    }
}
