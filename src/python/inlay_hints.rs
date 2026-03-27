use std::borrow::Cow;

use tower_lsp_server::ls_types::*;
use tracing::warn;
use tree_sitter::StreamingIterator;
use ts_macros::query;

use crate::analyze::type_cache;
use crate::backend::Backend;
use crate::index::PathSymbol;
use crate::prelude::*;
use crate::utils::uri_to_path;

use super::top_level_stmt;

#[rustfmt::skip]
query! {
	InlayHintTargets(Assignment, Target, ForTarget, ForIterable, CompTarget, CompIterable);

// Simple assignments: x = expr
(assignment
  left: (identifier) @TARGET
) @ASSIGNMENT

// For loops: for x in iterable
(for_statement
  left: (identifier) @FOR_TARGET
  right: (_) @FOR_ITERABLE)

// List/dict/set comprehensions: [x for x in iterable]
(list_comprehension
  (for_in_clause
    left: (identifier) @COMP_TARGET
    right: (_) @COMP_ITERABLE))

(dictionary_comprehension
  (for_in_clause
    left: (identifier) @COMP_TARGET
    right: (_) @COMP_ITERABLE))

(set_comprehension
  (for_in_clause
    left: (identifier) @COMP_TARGET
    right: (_) @COMP_ITERABLE))

(generator_expression
  (for_in_clause
    left: (identifier) @COMP_TARGET
    right: (_) @COMP_ITERABLE))
}

