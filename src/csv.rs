//! LSP support for Odoo CSV files (ir.model.access.csv).
//!
//! This module provides completions, hover, and diagnostics for Odoo security CSV files.

use std::borrow::Cow;

use tower_lsp_server::ls_types::*;

use crate::backend::Backend;
use crate::index::{_G, _R};
use crate::prelude::*;
use crate::utils::MaxVec;

/// Column indices in ir.model.access.csv
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CsvColumn {
	Id,
	Name,
	ModelId,
	GroupId,
	PermRead,
	PermWrite,
	PermCreate,
	PermUnlink,
}

/// Information about the cursor position within a CSV file
#[derive(Debug)]
pub struct CsvCursorContext<'a> {
	/// The current line number (0-indexed)
	pub line: usize,
	/// The current column index (0-indexed within the CSV structure)
	pub column_idx: usize,
	/// Which CSV column type this is
	pub column_type: Option<CsvColumn>,
	/// The current cell value
	pub cell_value: &'a str,
	/// The range of the current cell in bytes
	pub cell_range: core::ops::Range<usize>,
	/// Offset within the cell
	pub offset_in_cell: usize,
	/// Headers parsed from the first line
	pub headers: Vec<&'a str>,
}

impl Backend {
	/// Provide completions for CSV files (ir.model.access.csv)
	pub async fn csv_completions(
		&self,
		params: CompletionParams,
		rope: RopeSlice<'_>,
	) -> anyhow::Result<Option<CompletionResponse>> {
		let ByteOffset(offset) = rope_conv(params.text_document_position.position, rope);
		let contents = Cow::from(rope);

		let Some(context) = self.parse_csv_cursor_context(offset, &contents) else {
			return Ok(None);
		};

		// Skip the header line
		if context.line == 0 {
			return Ok(None);
		}

		let path = params.text_document_position.text_document.uri.to_file_path();
		let current_module = path
			.as_ref()
			.and_then(|p| self.index.find_module_of(p));

		match context.column_type {
			Some(CsvColumn::ModelId) => {
				self.complete_csv_model_id(&context, &contents, rope).await
			}
			Some(CsvColumn::GroupId) => {
				self.complete_csv_group_id(&context, &contents, rope, current_module).await
			}
			_ => Ok(None),
		}
	}

