use std::borrow::Cow;
use std::collections::HashMap;

use ropey::RopeSlice;
use tower_lsp_server::ls_types::*;
use tree_sitter::StreamingIterator;
use ts_macros::query;

use crate::backend::Backend;
use crate::utils::*;

#[rustfmt::skip]
query! {
	DocSymbolQuery(Class, ClassName, Function, FunctionName, Decorator, Assignment, AssignmentTarget);

// Top-level and nested class definitions
(class_definition
  name: (identifier) @CLASS_NAME) @CLASS

// Function definitions (methods and top-level)
(function_definition
  name: (identifier) @FUNCTION_NAME) @FUNCTION

// Decorated definitions
(decorated_definition
  (decorator) @DECORATOR)

// Top-level assignments (module constants)
(module
  (expression_statement
    (assignment
      left: (identifier) @ASSIGNMENT_TARGET))) @ASSIGNMENT
}

/// Document symbol extraction for Python files.
impl Backend {
	pub fn python_document_symbols(
		&self,
		_uri: &Uri,
		rope: RopeSlice<'_>,
	) -> anyhow::Result<Option<Vec<DocumentSymbol>>> {
		let contents = Cow::from(rope);

		let mut parser = python_parser();
		let ast = parser
			.parse(contents.as_bytes(), None)
			.ok_or_else(|| anyhow::anyhow!("Failed to parse Python AST"))?;

		let query = DocSymbolQuery::query();
		let mut cursor = tree_sitter::QueryCursor::new();

		let mut top_level_symbols: Vec<DocumentSymbol> = Vec::new();
		let mut class_symbols: HashMap<usize, DocumentSymbol> = HashMap::new();
		let mut class_children: HashMap<usize, Vec<DocumentSymbol>> = HashMap::new();

		let mut matches = cursor.matches(query, ast.root_node(), contents.as_bytes());
		while let Some(match_) = matches.next() {
			for capture in match_.captures {
				match DocSymbolQuery::from(capture.index) {
					Some(DocSymbolQuery::Class) => {
						let class_node = capture.node;
						let name_node = class_node.child_by_field_name("name");
						if let Some(name_node) = name_node {
							let name = &contents[name_node.byte_range()];
							let range = span_conv(class_node.range());
							let selection_range = span_conv(name_node.range());

							#[allow(deprecated)]
							let symbol = DocumentSymbol {
								name: name.to_string(),
								detail: None,
								kind: SymbolKind::CLASS,
								tags: None,
								deprecated: None,
								range,
								selection_range,
								children: Some(Vec::new()),
							};

							class_symbols.insert(class_node.id(), symbol);
						}
					}
					Some(DocSymbolQuery::Function) => {
						let func_node = capture.node;
						let name_node = func_node.child_by_field_name("name");
						if let Some(name_node) = name_node {
							let name = &contents[name_node.byte_range()];
							let range = span_conv(func_node.range());
							let selection_range = span_conv(name_node.range());

							// Determine if this is a method (inside a class) or top-level function
							let is_method = func_node
								.parent()
								.and_then(|p| p.parent())
								.is_some_and(|gp| gp.kind() == "class_definition");

							let kind = if is_method {
								SymbolKind::METHOD
							} else {
								SymbolKind::FUNCTION
							};

							#[allow(deprecated)]
							let symbol = DocumentSymbol {
								name: name.to_string(),
								detail: None,
								kind,
								tags: None,
								deprecated: None,
								range,
								selection_range,
								children: None,
							};

							// Find parent class if this is a method
							if is_method {
								if let Some(parent_class) = func_node
									.parent()
									.and_then(|p| p.parent())
									.filter(|gp| gp.kind() == "class_definition")
								{
									class_children.entry(parent_class.id()).or_default().push(symbol);
								}
							} else {
								top_level_symbols.push(symbol);
							}
						}
					}
					Some(DocSymbolQuery::AssignmentTarget) => {
						let target_node = capture.node;
						let name = &contents[target_node.byte_range()];

						// Skip private names for cleaner outline (but keep dunder like __all__)
						if name.starts_with('_') && !name.starts_with("__") {
							continue;
						}

						// Check this is truly module-level (parent chain: assignment -> expression_statement -> module)
						let is_module_level = target_node
							.parent() // assignment
							.and_then(|p| p.parent()) // expression_statement
							.and_then(|p| p.parent()) // module
							.is_some_and(|gp| gp.kind() == "module");

						if is_module_level {
							let range = span_conv(target_node.range());

							#[allow(deprecated)]
							let symbol = DocumentSymbol {
								name: name.to_string(),
								detail: None,
								kind: SymbolKind::CONSTANT,
								tags: None,
								deprecated: None,
								range,
								selection_range: range,
								children: None,
							};

							top_level_symbols.push(symbol);
						}
					}
					_ => {}
				}
			}
		}

		// Attach methods to their parent classes
		for (class_id, mut class_symbol) in class_symbols {
			if let Some(children) = class_children.remove(&class_id) {
				class_symbol.children = Some(children);
			}
			top_level_symbols.push(class_symbol);
		}

		// Sort symbols by their position in the file
		top_level_symbols.sort_by_key(|s| (s.range.start.line, s.range.start.character));

		if top_level_symbols.is_empty() {
			Ok(None)
		} else {
			Ok(Some(top_level_symbols))
		}
	}
}
