use std::future::Future;

use futures::future::BoxFuture;
use tree_sitter::{Node, QueryMatch};

use std::borrow::Cow;
use std::sync::atomic::Ordering::Relaxed;

use tower_lsp_server::ls_types::*;
use tracing::debug;
use tree_sitter::Tree;

use crate::prelude::*;

use crate::analyze::{DictKey, Type, type_cache};
use crate::backend::Backend;
use crate::domain::{self, DEFAULT_MAX_DOMAIN_DEPTH, FieldTypeCategory};
use crate::index::{_G, _I, _R, symbol::Symbol};
use crate::model::{FieldKind, ModelEntry, ModelName, PropertyKind};
use crate::some;
use crate::utils::MaxVec;
use crate::utils::*;
use crate::xml::determine_csv_xmlid_subgroup;

use super::{Mapped, PyCompletions, ThisModel, extract_string_needle_at_offset, top_level_stmt};

/// Context for domain expression completions.
/// 
/// Represents the position within a domain tuple where the cursor is located.
#[derive(Debug, Clone)]
pub enum DomainCompletionContext<'a> {
    /// Cursor is in the field name position (first element)
    /// Contains: (model_name, field_node, current_prefix)
    Field {
        model: String,
        node: Node<'a>,
    },
    /// Cursor is in the operator position (second element)
    Operator {
        field_type: Option<FieldTypeCategory>,
        node: Node<'a>,
    },
    /// Cursor is in the value position (third element)
    Value {
        field_type: Option<FieldTypeCategory>,
        operator: String,
        node: Node<'a>,
        /// Selection choices if the field is a Selection field
        selection_choices: Option<Vec<(String, String)>>,
    },
}

struct EarlyReturn<'a, T>(Option<Box<dyn FnOnce() -> BoxFuture<'a, T> + 'a + Send>>);

impl<'a, T> Default for EarlyReturn<'a, T> {
	fn default() -> Self {
		Self(None)
	}
}

impl<'a, T> EarlyReturn<'a, T> {
	fn lift<F, Fut>(&mut self, closure: F)
	where
		F: FnOnce() -> Fut + 'a + Send,
		Fut: Future<Output = T> + 'a + Send,
	{
		self.0 = Some(Box::new(|| Box::pin(closure())));
	}

	fn is_none(&self) -> bool {
		self.0.is_none()
	}

	async fn call(self) -> Option<T> {
		match self.0 {
			Some(closure) => Some(closure().await),
			None => None,
		}
	}
}