	/// Parse the cursor context for a CSV file
	fn parse_csv_cursor_context<'a>(&self, offset: usize, contents: &'a str) -> Option<CsvCursorContext<'a>> {
		let lines: Vec<&str> = contents.lines().collect();
		if lines.is_empty() {
			return None;
		}

		// Parse headers from first line
		let headers: Vec<&str> = lines[0].split(',').map(|s| s.trim()).collect();

		// Find which line the cursor is on
		let mut current_offset = 0;
		let mut line_idx = 0;
		for (idx, line) in lines.iter().enumerate() {
			let line_end = current_offset + line.len() + 1; // +1 for newline
			if offset < line_end {
				line_idx = idx;
				break;
			}
			current_offset = line_end;
		}

		if line_idx >= lines.len() {
			return None;
		}

		let line_content = lines[line_idx];
		let line_start_offset = current_offset;
		let offset_in_line = offset.saturating_sub(line_start_offset);

		// Find which column the cursor is in
		let mut col_idx = 0;
		let mut cell_start = 0;
		let mut cell_end = 0;
		let mut in_quotes = false;

		for (i, ch) in line_content.char_indices() {
			match ch {
				'"' => in_quotes = !in_quotes,
				',' if !in_quotes => {
					if i >= offset_in_line {
						cell_end = i;
						break;
					}
					col_idx += 1;
					cell_start = i + 1;
				}
				_ => {}
			}
		}

		// If we didn't break, we're in the last cell
		if cell_end <= cell_start {
			cell_end = line_content.len();
		}

		let cell_value = &line_content[cell_start..cell_end];
		let cell_range = (line_start_offset + cell_start)..(line_start_offset + cell_end);
		let offset_in_cell = offset_in_line.saturating_sub(cell_start);

		// Determine column type from header
		let column_type = headers.get(col_idx).and_then(|h| match *h {
			"id" => Some(CsvColumn::Id),
			"name" => Some(CsvColumn::Name),
			"model_id:id" | "model_id/id" => Some(CsvColumn::ModelId),
			"group_id:id" | "group_id/id" => Some(CsvColumn::GroupId),
			"perm_read" => Some(CsvColumn::PermRead),
			"perm_write" => Some(CsvColumn::PermWrite),
			"perm_create" => Some(CsvColumn::PermCreate),
			"perm_unlink" => Some(CsvColumn::PermUnlink),
			_ => None,
		});

		Some(CsvCursorContext {
			line: line_idx,
			column_idx: col_idx,
			column_type,
			cell_value,
			cell_range,
			offset_in_cell,
			headers,
		})
	}

	/// Complete model_id:id column (e.g., "model_sale_order")
	async fn complete_csv_model_id(
		&self,
		context: &CsvCursorContext<'_>,
		_contents: &str,
		rope: RopeSlice<'_>,
	) -> anyhow::Result<Option<CompletionResponse>> {
		let needle = &context.cell_value[..context.offset_in_cell];
		let range = rope_conv(
			ByteOffset(context.cell_range.start)..ByteOffset(context.cell_range.end),
			rope,
		);

		let mut items = MaxVec::new(100);

		// Iterate over all known models and create model_id references
		for model in self.index.models.iter() {
			let model_name = _R(*model.key());
			// Convert model name to model_id format: "sale.order" -> "model_sale_order"
			let model_ref = format!("model_{}", model_name.replace('.', "_"));

			if !needle.is_empty() && !model_ref.starts_with(needle) {
				continue;
			}

			items.push_checked(CompletionItem {
				label: model_ref.clone(),
				kind: Some(CompletionItemKind::CLASS),
				label_details: Some(CompletionItemLabelDetails {
					detail: Some(format!(" → {}", model_name)),
					description: None,
				}),
				text_edit: Some(CompletionTextEdit::Edit(TextEdit {
					range,
					new_text: model_ref,
				})),
				..Default::default()
			});
		}

		Ok(Some(CompletionResponse::List(CompletionList {
			is_incomplete: !items.has_space(),
			items: items.into_inner(),
		})))
	}

	/// Complete group_id:id column (e.g., "base.group_user")
	async fn complete_csv_group_id(
		&self,
		context: &CsvCursorContext<'_>,
		_contents: &str,
		rope: RopeSlice<'_>,
		current_module: Option<crate::index::ModuleName>,
	) -> anyhow::Result<Option<CompletionResponse>> {
		let needle = &context.cell_value[..context.offset_in_cell];
		let range = rope_conv(
			ByteOffset(context.cell_range.start)..ByteOffset(context.cell_range.end),
			rope,
		);

		let mut items = MaxVec::new(100);

		// Complete with XML IDs filtered to res.groups model
		let groups_model = _G("res.groups");
		if let Some(groups_model) = groups_model {
			for record_ref in self.index.records.by_model(&groups_model.into()) {
				let record = record_ref.value();
				let qualified_id = record.qualified_id();

				// Filter by needle
				if !needle.is_empty() && !qualified_id.starts_with(needle) && !record.id.starts_with(needle) {
					continue;
				}

				// Prefer unqualified if same module
				let insert_text = if current_module == Some(record.module) {
					record.id.to_string()
				} else {
					qualified_id.clone()
				};

				items.push_checked(CompletionItem {
					label: qualified_id.clone(),
					kind: Some(CompletionItemKind::REFERENCE),
					label_details: Some(CompletionItemLabelDetails {
						detail: Some(" res.groups".to_string()),
						description: None,
					}),
					text_edit: Some(CompletionTextEdit::Edit(TextEdit {
						range,
						new_text: insert_text,
					})),
					..Default::default()
				});
			}
		}

		Ok(Some(CompletionResponse::List(CompletionList {
			is_incomplete: !items.has_space(),
			items: items.into_inner(),
		})))
	}

	pub fn csv_hover(&self, params: HoverParams, rope: RopeSlice<'_>) -> anyhow::Result<Option<Hover>> {
		let ByteOffset(offset) = rope_conv(params.text_document_position_params.position, rope);
		let contents = Cow::from(rope);

		let Some(context) = self.parse_csv_cursor_context(offset, &contents) else {
			return Ok(None);
		};

		if context.line == 0 {
			return Ok(None);
		}

		let range = rope_conv(
			ByteOffset(context.cell_range.start)..ByteOffset(context.cell_range.end),
			rope,
		);

		let path = params.text_document_position_params.text_document.uri.to_file_path();
		let current_module = path.as_ref().and_then(|p| self.index.find_module_of(p));

		match context.column_type {
			Some(CsvColumn::ModelId) => self.hover_csv_model_id(context.cell_value, range),
			Some(CsvColumn::GroupId) => self.hover_csv_group_id(context.cell_value, range, current_module),
			_ => Ok(None),
		}
	}

	fn hover_csv_model_id(&self, cell_value: &str, range: Range) -> anyhow::Result<Option<Hover>> {
		let Some(model_name) =
			crate::index::access::resolve_model_from_csv_ref(cell_value, &self.index.models)
		else {
			return Ok(None);
		};

		let model_key = _G(&model_name);
		let Some(model_key) = model_key else {
			return Ok(Some(Hover {
				contents: HoverContents::Markup(MarkupContent {
					kind: MarkupKind::Markdown,
					value: format!("**Unknown model**: `{}`", model_name),
				}),
				range: Some(range),
			}));
		};

		if self.index.models.get(&model_key).is_none() {
			return Ok(Some(Hover {
				contents: HoverContents::Markup(MarkupContent {
					kind: MarkupKind::Markdown,
					value: format!("**Model not found**: `{}`", model_name),
				}),
				range: Some(range),
			}));
		}

		let mut value = format!("**Model**: `{}`\n\n", model_name);

		let access_rules = self.index.access_rules.by_model(&model_key.into());
		if !access_rules.is_empty() {
			value.push_str("**Existing access rules:**\n");
			for rule_id in access_rules.iter().take(5) {
				if let Some(rule) = self.index.access_rules.get(rule_id) {
					let group = rule.group_id.map(|g| _R(g).to_string()).unwrap_or_else(|| "Public".to_string());
					value.push_str(&format!("- `{}` ({}) [{}]\n", rule.id, group, rule.permission_string()));
				}
			}
			if access_rules.len() > 5 {
				value.push_str(&format!("- ... and {} more\n", access_rules.len() - 5));
			}
		}

		Ok(Some(Hover {
			contents: HoverContents::Markup(MarkupContent {
				kind: MarkupKind::Markdown,
				value,
			}),
			range: Some(range),
		}))
	}

	fn hover_csv_group_id(
		&self,
		cell_value: &str,
		range: Range,
		current_module: Option<crate::index::ModuleName>,
	) -> anyhow::Result<Option<Hover>> {
		if cell_value.is_empty() {
			return Ok(Some(Hover {
				contents: HoverContents::Markup(MarkupContent {
					kind: MarkupKind::Markdown,
					value: "**Public access** (no group restriction)".to_string(),
				}),
				range: Some(range),
			}));
		}

		let qualified = if cell_value.contains('.') {
			cell_value.to_string()
		} else if let Some(module) = current_module {
			format!("{}.{}", _R(module), cell_value)
		} else {
			cell_value.to_string()
		};

		let group_id = _G(&qualified);
		let Some(group_id) = group_id else {
			return Ok(Some(Hover {
				contents: HoverContents::Markup(MarkupContent {
					kind: MarkupKind::Markdown,
					value: format!("**Unknown group**: `{}`", qualified),
				}),
				range: Some(range),
			}));
		};

		let Some(record) = self.index.records.get(&group_id) else {
			return Ok(Some(Hover {
				contents: HoverContents::Markup(MarkupContent {
					kind: MarkupKind::Markdown,
					value: format!("**Group not found**: `{}`", qualified),
				}),
				range: Some(range),
			}));
		};

		Ok(Some(Hover {
			contents: HoverContents::Markup(MarkupContent {
				kind: MarkupKind::Markdown,
				value: format!(
					"**Group**: `{}`\n\n**Module**: `{}`",
					record.qualified_id(),
					_R(record.module)
				),
			}),
			range: Some(range),
		}))
	}

	/// Provide go-to-definition for CSV files
	pub fn csv_goto_definition(
		&self,
		params: GotoDefinitionParams,
		rope: RopeSlice<'_>,
	) -> anyhow::Result<Option<GotoDefinitionResponse>> {
		let ByteOffset(offset) = rope_conv(params.text_document_position_params.position, rope);
		let contents = Cow::from(rope);

		let Some(context) = self.parse_csv_cursor_context(offset, &contents) else {
			return Ok(None);
		};

		if context.line == 0 {
			return Ok(None);
		}

		let path = params.text_document_position_params.text_document.uri.to_file_path();
		let current_module = path.as_ref().and_then(|p| self.index.find_module_of(p));

		match context.column_type {
			Some(CsvColumn::ModelId) => self.goto_csv_model_id(context.cell_value),
			Some(CsvColumn::GroupId) => self.goto_csv_group_id(context.cell_value, current_module),
			_ => Ok(None),
		}
	}

	fn goto_csv_model_id(&self, cell_value: &str) -> anyhow::Result<Option<GotoDefinitionResponse>> {
		let Some(model_name) =
			crate::index::access::resolve_model_from_csv_ref(cell_value, &self.index.models)
		else {
			return Ok(None);
		};

		let Some(model_key) = _G(&model_name) else {
			return Ok(None);
		};

		let Some(model) = self.index.models.get(&model_key) else {
			return Ok(None);
		};

		let loc = model.base.as_ref().or_else(|| model.descendants.first());
		if let Some(loc) = loc {
			if let Some(uri) = Uri::from_file_path(loc.0.path.to_path()) {
				return Ok(Some(GotoDefinitionResponse::Scalar(Location {
					uri,
					range: loc.0.range,
				})));
			}
		}

		Ok(None)
	}

	fn goto_csv_group_id(
		&self,
		cell_value: &str,
		current_module: Option<crate::index::ModuleName>,
	) -> anyhow::Result<Option<GotoDefinitionResponse>> {
		if cell_value.is_empty() {
			return Ok(None);
		}

		let qualified = if cell_value.contains('.') {
			cell_value.to_string()
		} else if let Some(module) = current_module {
			format!("{}.{}", _R(module), cell_value)
		} else {
			cell_value.to_string()
		};

		let Some(group_id) = _G(&qualified) else {
			return Ok(None);
		};

		let Some(record) = self.index.records.get(&group_id) else {
			return Ok(None);
		};

		if let Some(uri) = Uri::from_file_path(record.location.path.to_path()) {
			return Ok(Some(GotoDefinitionResponse::Scalar(Location {
				uri,
				range: record.location.range,
			})));
		}

		Ok(None)
	}

	/// Provide diagnostics for CSV files
	pub fn csv_diagnostics(
		&self,
		contents: &str,
		current_module: Option<crate::index::ModuleName>,
	) -> Vec<Diagnostic> {
		let mut diagnostics = vec![];
		let lines: Vec<&str> = contents.lines().collect();

		if lines.is_empty() {
			return diagnostics;
		}

		let headers: Vec<&str> = lines[0].split(',').map(|s| s.trim()).collect();

		// Find column indices
		let model_idx = headers.iter().position(|&h| h == "model_id:id" || h == "model_id/id");
		let group_idx = headers.iter().position(|&h| h == "group_id:id" || h == "group_id/id");

		let rope = Rope::from_str(contents);
		let rope_slice = rope.slice(..);

		let mut line_offset = 0;
		for (line_num, line) in lines.iter().enumerate() {
			if line_num == 0 {
				// Skip header
				line_offset += line.len() + 1;
				continue;
			}

			let fields = parse_csv_fields(line);

			if let Some(idx) = model_idx {
				if let Some(field) = fields.get(idx) {
					let model_ref = field.value.trim();
					if !model_ref.is_empty() {
						if let Some(model_name) =
							crate::index::access::resolve_model_from_csv_ref(model_ref, &self.index.models)
						{
							if !_G(&model_name)
								.map(|k| self.index.models.contains_key(&k))
								.unwrap_or(false)
							{
								let start = rope_conv(ByteOffset(line_offset + field.start), rope_slice);
								let end = rope_conv(ByteOffset(line_offset + field.end), rope_slice);
								diagnostics.push(Diagnostic {
									range: Range { start, end },
									severity: Some(DiagnosticSeverity::WARNING),
									source: Some("odoo-lsp".to_string()),
									message: format!("Model '{}' not found", model_name),
									..Default::default()
								});
							}
						} else {
							let start = rope_conv(ByteOffset(line_offset + field.start), rope_slice);
							let end = rope_conv(ByteOffset(line_offset + field.end), rope_slice);
							diagnostics.push(Diagnostic {
								range: Range { start, end },
								severity: Some(DiagnosticSeverity::ERROR),
								source: Some("odoo-lsp".to_string()),
								message: format!(
									"Invalid model reference '{}'. Expected format: model_<model_name>",
									model_ref
								),
								..Default::default()
							});
						}
					}
				}
			}

			// Check group_id
			if let Some(idx) = group_idx {
				if let Some(field) = fields.get(idx) {
					let group_ref = field.value.trim();
					if !group_ref.is_empty() {
						// Qualify the group reference if needed
						let qualified = if group_ref.contains('.') {
							group_ref.to_string()
						} else if let Some(module) = current_module {
							format!("{}.{}", _R(module), group_ref)
						} else {
							group_ref.to_string()
						};

						if _G(&qualified).is_none() {
							let start = rope_conv(ByteOffset(line_offset + field.start), rope_slice);
							let end = rope_conv(ByteOffset(line_offset + field.end), rope_slice);
							diagnostics.push(Diagnostic {
								range: Range { start, end },
								severity: Some(DiagnosticSeverity::WARNING),
								source: Some("odoo-lsp".to_string()),
								message: format!("Group '{}' not found", qualified),
								..Default::default()
							});
						}
					}
				}
			}

			line_offset += line.len() + 1;
		}

		diagnostics
	}
}

/// A parsed CSV field with its position
struct CsvField<'a> {
	value: &'a str,
	start: usize,
	end: usize,
}

/// Parse CSV fields with their positions
fn parse_csv_fields(line: &str) -> Vec<CsvField<'_>> {
	let mut fields = vec![];
	let mut start = 0;
	let mut in_quotes = false;

	for (i, ch) in line.char_indices() {
		match ch {
			'"' => in_quotes = !in_quotes,
			',' if !in_quotes => {
				fields.push(CsvField {
					value: &line[start..i],
					start,
					end: i,
				});
				start = i + 1;
			}
			_ => {}
		}
	}

	// Last field
	fields.push(CsvField {
		value: &line[start..],
		start,
		end: line.len(),
	});

	fields
}
