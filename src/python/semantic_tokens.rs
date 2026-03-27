//! Semantic tokens for Python files.
//!
//! Provides rich syntax highlighting for Odoo-specific constructs:
//! - Model names in strings (e.g., 'res.partner')
//! - Field type constructors (fields.Char, fields.Many2one)
//! - XML IDs in env.ref() calls
//! - Decorated method names

use std::borrow::Cow;

use ropey::RopeSlice;
use tower_lsp_server::ls_types::*;
use tracing::warn;
use tree_sitter::StreamingIterator;

use crate::backend::Backend;
use crate::index::_G;
use crate::prelude::*;
use crate::utils::uri_to_path;

use super::PyCompletions;

/// Token type indices matching the legend defined in server.rs
mod token_types {
	pub const CLASS: u32 = 0; // Model names
	pub const PROPERTY: u32 = 1; // Field names
	#[allow(dead_code)]
	pub const METHOD: u32 = 2; // Method names (reserved for future use)
	pub const DECORATOR: u32 = 3; // @api.* decorators
	pub const TYPE: u32 = 4; // Field types (fields.Char, etc.)
	pub const STRING: u32 = 5; // XML IDs in strings
}

/// Internal representation of a token before delta encoding
struct RawToken {
	line: u32,
	start_char: u32,
	length: u32,
	token_type: u32,
	modifiers: u32,
}

/// Python semantic token extensions.
impl Backend {
	/// Generate semantic tokens for a Python file.
	///
	/// If `range` is Some, only tokens within that range are returned.
	/// Tokens are delta-encoded as required by the LSP specification.
	pub fn python_semantic_tokens(
		&self,
		uri: &Uri,
		rope: RopeSlice<'_>,
		range: Option<Range>,
	) -> anyhow::Result<Option<Vec<SemanticToken>>> {
		let file_path = uri_to_path(uri)?;
		let file_path_str = file_path
			.to_str()
			.ok_or_else(|| errloc!("non-utf8 path: {:?}", file_path))?;
		let Some(ast) = self.ast_map.get(file_path_str) else {
			warn!("Did not build AST for {}", file_path_str);
			return Ok(None);
		};
		let contents = Cow::from(rope);

		// Convert range to byte range for filtering (if provided)
		let byte_range = range.map(|r| {
			let ByteOffset(start) = rope_conv(r.start, rope);
			let ByteOffset(end) = rope_conv(r.end, rope);
			start..end
		});

		let query = PyCompletions::query();
		let mut cursor = tree_sitter::QueryCursor::new();
		let mut raw_tokens = Vec::new();

		let mut matches = cursor.matches(query, ast.root_node(), contents.as_bytes());
		while let Some(match_) = matches.next() {
			for capture in match_.captures {
				let node = capture.node;
				let node_range = node.byte_range();

				// Skip if outside requested range
				if let Some(ref br) = byte_range {
					if node_range.end < br.start || node_range.start > br.end {
						continue;
					}
				}

				match PyCompletions::from(capture.index) {
					Some(PyCompletions::Model) => {
						// Model name in string - only highlight if it exists in index
						let model_str = node.utf8_text(contents.as_bytes()).unwrap_or("");
						// Remove quotes if present
						let model_name = model_str.trim_matches(|c| c == '\'' || c == '"');

						if !model_name.is_empty() {
							if let Some(model_key) = _G(model_name) {
								if self.index.models.contains_key(&model_key) {
									if let Some(token) =
										self.create_raw_token(node_range.clone(), token_types::CLASS, 0, rope)
									{
										raw_tokens.push(token);
									}
								}
							}
						}
					}
					Some(PyCompletions::FieldType) => {
						// Field type like "Char", "Many2one", etc.
						if let Some(token) = self.create_raw_token(node_range.clone(), token_types::TYPE, 0, rope) {
							raw_tokens.push(token);
						}
					}
					Some(PyCompletions::XmlId) => {
						// XML ID in env.ref() - only highlight if it exists
						let xml_id_str = node.utf8_text(contents.as_bytes()).unwrap_or("");
						let xml_id = xml_id_str.trim_matches(|c| c == '\'' || c == '"');

						if !xml_id.is_empty() {
							// Check if the XML ID exists in records
							if let Some(key) = _G(xml_id) {
								if self.index.records.get(&key).is_some() {
									if let Some(token) =
										self.create_raw_token(node_range.clone(), token_types::STRING, 0, rope)
									{
										raw_tokens.push(token);
									}
								}
							}
						}
					}
					Some(PyCompletions::Mapped) => {
						// Field path in mapped() - highlight as property
						let field_str = node.utf8_text(contents.as_bytes()).unwrap_or("");
						let field_path = field_str.trim_matches(|c| c == '\'' || c == '"');

						if !field_path.is_empty() {
							if let Some(token) =
								self.create_raw_token(node_range.clone(), token_types::PROPERTY, 0, rope)
							{
								raw_tokens.push(token);
							}
						}
					}
					Some(PyCompletions::Depends) => {
						// @api.depends, @api.constrains, @api.onchange - the decorator name
						if let Some(token) = self.create_raw_token(node_range.clone(), token_types::DECORATOR, 0, rope)
						{
							raw_tokens.push(token);
						}
					}
					Some(PyCompletions::Prop) => {
						// Field property name like "_name", "partner_id"
						if let Some(token) = self.create_raw_token(node_range.clone(), token_types::PROPERTY, 0, rope) {
							raw_tokens.push(token);
						}
					}
					_ => {}
				}
			}
		}

		if raw_tokens.is_empty() {
			return Ok(None);
		}

		// Sort tokens by position (required for delta encoding)
		raw_tokens.sort_by_key(|t| (t.line, t.start_char));

		// Delta-encode tokens
		let tokens = self.encode_tokens(raw_tokens);

		Ok(Some(tokens))
	}

	/// Create a RawToken from a byte range
	fn create_raw_token(
		&self,
		byte_range: std::ops::Range<usize>,
		token_type: u32,
		modifiers: u32,
		rope: RopeSlice<'_>,
	) -> Option<RawToken> {
		// Convert byte positions to line/character
		let start_pos: Position = rope_conv(ByteOffset(byte_range.start), rope);
		let end_pos: Position = rope_conv(ByteOffset(byte_range.end), rope);

		// For multi-line tokens, we only report the first line
		// (LSP semantic tokens don't span lines well)
		// In practice, model names and field types are always single-line
		let length = if start_pos.line == end_pos.line {
			end_pos.character - start_pos.character
		} else {
			// Skip multi-line tokens - they're not typical for our use cases
			return None;
		};

		Some(RawToken {
			line: start_pos.line,
			start_char: start_pos.character,
			length,
			token_type,
			modifiers,
		})
	}

	/// Delta-encode tokens as required by LSP
	fn encode_tokens(&self, tokens: Vec<RawToken>) -> Vec<SemanticToken> {
		let mut result = Vec::with_capacity(tokens.len());
		let mut prev_line = 0u32;
		let mut prev_start = 0u32;

		for token in tokens {
			let delta_line = token.line - prev_line;
			let delta_start = if delta_line == 0 {
				token.start_char - prev_start
			} else {
				token.start_char
			};

			result.push(SemanticToken {
				delta_line,
				delta_start,
				length: token.length,
				token_type: token.token_type,
				token_modifiers_bitset: token.modifiers,
			});

			prev_line = token.line;
			prev_start = token.start_char;
		}

		result
	}
}