/// Python extensions for item completions.
impl Backend {
	pub(crate) async fn python_completions(
		&self,
		params: CompletionParams,
		ast: Tree,
		rope: RopeSlice<'_>,
	) -> anyhow::Result<Option<CompletionResponse>> {
		let ByteOffset(offset) = rope_conv(params.text_document_position.position, rope);
		let path = some!(params.text_document_position.text_document.uri.to_file_path());
		let Some(current_module) = self.index.find_module_of(&path) else {
			debug!("no current module");
			return Ok(None);
		};
		let mut cursor = tree_sitter::QueryCursor::new();
		let contents = Cow::from(rope);
		let query = PyCompletions::query();
		let completions_limit = self
			.workspaces
			.find_workspace_of(&path, |_, ws| ws.completions.limit)
			.unwrap_or_else(|| self.project_config.completions_limit.load(Relaxed));
		let mut this_model = ThisModel::default();
		// FIXME: This hack is necessary to drop !Send locals before await points.
		let mut early_return = EarlyReturn::<anyhow::Result<_>>::default();
		{
			let root = some!(top_level_stmt(ast.root_node(), offset));
			let mut matches = cursor.matches(query, root, contents.as_bytes());
			'match_: while let Some(match_) = matches.next() {
				let mut model_filter = None;
				let mut field_descriptors = vec![];
				let mut field_descriptor_in_offset = None;
				let mut field_model = None;

				for capture in match_.captures {
					let range = capture.node.byte_range();
					match PyCompletions::from(capture.index) {
						Some(PyCompletions::Request) => model_filter = Some("ir.ui.view"),
						Some(PyCompletions::HasGroups) => model_filter = Some("res.groups"),
						Some(PyCompletions::ForXmlId) => {
							let model = || {
								let model = capture.node.prev_named_sibling()?;
								let model = self.index.model_of_range(
									root,
									model.byte_range().map_unit(ByteOffset),
									&contents,
								)?;
								Some(_R(model))
							};
							model_filter = model()
						}
						Some(PyCompletions::XmlId) if range.contains_end(offset) => {
							let mut range = range.shrink(1);
							let mut needle = &contents[range.clone()];
							if match_
								.nodes_for_capture_index(PyCompletions::HasGroups as _)
								.next()
								.is_some()
							{
								let mut ref_ = None;
								determine_csv_xmlid_subgroup(&mut ref_, (needle, range), offset);
								(needle, range) = some!(ref_);
							}
							let needle = needle[..offset - range.start].to_string();
							early_return.lift(move || async move {
								let mut items = MaxVec::new(completions_limit);
								self.index.complete_xml_id(
									&needle,
									range.map_unit(ByteOffset),
									rope,
									model_filter.map(|m| vec![m.into()]).as_deref(),
									current_module,
									None,
									&mut items,
								)?;
								Ok(Some(CompletionResponse::List(CompletionList {
									is_incomplete: !items.has_space(),
									items: items.into_inner(),
								})))
							});
							break 'match_;
						}
						Some(PyCompletions::Model) => {
							if range.contains_end(offset) {
								let (needle, byte_range) = extract_string_needle_at_offset(rope, range, offset)?;
								let range = rope_conv(byte_range, rope);
								early_return.lift(move || async move {
									let mut items = MaxVec::new(completions_limit);
									self.index.complete_model(&needle, range, &mut items)?;
									Ok(Some(CompletionResponse::List(CompletionList {
										is_incomplete: !items.has_space(),
										items: items.into_inner(),
									})))
								});
								break 'match_;
							}

							// capture a model for later use
							if match_
								.nodes_for_capture_index(PyCompletions::Prop as _)
								.next()
								.is_none()
							{
								continue;
							}

							if range.end < offset
								&& let Some(field) =
									match_.nodes_for_capture_index(PyCompletions::FieldType as _).next()
							{
								if field_model.is_none()
									&& matches!(&contents[field.byte_range()], "Many2one" | "One2many" | "Many2many")
								{
									field_model = Some(&contents[capture.node.byte_range().shrink(1)]);
								}
							} else {
								this_model.tag_model(capture.node, match_, root.byte_range(), &contents);
							}
						}
						Some(PyCompletions::Mapped) => {
							if range.contains_end(offset) {
								// Check if this is an ERROR node with broken syntax
								tracing::debug!(
									"Mapped capture node kind: {}, range: {:?}, offset: {}",
									capture.node.kind(),
									range,
									offset
								);
								if capture.node.kind() == "ERROR" {
									// This might be a string without a colon in a dictionary
									let error_text = &contents[capture.node.byte_range()];
									if error_text.starts_with("'") || error_text.starts_with("\"") {
										// Extract the partial text
										let quote_char = error_text.as_bytes()[0];
										let end_quote = error_text.bytes().rposition(|b| b == quote_char);
										let needle = if let Some(end) = end_quote {
											&error_text[1..end]
										} else if offset > capture.node.start_byte() + 1 {
											&error_text[1..offset - capture.node.start_byte()]
										} else {
											""
										};

										// Try to determine the model from context
										// First check if we have a MappedTarget in the match
										let mut field_model: Option<Symbol<ModelEntry>> = None;
										// Try to get the model from MappedTarget
										if let Some(target_node) =
											match_.nodes_for_capture_index(PyCompletions::MappedTarget as _).next()
											&& let Some(model_) = self.index.model_of_range(
												root,
												target_node.byte_range().map_unit(ByteOffset),
												&contents,
											) {
											field_model = Some(model_);
										}

										// If we didn't find it from MappedTarget, look for the commandlist pattern in the parent nodes
										if field_model.is_none() {
											let mut current = capture.node;
											while let Some(parent) = current.parent() {
												// Check if we're in a list that's part of a mapped/commandlist
												if parent.kind() == "list" {
													// Try to find the full expression this list is part of
													if let Some(expr_parent) = parent.parent()
														&& (expr_parent.kind() == "call"
															|| expr_parent.kind() == "attribute")
													{
														break;
													}
												}

												if parent.kind() == "dictionary" {
													// Found the dictionary, now look for the field assignment
													if let Some(list_parent) = parent.parent()
														&& list_parent.kind() == "list" && let Some(pair_parent) =
														list_parent.parent() && pair_parent.kind() == "pair"
														&& let Some(key) = pair_parent.child_by_field_name("key")
														&& key.kind() == "string"
													{
														let field_name = &contents[key.byte_range().shrink(1)];

														if let Some(model_str) = this_model.inner {
															let model_key = ModelName::from(_I(model_str));

															if let Some(props) =
																self.index.models.populate_properties(model_key, &[])
																&& let Some(fields) = &props.fields && let Some(
																field_key,
															) = _G(field_name) && let Some(field_info) =
																fields.get(&field_key)
															{
																// Check if this field has a relational type
																if let FieldKind::Relational(relation) =
																	&field_info.kind
																{
																	field_model = Some((*relation).into());
																}
															}
														}
													}
													break;
												}
												current = parent;
											}
										}

										if let Some(model) = field_model {
											let range = if let Some(end) = end_quote {
												ByteRange {
													start: ByteOffset(capture.node.start_byte() + 1),
													end: ByteOffset(capture.node.start_byte() + end),
												}
											} else {
												ByteRange {
													start: ByteOffset(capture.node.start_byte() + 1),
													end: ByteOffset(offset),
												}
											};

											let mut items = MaxVec::new(completions_limit);
											self.index.complete_property_name(
												needle,
												range,
										_R(model).into(),
										rope,
										Some(PropertyKind::Field),
										None,
										true,
										false,
										&mut items,
											)?;
											return Ok(Some(CompletionResponse::List(CompletionList {
												is_incomplete: !items.has_space(),
												items: items.into_inner(),
											})));
										}
									}
								}

								// Normal case - not an ERROR node
								// Check if we're inside a subdomain and need to resolve the comodel
								let resolved_model = self
									.resolve_subdomain_model_context(
										capture.node,
										offset,
										this_model.inner,
										&contents,
									)
									.or_else(|| this_model.inner.map(|s| s.to_string()));

								return self.python_completions_for_prop(
									root,
									match_,
									offset,
									capture.node,
									resolved_model.as_deref(),
									&contents,
									completions_limit,
									Some(PropertyKind::Field),
									None,
									rope,
								);
							} else if let Some(cmdlist) = python_next_named_sibling(capture.node)
								&& Backend::is_commandlist(cmdlist, offset)
								&& let Some((needle, range, model)) = self.gather_commandlist(
									cmdlist,
									root,
									match_,
									offset,
									range,
									this_model.inner,
									&contents,
									true,
								) {
								let mut items = MaxVec::new(completions_limit);
								self.index.complete_property_name(
									needle,
									range,
									ImStr::from(_R(model)),
									rope,
									Some(PropertyKind::Field),
									None,
									true,
									false,
									&mut items,
								)?;
								return Ok(Some(CompletionResponse::List(CompletionList {
									is_incomplete: !items.has_space(),
									items: items.into_inner(),
								})));
								// If gather_commandlist returns None, continue to next match
							}
						}
						Some(PyCompletions::FieldDescriptor) => {
							let Some(desc_value) = python_next_named_sibling(capture.node) else {
								continue;
							};

							let descriptor = &contents[capture.node.byte_range()];
							if desc_value.byte_range().contains_end(offset) {
							match descriptor {
								"compute" | "search" | "inverse" | "related" | "inverse_name" => {
									let prop_kind = if matches!(descriptor, "related" | "inverse_name") {
										PropertyKind::Field
									} else {
										PropertyKind::Method
									};
									let (mapped_model, field_model_filter) = if descriptor == "inverse_name" {
										(
											super::extract_comodel_name(match_.captures, &contents)
												.map(|comodel_name| &contents[comodel_name.byte_range().shrink(1)]),
											this_model.inner,
										)
									} else {
										(this_model.inner, None)
									};
									return self.python_completions_for_prop(
										root,
										match_,
										offset,
										desc_value,
										mapped_model,
										&contents,
										completions_limit,
										Some(prop_kind),
										field_model_filter,
										rope,
										);
									}
									"comodel_name" => {
										// same as model
										let range = desc_value.byte_range();
										let (needle, byte_range) =
											extract_string_needle_at_offset(rope, range, offset)?;
										let range = rope_conv(byte_range, rope);
										early_return.lift(move || async move {
											let mut items = MaxVec::new(completions_limit);
											self.index.complete_model(&needle, range, &mut items)?;
											Ok(Some(CompletionResponse::List(CompletionList {
												is_incomplete: !items.has_space(),
												items: items.into_inner(),
											})))
										});
										break 'match_;
									}
								"groups" => {
									// complete res.groups records
									let range = desc_value.byte_range().shrink(1);
									let value = Cow::from(ok!(rope.try_slice(range.clone())));
									let mut ref_ = None;
									determine_csv_xmlid_subgroup(&mut ref_, (&value, range), offset);
									let (needle, range) = some!(ref_);
									let needle = needle[..offset - range.start - 1].to_string();
									early_return.lift(move || async move {
										let mut items = MaxVec::new(completions_limit);
										self.index.complete_xml_id(
											&needle,
											range.map_unit(ByteOffset),
											rope,
											Some(&["res.groups".into()]),
											current_module,
											None,
											&mut items,
										)?;
										Ok(Some(CompletionResponse::List(CompletionList {
											is_incomplete: !items.has_space(),
											items: items.into_inner(),
										})))
									});
									break 'match_;
								}
								"definition" => {
									// Complete definition path for Properties fields
									// Format: "many2one_field.properties_definition_field"
									let range = desc_value.byte_range().shrink(1);
									if range.is_empty() {
										// Empty string - complete Many2one fields
										let model = this_model.inner.map(String::from);
										early_return.lift(move || async move {
											let mut items = MaxVec::new(completions_limit);
											self.complete_definition_m2o_fields(
												model.as_deref(),
												"",
												rope,

												&mut items,
											)?;
											Ok(Some(CompletionResponse::List(CompletionList {
												is_incomplete: !items.has_space(),
												items: items.into_inner(),
											})))
										});
										break 'match_;
									}

									let value = ok!(rope.try_slice(range.clone()));
									let cursor_in_value = offset - range.start; // range is already shrunk past the quote
									let value_str = Cow::from(value);

									if let Some(dot_pos) = value_str.find('.') {
										if cursor_in_value > dot_pos {
											// After the dot - complete PropertiesDefinition fields
											let m2o_field = value_str[..dot_pos].to_string();
											let needle = value_str[dot_pos + 1..cursor_in_value.min(value_str.len())].to_string();
											let model = this_model.inner.map(String::from);
											early_return.lift(move || async move {
												let mut items = MaxVec::new(completions_limit);
												self.complete_definition_propdef_fields(
													model.as_deref(),
													&m2o_field,
													&needle,
													rope,
													&mut items,
												)?;
												Ok(Some(CompletionResponse::List(CompletionList {
													is_incomplete: !items.has_space(),
													items: items.into_inner(),
												})))
											});
											break 'match_;
										}
									}

									// Before or at the dot - complete Many2one fields
									let needle = if let Some(dot_pos) = value_str.find('.') {
										value_str[..cursor_in_value.min(dot_pos)].to_string()
									} else {
										value_str[..cursor_in_value.min(value_str.len())].to_string()
									};
									let model = this_model.inner.map(String::from);
									early_return.lift(move || async move {
										let mut items = MaxVec::new(completions_limit);
										self.complete_definition_m2o_fields(
											model.as_deref(),
											&needle,
											rope,
											&mut items,
										)?;
										Ok(Some(CompletionResponse::List(CompletionList {
											is_incomplete: !items.has_space(),
											items: items.into_inner(),
										})))
									});
									break 'match_;
								}
								_ => {}
								}
							}

							if matches!(descriptor, "comodel_name" | "domain" | "groups") {
								field_descriptors.push((descriptor, desc_value));
							}
							if desc_value.byte_range().contains_end(offset) {
								field_descriptor_in_offset = Some((descriptor, desc_value));
							}
						}
						Some(PyCompletions::Depends)
						| Some(PyCompletions::MappedTarget)
						| Some(PyCompletions::XmlId)
						| Some(PyCompletions::Prop)
						| Some(PyCompletions::Scope)
						| Some(PyCompletions::ReadFn)
						| Some(PyCompletions::FieldType)
						| None => {}
					}
				}
				if let Some(("domain", value)) = field_descriptor_in_offset {
					let mut domain_node = value;
					if domain_node.kind() == "lambda" {
						let Some(body) = domain_node.child_by_field_name("body") else {
							continue;
						};
						domain_node = body;
					}
					if domain_node.kind() != "list" {
						continue;
					}
					let comodel_name = field_descriptors
						.iter()
						.find_map(|&(desc, node)| {
							(desc == "comodel_name").then(|| &contents[node.byte_range().shrink(1)])
						})
						.or(field_model);

					let Some(comodel) = comodel_name else {
						continue;
					};

					// Find the full domain context at the cursor position
					let Some(context) = self.find_domain_completion_context_full(
						domain_node,
						offset,
						comodel,
						&contents,
						0,
						DEFAULT_MAX_DOMAIN_DEPTH,
					) else {
						continue;
					};

					match context {
						DomainCompletionContext::Field { model, node } => {
							// Field completion - use existing logic
							return self.python_completions_for_prop(
								root,
								match_,
								offset,
								node,
								Some(model.as_str()),
								&contents,
								completions_limit,
								Some(PropertyKind::Field),
								None,
								rope,
							);
						}
						DomainCompletionContext::Operator { field_type, node, .. } => {
							// Operator completion
							let range = node.byte_range();
							let inner_range = range.clone().shrink(1);
							let full_content = if range.len() >= 2 {
								contents[inner_range.clone()].to_string()
							} else {
								String::new()
							};
							// Extract prefix up to cursor
							let inner_start = range.start + 1; // After opening quote
							let prefix_len = offset.saturating_sub(inner_start);
							let prefix = full_content[..prefix_len.min(full_content.len())].to_string();
							let lsp_range = rope_conv(inner_range.map_unit(ByteOffset), rope);
							
							early_return.lift(move || async move {
								let items: Vec<CompletionItem> = domain::get_operator_completions(field_type)
									.into_iter()
									.filter(|item| prefix.is_empty() || item.label.to_lowercase().starts_with(&prefix.to_lowercase()))
									.map(|mut item| {
										// Set text edit to replace the entire operator string
										item.text_edit = Some(CompletionTextEdit::Edit(TextEdit {
											range: lsp_range,
											new_text: item.label.clone(),
										}));
										item
									})
									.collect();
								
								Ok(Some(CompletionResponse::List(CompletionList {
									is_incomplete: false,
									items,
								})))
							});
							break 'match_;
						}
						DomainCompletionContext::Value { field_type, operator, node, selection_choices, .. } => {
							// Value completion
							// For selection fields with '=' or 'in' operators, offer selection choices
							if let Some(choices) = selection_choices {
								if matches!(operator.as_str(), "=" | "!=" | "in" | "not in" | "=?") {
									let range = node.byte_range();
									// Check if we're inside a string
									if node.kind() == "string" && range.len() >= 2 {
										let inner_range = range.clone().shrink(1);
										let inner_start = range.start + 1;
										let prefix_len = offset.saturating_sub(inner_start);
										let inner_content = contents[inner_range.clone()].to_string();
										let prefix = inner_content[..prefix_len.min(inner_content.len())].to_string();
										let lsp_range = rope_conv(inner_range.map_unit(ByteOffset), rope);
										
										early_return.lift(move || async move {
											let items: Vec<CompletionItem> = choices
												.iter()
												.filter(|(value, _)| prefix.is_empty() || value.starts_with(&prefix))
												.map(|(value, label)| {
													CompletionItem {
														label: value.clone(),
														kind: Some(CompletionItemKind::ENUM_MEMBER),
														detail: Some(label.clone()),
														label_details: Some(CompletionItemLabelDetails {
															detail: None,
															description: Some("Selection value".to_string()),
														}),
														text_edit: Some(CompletionTextEdit::Edit(TextEdit {
															range: lsp_range,
															new_text: value.clone(),
														})),
														..Default::default()
													}
												})
												.collect();
											
											Ok(Some(CompletionResponse::List(CompletionList {
												is_incomplete: false,
												items,
											})))
										});
										break 'match_;
									}
								}
							}
							
							// For boolean fields, offer True/False
							if field_type == Some(FieldTypeCategory::Boolean) {
								if matches!(operator.as_str(), "=" | "!=" | "=?") && (node.kind() == "true" || node.kind() == "false" || node.kind() == "identifier") {
									early_return.lift(move || async move {
										let items = vec![
											CompletionItem {
												label: "True".to_string(),
												kind: Some(CompletionItemKind::KEYWORD),
												detail: Some("Boolean value".to_string()),
												..Default::default()
											},
											CompletionItem {
												label: "False".to_string(),
												kind: Some(CompletionItemKind::KEYWORD),
												detail: Some("Boolean value".to_string()),
												..Default::default()
											},
										];
										Ok(Some(CompletionResponse::List(CompletionList {
											is_incomplete: false,
											items,
										})))
									});
									break 'match_;
								}
							}
							
							// No specific value completion available
							continue;
						}
					}
				}
			}
			if early_return.is_none() {
				let cursor_node = root.descendant_for_byte_range(offset, offset);
				if let Some(node) = cursor_node {
					let mut current = node;
					while let Some(parent) = current.parent() {
						// (dictionary (ERROR ^cursor))
						if parent.kind() == "ERROR" {
							if let Some(grandparent) = parent.parent()
								&& grandparent.kind() == "dictionary"
							{
								// For broken syntax (string without colon), we want to show all fields
								// So we use an empty needle
								let needle = Cow::Borrowed("");

								// Try to determine the model from the context
								// Look for the field assignment this dictionary belongs to
								let mut field_model: Option<Symbol<ModelEntry>> = None;
								let mut dict_parent = grandparent;

								while let Some(parent) = dict_parent.parent() {
									if parent.kind() == "list"
										&& let Some(list_parent) = parent.parent()
										&& list_parent.kind() == "pair"
									{
										if let Some(key) = list_parent.child_by_field_name("key")
											&& key.kind() == "string" && let Some(model_str) = &this_model.inner
										{
											let field_name = &contents[key.byte_range().shrink(1)];

											let model_key = ModelName::from(_I(model_str));

											// Check if this field has a relational type
											if let Some(props) = self.index.models.populate_properties(model_key, &[])
												&& let Some(fields) = &props.fields && let Some(field_key) =
												_G(field_name) && let Some(field_info) = fields.get(&field_key)
												&& let FieldKind::Relational(relation) = field_info.kind
											{
												field_model = Some(relation.into());
											}
										}
										break;
									}
									dict_parent = parent;
								}

								if let Some(model) = field_model {
									// For broken syntax, we want to replace the whole string
									let range = if node.kind() == "string_content" {
										// Find the parent string node to get the full range
										if let Some(string_parent) = node.parent() {
											if string_parent.kind() == "string" {
												string_parent.byte_range().shrink(1).map_unit(ByteOffset)
											} else {
												node.byte_range().map_unit(ByteOffset)
											}
										} else {
											node.byte_range().map_unit(ByteOffset)
										}
									} else {
										node.byte_range().shrink(1).map_unit(ByteOffset)
									};
									let mut items = MaxVec::new(completions_limit);
									self.index.complete_property_name(
										&needle,
										range,
										_R(model).into(),
										rope,
										Some(PropertyKind::Field),
										None,
										true,
										false,
										&mut items,
									)?;
									return Ok(Some(CompletionResponse::List(CompletionList {
										is_incomplete: !items.has_space(),
										items: items.into_inner(),
									})));
								}
							}
						} else if current.kind() == "string"
							&& parent.kind() == "subscript"
							&& let Some(lhs) = parent.named_child(0)
							&& let Some((tid, _)) =
								self.index
									.type_of_range(root, dbg!(lhs).byte_range().map_unit(ByteOffset), &contents)
							&& let Type::DictBag(dict) = type_cache().resolve(tid)
						{
							let mut items = MaxVec::new(completions_limit);
							let dict = dict.iter().flat_map(|(key, _)| match key {
								DictKey::String(str) => Some(str.to_string()),
								_ => None,
							});
							let range = current.byte_range().shrink(1).map_unit(ByteOffset);
							let range = rope_conv(range, rope);
							let to_item = |label: String| {
								let new_text = label.clone();
								CompletionItem {
									label,
									kind: Some(CompletionItemKind::CONSTANT),
									text_edit: Some(CompletionTextEdit::Edit(TextEdit { range, new_text })),
									..Default::default()
								}
							};
							if offset <= current.start_byte() + 1 {
								items.extend(dict.map(to_item));
							} else {
								let needle = &contents[current.start_byte() + 1..offset];
								items.extend(dict.filter(|label| label.starts_with(needle)).map(to_item));
							}
							return Ok(Some(CompletionResponse::List(CompletionList {
								is_incomplete: !items.has_space(),
								items: items.into_inner(),
							})));
						} else if current.kind() == "string"
							&& parent.kind() == "pair"
							&& let Some(key_node) = parent.child_by_field_name("key")
							&& key_node.kind() == "string"
						{
							// We're in a dict value position: {'field': '|'}
							// Check if parent dict is in a create/write context with a model
							let field_name = &contents[key_node.byte_range().shrink(1)];
							
							// Look up the tree to find the model context
							if let Some(model_name) = this_model.inner.as_ref() {
								let model_key = ModelName::from(_I(model_name));
								if let Some(entry) = self.index.models.populate_properties(model_key, &[])
									&& let Some(fields) = &entry.fields
									&& let Some(field_key) = _G(field_name)
									&& let Some(field) = fields.get(&field_key)
									&& let Some(choices) = &field.choices
								{
									let range = current.byte_range().shrink(1).map_unit(ByteOffset);
									let lsp_range = rope_conv(range, rope);
									let needle = if offset > current.start_byte() + 1 {
										&contents[current.start_byte() + 1..offset]
									} else {
										""
									};
									
								let mut items = MaxVec::new(completions_limit);
								for (value, _label) in choices.iter() {
									if needle.is_empty() || value.starts_with(needle) {
										items.push_checked(CompletionItem {
												label: value.to_string(),
												kind: Some(CompletionItemKind::ENUM_MEMBER),
												text_edit: Some(CompletionTextEdit::Edit(TextEdit {
													range: lsp_range,
													new_text: value.to_string(),
												})),
												..Default::default()
											});
										}
									}
									
									if !items.is_empty() {
										return Ok(Some(CompletionResponse::List(CompletionList {
											is_incomplete: !items.has_space(),
											items: items.into_inner(),
										})));
									}
								}
							}
						}
						current = parent;
					}
				}

				// Fallback to regular attribute completion
				// First check if the LHS is Type::Env - if so, provide env attribute completions
				if let Some((lhs, needle, range)) = Self::attribute_node_at_offset(offset, root, &contents)
					&& let Some((tid, _scope)) =
						self.index.type_of_range(root, lhs.byte_range().map_unit(ByteOffset), &contents)
					&& matches!(type_cache().resolve(tid), Type::Env)
				{
					let mut items = MaxVec::new(completions_limit);
					self.index.complete_env_attributes(
						needle,
						range.map_unit(ByteOffset),
						rope,
						&mut items,
					)?;
					return Ok(Some(CompletionResponse::List(CompletionList {
						is_incomplete: !items.has_space(),
						items: items.into_inner(),
					})));
				}

				let (model, needle, range) = some!(self.attribute_at_offset(offset, root, &contents));
				let mut items = MaxVec::new(completions_limit);
				self.index.complete_property_name(
					needle,
					range.map_unit(ByteOffset),
					ImStr::from(model),
					rope,
					None,
					None,
					false,
					false,
					&mut items,
				)?;
				return Ok(Some(CompletionResponse::List(CompletionList {
					is_incomplete: !items.has_space(),
					items: items.into_inner(),
				})));
			}
		}
		let result = some!(early_return.call().await);
		result
	}
	/// `range` is the entire range of the mapped **string**, quotes included.
	fn python_completions_for_prop(
		&self,
		root: Node,
		match_: &QueryMatch,
		offset: usize,
		node: Node,
		this_model: Option<&str>,
		contents: &str,
		completions_limit: usize,
		prop_type: Option<PropertyKind>,
		field_model_filter: Option<&str>,
		rope: RopeSlice<'_>,
	) -> anyhow::Result<Option<CompletionResponse>> {
		let Mapped {
			mut needle,
			model,
			single_field,
			range,
		} = some!(self.gather_mapped(
			root,
			match_,
			Some(offset),
			node.byte_range(),
			this_model,
			contents,
			true,
			matches!(prop_type, Some(PropertyKind::Method)).then_some(true)
		));

		// range:  foo.bar.baz
		// needle: foo.ba
		let mut range = range;
		let mut items = MaxVec::new(completions_limit);
		let mut model = some!(_G(model));

		if !single_field {
			some!(
				(self.index.models)
					.resolve_mapped(&mut model, &mut needle, Some(&mut range))
					.ok()
			);
		}
		let model_name = _R(model);
		self.index.complete_property_name(
			needle,
			range,
			ImStr::from(model_name),
			rope,
			prop_type,
			field_model_filter,
			node.kind() == "string",
			false,
			&mut items,
		)?;
		Ok(Some(CompletionResponse::List(CompletionList {
			is_incomplete: !items.has_space(),
			items: items.into_inner(),
		})))
	}

	/// Resolve the model context for a field in a method call domain, handling subdomains.
	///
	/// This function checks if the captured field string is inside a subdomain (value of
	/// `any`/`not any`) and if so, resolves the comodel chain to get the correct model
	/// for field completions.
	///
	/// Returns `Some(model_name)` if a subdomain context was found, `None` otherwise.
	fn resolve_subdomain_model_context(
		&self,
		field_node: Node,
		_offset: usize,
		base_model: Option<&str>,
		contents: &str,
	) -> Option<String> {
		let base_model = base_model?;

		// Navigate up to find if we're inside a subdomain
		// Structure: list > tuple > (field_string | operator | value_list)
		// We want to find if we're in a value_list that's part of an any/not any tuple
		let tuple_node = field_node.parent().filter(|p| p.kind() == "tuple")?;
		let domain_list = tuple_node.parent().filter(|p| p.kind() == "list")?;

		// Check if this domain_list is the value of an any/not any operator
		let parent_tuple = domain_list.parent().filter(|p| p.kind() == "tuple")?;

		// Get the elements of the parent tuple
		let mut cursor = parent_tuple.walk();
		let mut children = parent_tuple.named_children(&mut cursor);
		let parent_field_node = children.next()?;
		let parent_operator_node = children.next()?;
		let parent_value_node = children.next()?;

		// Check if the value_node is our domain_list
		if parent_value_node.id() != domain_list.id() {
			return None;
		}

		// Check if the operator is any/not any
		if parent_operator_node.kind() != "string" {
			return None;
		}

		let operator_range = parent_operator_node.byte_range();
		if operator_range.len() < 3 {
			// Too short to be a valid string with quotes
			return None;
		}
		let operator_range = operator_range.shrink(1);
		let operator = &contents[operator_range];

		if !domain::is_subdomain_operator(operator) {
			return None;
		}

		// Get the parent field name to resolve its comodel
		if parent_field_node.kind() != "string" {
			return None;
		}

		let parent_field_range = parent_field_node.byte_range();
		if parent_field_range.len() < 3 {
			return None;
		}
		let parent_field_range = parent_field_range.shrink(1);
		let parent_field_name = &contents[parent_field_range];
		let base_field_name = parent_field_name.split('.').next().unwrap_or(parent_field_name);

		// Try to resolve the model chain - we might be nested multiple levels deep
		// First, try to get the model for the parent context
		let parent_model = self
			.resolve_subdomain_model_context(parent_field_node, 0, Some(base_model), contents)
			.unwrap_or_else(|| base_model.to_string());

		// Now resolve the field's comodel
		let sub_comodel = self.index.models.get_field_comodel(&parent_model, base_field_name)?;
		Some(_R(sub_comodel).to_string())
	}

	/// Find the full domain completion context at the given offset.
	/// 
	/// This is an enhanced version that returns detailed context about whether
	/// the cursor is in field, operator, or value position.
	fn find_domain_completion_context_full<'a>(
		&self,
		domain_node: Node<'a>,
		offset: usize,
		comodel_name: &str,
		contents: &str,
		depth: usize,
		max_depth: usize,
	) -> Option<DomainCompletionContext<'a>> {
		if depth > max_depth {
			return None;
		}

		let mut cursor = domain_node.walk();
		for child in domain_node.named_children(&mut cursor) {
			// Check if cursor is within this child's range
			if !child.byte_range().contains(&offset) {
				continue;
			}

			match child.kind() {
				"tuple" | "parenthesized_expression" => {
					// Domain tuple: ("field", "operator", value)
					let tuple_node = if child.kind() == "parenthesized_expression" {
						match child.named_child(0) {
							Some(inner) if inner.kind() == "tuple" => inner,
							_ => continue,
						}
					} else {
						child
					};

					return self.find_domain_tuple_completion_context_full(
						tuple_node,
						offset,
						comodel_name,
						contents,
						depth,
						max_depth,
					);
				}
				"list" => {
					// Nested list - recurse into it with same model
					return self.find_domain_completion_context_full(
						child,
						offset,
						comodel_name,
						contents,
						depth + 1,
						max_depth,
					);
				}
				"string" => {
					// Check if this is a domain-level operator ('&', '|', '!')
					let range = child.byte_range();
					if range.len() >= 2 {
						let inner = &contents[range.shrink(1)];
						if domain::is_domain_operator(inner) {
							// Cursor is on a domain operator - no completion needed
							return None;
						}
					}
				}
				_ => {}
			}
		}

		None
	}

	/// Find the full completion context within a single domain tuple.
	fn find_domain_tuple_completion_context_full<'a>(
		&self,
		tuple_node: Node<'a>,
		offset: usize,
		comodel_name: &str,
		contents: &str,
		depth: usize,
		max_depth: usize,
	) -> Option<DomainCompletionContext<'a>> {
		let mut cursor = tuple_node.walk();
		let mut children = tuple_node.named_children(&mut cursor);

		let field_node = children.next()?;
		let operator_node = children.next();
		let value_node = children.next();

		// Check if cursor is in the field position (first element)
		if field_node.byte_range().contains(&offset) && field_node.kind() == "string" {
			return Some(DomainCompletionContext::Field {
				model: comodel_name.to_string(),
				node: field_node,
			});
		}

		// Get field name and type for operator/value context
		let (field_name, field_type, selection_choices) = if field_node.kind() == "string" {
			let range = field_node.byte_range();
			if range.len() >= 2 {
				let name = &contents[range.shrink(1)];
				let base_name = name.split('.').next().unwrap_or(name);
				let (ftype, choices) = self.get_field_type_and_choices(comodel_name, base_name);
				(name.to_string(), ftype, choices)
			} else {
				(String::new(), None, None)
			}
		} else {
			(String::new(), None, None)
		};

		// Check if cursor is in the operator position (second element)
		if let Some(op_node) = operator_node {
			if op_node.byte_range().contains(&offset) && op_node.kind() == "string" {
				return Some(DomainCompletionContext::Operator {
					field_type,
					node: op_node,
				});
			}

			// Get operator for value context
			let operator = if op_node.kind() == "string" {
				let range = op_node.byte_range();
				if range.len() >= 2 {
					contents[range.shrink(1)].to_string()
				} else {
					String::new()
				}
			} else {
				String::new()
			};

			// Check if cursor is in the value position (third element)
			if let Some(val_node) = value_node {
				if val_node.byte_range().contains(&offset) {
					// Check if this is a subdomain operator and value is a list
					if domain::is_subdomain_operator(&operator) && val_node.kind() == "list" {
						// Resolve comodel and recurse into subdomain
						let base_field = field_name.split('.').next().unwrap_or(&field_name);
						if let Some(sub_comodel) = self.index.models.get_field_comodel(comodel_name, base_field) {
							let sub_comodel_name = _R(sub_comodel);
							return self.find_domain_completion_context_full(
								val_node,
								offset,
								sub_comodel_name,
								contents,
								depth + 1,
								max_depth,
							);
						}
					}

					// Return value context
					return Some(DomainCompletionContext::Value {
						field_type,
						operator,
						node: val_node,
						selection_choices,
					});
				}
			}
		}

		None
	}

	/// Get the field type category and selection choices for a field.
	fn get_field_type_and_choices(
		&self,
		model_name: &str,
		field_name: &str,
	) -> (Option<FieldTypeCategory>, Option<Vec<(String, String)>>) {
		let Some(model_key) = _G(model_name) else {
			return (None, None);
		};
		let Some(entry) = self.index.models.populate_properties(model_key.into(), &[]) else {
			return (None, None);
		};
		let Some(fields) = entry.fields.as_ref() else {
			return (None, None);
		};
		let Some(field_key) = _G(field_name) else {
			return (None, None);
		};
		let Some(field) = fields.get(&field_key) else {
			return (None, None);
		};
		
		let type_str = _R(field.type_);
		let field_type = FieldTypeCategory::from_field_type(type_str);
		
		// Get selection choices if available
		let choices = field.choices.as_ref().map(|c| {
			c.iter()
				.map(|(val, label)| (val.to_string(), label.to_string()))
				.collect()
		});
		
		(Some(field_type), choices)
	}

	/// Generate completions for domain operators.
	pub fn complete_domain_operators(
		&self,
		field_type: Option<FieldTypeCategory>,
		current_prefix: &str,
	) -> Vec<CompletionItem> {
		domain::get_operator_completions(field_type)
			.into_iter()
			.filter(|item| {
				current_prefix.is_empty() || item.label.starts_with(current_prefix)
			})
			.collect()
	}

	/// Generate completions for selection field values.
	pub fn complete_selection_values(
		&self,
		choices: &[(String, String)],
		current_prefix: &str,
		rope: RopeSlice<'_>,
		node: Node<'_>,
	) -> Vec<CompletionItem> {
		let inner_range = node.byte_range().shrink(1);
		let lsp_range = rope_conv(inner_range.map_unit(ByteOffset), rope);
		
		choices
			.iter()
			.filter(|(value, _)| current_prefix.is_empty() || value.starts_with(current_prefix))
			.map(|(value, label)| {
				CompletionItem {
					label: value.clone(),
					kind: Some(CompletionItemKind::ENUM_MEMBER),
					detail: Some(label.clone()),
					label_details: Some(CompletionItemLabelDetails {
						detail: None,
						description: Some("Selection value".to_string()),
					}),
					text_edit: Some(CompletionTextEdit::Edit(TextEdit {
						range: lsp_range,
						new_text: value.clone(),
					})),
					..Default::default()
				}
			})
			.collect()
	}

	/// Complete Many2one field names for the first part of a Properties definition.
	///
	/// Returns fields like `project_id`, `company_id` etc. that can contain
	/// a PropertiesDefinition on their target model.
	fn complete_definition_m2o_fields(
		&self,
		model: Option<&str>,
		needle: &str,
		_rope: RopeSlice<'_>,
		items: &mut MaxVec<CompletionItem>,
	) -> anyhow::Result<()> {
		use crate::model::FieldKind;

		let Some(model_name) = model else {
			return Ok(());
		};

		let Some(model_key) = _G(model_name) else {
			return Ok(());
		};

		let Some(entry) = self.index.models.populate_properties(model_key.into(), &[]) else {
			return Ok(());
		};

		let Some(fields) = entry.fields.as_ref() else {
			return Ok(());
		};

		for (field_sym, field) in fields.iter() {
			if !items.has_space() {
				break;
			}

			// Only include Many2one fields
			let type_str = _R(field.type_);
			if type_str != "Many2one" {
				continue;
			}

			let field_name = _R(*field_sym);
			if !field_name.starts_with(needle) {
				continue;
			}

			let comodel = match &field.kind {
				FieldKind::Relational(cm) => Some(_R(*cm).to_string()),
				_ => None,
			};

			items.push_checked(CompletionItem {
				label: field_name.to_string(),
				kind: Some(CompletionItemKind::FIELD),
				detail: Some("Many2one".to_string()),
				label_details: Some(CompletionItemLabelDetails {
					detail: Some(" Many2one".to_string()),
					description: comodel,
				}),
				insert_text: Some(format!("{}.", field_name)),
				..Default::default()
			});
		}

		Ok(())
	}

	/// Complete PropertiesDefinition field names for the second part of a Properties definition.
	///
	/// Given a Many2one field name, finds its target model and lists all
	/// PropertiesDefinition fields on that model.
	fn complete_definition_propdef_fields(
		&self,
		model: Option<&str>,
		m2o_field: &str,
		needle: &str,
		_rope: RopeSlice<'_>,
		items: &mut MaxVec<CompletionItem>,
	) -> anyhow::Result<()> {
		use crate::model::FieldKind;

		let Some(model_name) = model else {
			return Ok(());
		};

		let Some(model_key) = _G(model_name) else {
			return Ok(());
		};

		// Get the Many2one field and its comodel in a block to release the lock
		let comodel = {
			let Some(entry) = self.index.models.populate_properties(model_key.into(), &[]) else {
				return Ok(());
			};

			let Some(fields) = entry.fields.as_ref() else {
				return Ok(());
			};

			// Get the Many2one field and its comodel
			let Some(m2o_field_key) = _G(m2o_field) else {
				return Ok(());
			};

			let Some(m2o_field_entry) = fields.get(&m2o_field_key) else {
				return Ok(());
			};

			match &m2o_field_entry.kind {
				FieldKind::Relational(cm) => *cm,
				_ => return Ok(()),
			}
		};
		// Lock is now released

		// Now get PropertiesDefinition fields on the comodel
		let Some(comodel_entry) = self.index.models.populate_properties(comodel.into(), &[]) else {
			return Ok(());
		};

		let Some(comodel_fields) = comodel_entry.fields.as_ref() else {
			return Ok(());
		};

		for (field_sym, field) in comodel_fields.iter() {
			if !items.has_space() {
				break;
			}

			// Only include PropertiesDefinition fields
			let type_str = _R(field.type_);
			if type_str != "PropertiesDefinition" {
				continue;
			}

			let field_name = _R(*field_sym);
			if !field_name.starts_with(needle) {
				continue;
			}

			items.push_checked(CompletionItem {
				label: field_name.to_string(),
				kind: Some(CompletionItemKind::FIELD),
				detail: Some("PropertiesDefinition".to_string()),
				label_details: Some(CompletionItemLabelDetails {
					detail: Some(" PropertiesDefinition".to_string()),
					description: Some(_R(comodel).to_string()),
				}),
				..Default::default()
			});
		}

		Ok(())
	}
}