/// Python extensions for inlay hints.
impl Backend {
	pub fn python_inlay_hints(
		&self,
		uri: &Uri,
		rope: RopeSlice<'_>,
		range: Range,
	) -> anyhow::Result<Option<Vec<InlayHint>>> {
		let file_path = uri_to_path(uri)?;
		let file_path_str = file_path.to_str().ok_or_else(|| errloc!("non-utf8 path: {:?}", file_path))?;
		let Some(ast) = self.ast_map.get(file_path_str) else {
			warn!("Did not build AST for {}", file_path_str);
			return Ok(None);
		};
		let contents = Cow::from(rope);

		// Convert LSP range to byte range for filtering
		let ByteOffset(range_start) = rope_conv(range.start, rope);
		let ByteOffset(range_end) = rope_conv(range.end, rope);
		let visible_range = range_start..range_end;

		// Get path symbol for function resolution
		let current_path = self.index.find_root_of(&file_path).map(|root_path| {
			let root_spur = _I(root_path.to_string_lossy());
			PathSymbol::strip_root(root_spur, &file_path)
		});

		let query = InlayHintTargets::query();
		let mut cursor = tree_sitter::QueryCursor::new();
		let mut hints = Vec::new();

		let mut matches = cursor.matches(query, ast.root_node(), contents.as_bytes());
		while let Some(match_) = matches.next() {
			for capture in match_.captures {
				let node = capture.node;
				let byte_range = node.byte_range();

				// Skip if outside visible range
				if byte_range.end < visible_range.start || byte_range.start > visible_range.end {
					continue;
				}

				match InlayHintTargets::from(capture.index) {
					Some(InlayHintTargets::Target) => {
						// Skip if parent is annotated assignment (already has type annotation)
						if let Some(parent) = node.parent()
							&& parent.kind() == "assignment"
							&& parent.child_by_field_name("type").is_some()
						{
							continue;
						}

						// Skip `self` and `cls` parameters
						let name = &contents[byte_range.clone()];
						if matches!(name, "self" | "cls" | "_") {
							continue;
						}

						// Get the expression being assigned
						let Some(parent) = node.parent() else {
							continue;
						};
						let Some(value) = parent.child_by_field_name("right") else {
							continue;
						};

						// Get the top-level statement for scope resolution
						let root = top_level_stmt(ast.root_node(), byte_range.start).unwrap_or(ast.root_node());

						// Infer type of the right-hand side
						let type_result = self.index.type_of_range_with_path(
							root,
							value.byte_range().map_unit(ByteOffset),
							&contents,
							current_path,
						);

						if let Some((type_id, _)) = type_result {
							let type_ = type_cache().resolve(type_id);

							// Skip trivial types
							if self.should_skip_type_hint(&type_) {
								continue;
							}

							if let Some(display) = self.index.type_display(type_id) {
								let lsp_range: Range = span_conv(node.range());
								hints.push(InlayHint {
									position: lsp_range.end,
									label: InlayHintLabel::String(format!(": {display}")),
									kind: Some(InlayHintKind::TYPE),
									text_edits: None,
									tooltip: None,
									padding_left: None,
									padding_right: None,
									data: None,
								});
							}
						}
					}
					Some(InlayHintTargets::ForTarget) => {
						let name = &contents[byte_range.clone()];
						if matches!(name, "_") {
							continue;
						}

						// Get the iterable expression
						let iterable = match_
							.nodes_for_capture_index(InlayHintTargets::ForIterable as _)
							.next();

						let Some(iterable) = iterable else {
							continue;
						};

						let root = top_level_stmt(ast.root_node(), byte_range.start).unwrap_or(ast.root_node());

						// Infer type of the iterable and extract element type
						let type_result = self.index.type_of_range_with_path(
							root,
							iterable.byte_range().map_unit(ByteOffset),
							&contents,
							current_path,
						);

						if let Some((type_id, _)) = type_result {
							if let Some(element_type) = self.get_iterator_element_type(type_id) {
								if let Some(display) = self.index.type_display(element_type) {
									let lsp_range: Range = span_conv(node.range());
									hints.push(InlayHint {
										position: lsp_range.end,
										label: InlayHintLabel::String(format!(": {display}")),
										kind: Some(InlayHintKind::TYPE),
										text_edits: None,
										tooltip: None,
										padding_left: None,
										padding_right: None,
										data: None,
									});
								}
							}
						}
					}
					Some(InlayHintTargets::CompTarget) => {
						let name = &contents[byte_range.clone()];
						if matches!(name, "_") {
							continue;
						}

						// Get the iterable expression for comprehension
						let iterable = match_
							.nodes_for_capture_index(InlayHintTargets::CompIterable as _)
							.next();

						let Some(iterable) = iterable else {
							continue;
						};

						let root = top_level_stmt(ast.root_node(), byte_range.start).unwrap_or(ast.root_node());

						let type_result = self.index.type_of_range_with_path(
							root,
							iterable.byte_range().map_unit(ByteOffset),
							&contents,
							current_path,
						);

						if let Some((type_id, _)) = type_result {
							if let Some(element_type) = self.get_iterator_element_type(type_id) {
								if let Some(display) = self.index.type_display(element_type) {
									let lsp_range: Range = span_conv(node.range());
									hints.push(InlayHint {
										position: lsp_range.end,
										label: InlayHintLabel::String(format!(": {display}")),
										kind: Some(InlayHintKind::TYPE),
										text_edits: None,
										tooltip: None,
										padding_left: None,
										padding_right: None,
										data: None,
									});
								}
							}
						}
					}
					_ => {}
				}
			}
		}

		if hints.is_empty() {
			Ok(None)
		} else {
			Ok(Some(hints))
		}
	}

	/// Check if a type should not be shown as an inlay hint
	fn should_skip_type_hint(&self, type_: &crate::analyze::Type) -> bool {
		use crate::analyze::Type;
		match type_ {
			// Skip Value (unknown type)
			Type::Value => true,
			// Skip None type
			Type::None => true,
			// Allow models, builtins, lists, dicts, etc.
			_ => false,
		}
	}

	/// Extract the element type from an iterable type
	fn get_iterator_element_type(&self, type_id: crate::analyze::TypeId) -> Option<crate::analyze::TypeId> {
		use crate::analyze::{ListElement, Type};
		let type_ = type_cache().resolve(type_id);

		match type_ {
			// List[T] -> T
			Type::List(element) => match element {
				ListElement::Occupied(t) => Some(*t),
				ListElement::Vacant => None,
			},
			// Model recordset iteration yields the same model type
			Type::Model(_) => Some(type_id),
			// For dict iteration, yield the key type
			Type::Dict(key, _) => Some(*key),
			// Iterable[T] -> T
			Type::Iterable(inner) => *inner,
			// Tuple iteration - could yield union of element types, but skip for now
			_ => None,
		}
	}
}
