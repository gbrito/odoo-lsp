use std::{borrow::Cow, cmp::Ordering, ops::ControlFlow};

use tower_lsp_server::ls_types::{Diagnostic, DiagnosticRelatedInformation, DiagnosticSeverity, Location};
use tracing::{debug, warn};
use tree_sitter::{Node, QueryCursor, QueryMatch};

use crate::analyze::type_cache;
use crate::domain::{self, DEFAULT_MAX_DOMAIN_DEPTH};
use crate::index::{_G, _R, Index, PathSymbol};
use crate::prelude::*;

use crate::{
	analyze::{MODEL_METHODS, Scope, Type, determine_scope},
	backend::Backend,
	model::{ModelBaseType, ModelName, ResolveMappedError},
};

use super::{Mapped, PyCompletions, PyImports, ThisModel, top_level_stmt};

/// Python extensions.
impl Backend {
	pub fn diagnose_python(
		&self,
		path: &str,
		rope: RopeSlice<'_>,
		damage_zone: Option<ByteRange>,
		diagnostics: &mut Vec<Diagnostic>,
	) {
		let Some(ast) = self.ast_map.get(path) else {
			warn!("Did not build AST for {path}");
			return;
		};
		let contents = Cow::from(rope);
		let query = PyCompletions::query();
		let mut root = ast.root_node();
		// TODO: Limit range of diagnostics with new heuristics
		if let Some(zone) = damage_zone.as_ref() {
			root = top_level_stmt(root, zone.end.0).unwrap_or(root);
			let before_count = diagnostics.len();
			diagnostics.retain(|diag| {
				// If we couldn't get a range here, rope has changed significantly so just toss the diag.
				let ByteOffset(start) = rope_conv(diag.range.start, rope);
				!root.byte_range().contains(&start)
			});
			debug!(
				"Retained {}/{} diagnostics after damage zone check",
				diagnostics.len(),
				before_count
			);
		} else {
			// There is no damage zone, assume everything has been reset.
			debug!("Clearing all diagnostics - no damage zone");
			diagnostics.clear();
		}
		let in_active_root =
			|range: core::ops::Range<usize>| damage_zone.as_ref().map(|zone| zone.intersects(range)).unwrap_or(true);

		// Diagnose missing imports
		self.diagnose_python_imports(diagnostics, &contents, ast.root_node());

		// Diagnose manifest dependencies if this is a __manifest__.py file
		if path.ends_with("__manifest__.py") {
			self.diagnose_manifest_dependencies(diagnostics, &contents, ast.root_node());
		}

		// Diagnose controller routes
		if let Some(module) = self.index.find_module_of(std::path::Path::new(path)) {
			self.diagnose_controller_routes(rope, diagnostics, &contents, ast.root_node(), module);
		}
		let top_level_ranges = root
			.named_children(&mut root.walk())
			.map(|node| node.byte_range())
			.collect::<Vec<_>>();
		let mut cursor = QueryCursor::new();
		let mut this_model = ThisModel::default();
		let mut matches = cursor.matches(query, root, contents.as_bytes());
		while let Some(match_) = matches.next() {
			let mut field_descriptors = vec![];
			let mut field_model = None;
			let mut is_properties_field = false;

			for capture in match_.captures {
				match PyCompletions::from(capture.index) {
					Some(PyCompletions::XmlId) => {
						if !in_active_root(capture.node.byte_range()) {
							continue;
						}

						let range = capture.node.byte_range().shrink(1);
						let mut slice = &contents[range.clone()];

						let mut xmlids = vec![];

						if match_
							.nodes_for_capture_index(PyCompletions::HasGroups as _)
							.next()
							.is_some()
						{
							let mut start = range.start;
							while let Some((xmlid, rest)) = slice.split_once(',') {
								let range = start..start + xmlid.len();
								start = range.end + 1;
								xmlids.push((xmlid, range));
								slice = rest;
							}
						} else {
							xmlids.push((slice, range));
						}
						for (xmlid, range) in xmlids {
							let mut id_found = false;
							if let Some(id) = _G(xmlid) {
								id_found = self.index.records.contains_key(&id);
							}

							if !id_found {
								let range = rope_conv(range.map_unit(ByteOffset), rope);
								diagnostics.push(Diagnostic {
									range,
									message: format!("No XML record with ID `{xmlid}` found"),
									severity: Some(DiagnosticSeverity::WARNING),
									..Default::default()
								})
							}
						}
					}
					Some(PyCompletions::Model) => {
						match capture.node.parent() {
							Some(subscript) if subscript.kind() == "subscript" => {
								// diagnose only, do not tag
								let range = capture.node.byte_range().shrink(1);
								let model = &contents[range.clone()];
								let model_key = _G(model);
								let has_model = model_key.map(|model| self.index.models.contains_key(&model));
								if !has_model.unwrap_or(false) {
									diagnostics.push(Diagnostic {
										range: rope_conv(range.map_unit(ByteOffset), rope),
										message: format!("`{model}` is not a valid model name"),
										severity: Some(DiagnosticSeverity::ERROR),
										..Default::default()
									})
								}
								continue;
							}
							_ => {}
						}
						if let Some(field_type) = match_.nodes_for_capture_index(PyCompletions::FieldType as _).next() {
							if !matches!(
								&contents[field_type.byte_range()],
								"One2many" | "Many2one" | "Many2many"
							) {
								continue;
							}
							let range = capture.node.byte_range().shrink(1);
							let model = &contents[range.clone()];
							let model_key = _G(model);
							let has_model = model_key.map(|model| self.index.models.contains_key(&model));
							if !has_model.unwrap_or(false) {
								diagnostics.push(Diagnostic {
									range: rope_conv(range.map_unit(ByteOffset), rope),
									message: format!("`{model}` is not a valid model name"),
									severity: Some(DiagnosticSeverity::ERROR),
									..Default::default()
								})
							} else if field_model.is_none() {
								field_model = Some(&contents[range]);
							}
							continue;
						}
						let Ok(idx) = top_level_ranges.binary_search_by(|range| {
							let needle = capture.node.end_byte();
							if needle < range.start {
								Ordering::Greater
							} else if needle > range.end {
								Ordering::Less
							} else {
								Ordering::Equal
							}
						}) else {
							debug!("binary search for top-level range failed");
							continue;
						};
						this_model.tag_model(capture.node, match_, top_level_ranges[idx].clone(), &contents);
					}
					Some(PyCompletions::FieldDescriptor) => {
						// fields.Many2one(field_descriptor=...)

						let Some(desc_value) = python_next_named_sibling(capture.node) else {
							continue;
						};

						let descriptor = &contents[capture.node.byte_range()];
						if matches!(
							descriptor,
							"comodel_name" | "domain" | "compute" | "search" | "inverse" | "related" | "groups" | "definition"
						) {
							field_descriptors.push((descriptor, desc_value));
						}
					}
					Some(PyCompletions::FieldType) => {
						// Track if this is a Properties field for definition validation
						let type_str = &contents[capture.node.byte_range()];
						if type_str == "Properties" {
							is_properties_field = true;
						}
					}
					Some(PyCompletions::Mapped) => {
						// First validate the field name
						self.diagnose_mapped(
							rope,
							diagnostics,
							&contents,
							root,
							this_model.inner,
							match_,
							capture.node.byte_range(),
							true,
						);
						// Then validate the operator and subdomain if applicable
						self.diagnose_domain_tuple_from_field(
							rope,
							diagnostics,
							&contents,
							root,
							this_model.inner,
							capture.node,
							match_,
							DEFAULT_MAX_DOMAIN_DEPTH,
						);
					}
					Some(PyCompletions::Scope) => {
						if !in_active_root(capture.node.byte_range()) {
							continue;
						}
						self.diagnose_python_scope(root, capture.node, &contents, diagnostics, path);
					}
					Some(PyCompletions::Request)
					| Some(PyCompletions::ForXmlId)
					| Some(PyCompletions::HasGroups)
					| Some(PyCompletions::MappedTarget)
					| Some(PyCompletions::Depends)
					| Some(PyCompletions::Prop)
					| Some(PyCompletions::ReadFn)
					| None => {}
				}
			}

			// post-process for field_descriptors
			for &(descriptor, node) in &field_descriptors {
				match descriptor {
					"compute" | "search" | "inverse" | "related" => self.diagnose_mapped(
						rope,
						diagnostics,
						&contents,
						root,
						this_model.inner,
						match_,
						node.byte_range(),
						descriptor == "related",
					),
					"comodel_name" => {
						let range = node.byte_range().shrink(1);
						let model = &contents[range.clone()];
						let model_key = _G(model);
						let has_model = model_key.map(|model| self.index.models.contains_key(&model));
						if !has_model.unwrap_or(false) {
							diagnostics.push(Diagnostic {
								range: rope_conv(range.map_unit(ByteOffset), rope),
								message: format!("`{model}` is not a valid model name"),
								severity: Some(DiagnosticSeverity::ERROR),
								..Default::default()
							})
						}
					}
					"domain" => {
						let mut domain_node = node;
						if domain_node.kind() == "lambda" {
							let Some(body) = domain_node.child_by_field_name("body") else {
								continue;
							};
							domain_node = body;
						}
						if domain_node.kind() != "list" {
							continue;
						}

						let Some(comodel_name) = field_model.or_else(|| {
							field_descriptors.iter().find_map(|&(desc, node)| {
								(desc == "comodel_name").then(|| &contents[node.byte_range().shrink(1)])
							})
						}) else {
							continue;
						};

						self.diagnose_domain(
							rope,
							diagnostics,
							&contents,
							root,
							comodel_name,
							domain_node,
							match_,
							0,
							DEFAULT_MAX_DOMAIN_DEPTH,
						);
					}
					"groups" => {
						// Validate groups - comma-separated XML IDs of res.groups records
						if node.kind() != "string" {
							continue;
						}
						let range = node.byte_range().shrink(1);
						let groups_str = &contents[range.clone()];
						
						// Parse comma-separated group IDs
						let mut start = range.start;
						let mut slice = groups_str;
						while !slice.is_empty() {
							let (xmlid, rest) = slice.split_once(',').unwrap_or((slice, ""));
							let xmlid = xmlid.trim();
							let xmlid_range = start..start + xmlid.len();
							start = xmlid_range.end + 1; // +1 for the comma
							
							if !xmlid.is_empty() {
								let mut id_found = false;
								if let Some(id) = _G(xmlid) {
									id_found = self.index.records.contains_key(&id);
								}
								
								if !id_found {
									diagnostics.push(Diagnostic {
										range: rope_conv(xmlid_range.map_unit(ByteOffset), rope),
										message: format!("No XML record with ID `{xmlid}` found"),
										severity: Some(DiagnosticSeverity::WARNING),
										..Default::default()
									});
								}
							}
							
							slice = rest;
						}
					}
					"definition" => {
						// Validate definition parameter for Properties fields
						// Format: "many2one_field.properties_definition_field"
						self.diagnose_properties_definition(
							rope,
							diagnostics,
							&contents,
							this_model.inner,
							node,
						);
					}
					_ => {}
				}
			}

			// Warn if Properties field is missing definition parameter
			if is_properties_field {
				let has_definition = field_descriptors.iter().any(|(desc, _)| *desc == "definition");
				if !has_definition {
					// Find the field type node to attach the warning
					if let Some(field_type_node) = match_.nodes_for_capture_index(PyCompletions::FieldType as _).next() {
						if &contents[field_type_node.byte_range()] == "Properties" {
							diagnostics.push(Diagnostic {
								range: rope_conv(field_type_node.byte_range().map_unit(ByteOffset), rope),
								message: "Properties field should have a 'definition' parameter specifying the definition source".to_string(),
								severity: Some(DiagnosticSeverity::WARNING),
								..Default::default()
							});
						}
					}
				}
			}
		}

		// Diagnose missing super() calls in overridden methods
		// Only run on full diagnostics (no damage zone) to avoid partial results
		if damage_zone.is_none() {
			let file_path = std::path::Path::new(path);
			if let Some(root_path) = self.index.find_root_of(file_path) {
				let root_spur = crate::index::_I(root_path.to_str().unwrap_or(""));
				let path_sym = PathSymbol::strip_root(root_spur, file_path);
				self.diagnose_missing_super(path_sym, diagnostics);
			}
		}

		// Diagnose models without access rules (once per file)
		// Only run if the current module has at least some access rules defined
		// (to avoid noisy warnings in modules that don't manage security)
		if damage_zone.is_none() {
			let file_path = std::path::Path::new(path);
			if let Some(current_module) = self.index.find_module_of(file_path) {
				// Only warn about missing access rules if this module has any access rules at all
				// This avoids noise in modules that don't explicitly manage security
				if self.index.access_rules.module_has_any_rules(current_module) {
					self.diagnose_models_without_access_rules(rope, diagnostics, &contents, root);
				}
			}
		}
	}

	/// Check if any model in this file lacks access rules.
	/// Only emits one diagnostic per file (for the first model found without rules).
	fn diagnose_models_without_access_rules(
		&self,
		rope: RopeSlice<'_>,
		diagnostics: &mut Vec<Diagnostic>,
		contents: &str,
		root: Node,
	) {
		use crate::index::ModelQuery;

		let query = ModelQuery::query();
		let mut cursor = QueryCursor::new();
		let mut matches = cursor.matches(query, root, contents.as_bytes());

		// Track models we've seen in this file to find those with _name definitions
		let mut models_in_file: std::collections::HashMap<usize, ModelInfo> = std::collections::HashMap::new();

		struct ModelInfo {
			name: Option<String>,
			name_range: Option<core::ops::Range<usize>>,
			base_type: ModelBaseType,
		}

		while let Some(match_) = matches.next() {
			let Some(model_node) = match_.nodes_for_capture_index(ModelQuery::Model as _).next() else {
				continue;
			};
			let Some(capture) = match_.nodes_for_capture_index(ModelQuery::Name as _).next() else {
				continue;
			};

			let model_id = model_node.id();
			let model_info = models_in_file.entry(model_id).or_insert_with(|| {
				// Get base class type
				let base_type = match_
					.nodes_for_capture_index(ModelQuery::BaseClass as _)
					.next()
					.map(|node| {
						let class_name = &contents[node.byte_range()];
						ModelBaseType::from_class_name(class_name)
					})
					.unwrap_or_default();

				ModelInfo {
					name: None,
					name_range: None,
					base_type,
				}
			});

			// Check if this is a _name assignment
			let attr_name = &contents[capture.byte_range()];
			if attr_name == "_name" {
				if let Some(name_value) = super::python_next_named_sibling(capture) {
					if name_value.kind() == "string" {
						let name_range = name_value.byte_range().shrink(1);
						if !name_range.is_empty() {
							model_info.name = Some(contents[name_range.clone()].to_string());
							model_info.name_range = Some(name_range);
						}
					}
				}
			}
		}

		// Check each model for access rules, but only emit ONE diagnostic per file
		for model_info in models_in_file.values() {
			// Skip if no _name defined (inherit-only class)
			let Some(model_name) = &model_info.name else {
				continue;
			};

			// Skip AbstractModel - they cannot have access rules
			if model_info.base_type == ModelBaseType::AbstractModel {
				continue;
			}

			// Check if model has access rules
			if let Some(model_key) = _G(model_name) {
				if !self.index.access_rules.has_rules_for_model(&model_key.into()) {
					// Emit diagnostic at the _name value location
					if let Some(name_range) = &model_info.name_range {
						diagnostics.push(Diagnostic {
							range: rope_conv(name_range.clone().map_unit(ByteOffset), rope),
							severity: Some(DiagnosticSeverity::WARNING),
							message: format!(
								"Model `{}` has no access rules defined. Consider adding rules in security/ir.model.access.csv",
								model_name
							),
							..Default::default()
						});
						// Only emit once per file
						return;
					}
				}
			}
		}
	}
	fn diagnose_python_scope(
		&self,
		root: Node,
		node: Node,
		contents: &str,
		diagnostics: &mut Vec<Diagnostic>,
		path: &str,
	) {
		// Most of these steps are similar to what is done inside model_of_range.
		let offset = node.start_byte();
		let Some((self_type, fn_scope, self_param)) = determine_scope(root, contents, offset) else {
			return;
		};
		let mut scope = Scope::default();
		let self_type = match self_type {
			Some(type_) => &contents[type_.byte_range().shrink(1)],
			None => "",
		};
		scope.super_ = Some(self_param.into());
		scope.insert(self_param.to_string(), Type::Model(self_type.into()));
		let scope_end = fn_scope.end_byte();
		Index::walk_scope(fn_scope, Some(scope), |scope, node| {
			let entered = (self.index).build_scope(scope, node, scope_end, contents)?;

			let attribute = node.child_by_field_name("attribute");
			if node.kind() != "attribute" || attribute.as_ref().unwrap().kind() != "identifier" {
				return ControlFlow::Continue(entered);
			}

			let attribute = attribute.unwrap();
			#[rustfmt::skip]
			static MODEL_BUILTINS: phf::Set<&str> = phf::phf_set!(
				"env", "id", "ids", "display_name", "create_date", "write_date",
				"create_uid", "write_uid", "pool", "record", "flush_model", "mapped",
				"grouped", "_read_group", "filtered", "sorted", "_origin", "fields_get",
				"user_has_groups", "read",
			);
			let prop = &contents[attribute.byte_range()];
			if prop.starts_with('_') || MODEL_BUILTINS.contains(prop) || MODEL_METHODS.contains(prop) {
				return ControlFlow::Continue(entered);
			}

			let Some(lhs_t) = (self.index).type_of(node.child_by_field_name("object").unwrap(), scope, contents) else {
				return ControlFlow::Continue(entered);
			};
			let lhs_t = type_cache().resolve(lhs_t);

			let Some(model_name) = (self.index).try_resolve_model(lhs_t, scope) else {
				return ControlFlow::Continue(entered);
			};

			if (self.index).has_attribute(lhs_t, &contents[attribute.byte_range()], scope) {
				return ControlFlow::Continue(entered);
			}

			// HACK: fix this issue where the model name is just empty
			if _R(model_name).is_empty() {
				return ControlFlow::Continue(entered);
			}

			// Check if the attribute might belong to an unloaded auto_install module
			let attr_name = &contents[attribute.byte_range()];
			debug!(
				"Checking unloaded auto_install for model: {} attribute: {}",
				_R(model_name),
				attr_name
			);
			let diagnostic_message = format!("Model `{}` has no property `{}`", _R(model_name), attr_name);

			// Build related information if this is an auto_install issue
			let related_information = if let Some((module_name, missing_deps_with_chains)) =
				self.index.get_unloaded_auto_install_for_model(_R(model_name))
			{
				self.build_auto_install_related_info(
					module_name,
					&missing_deps_with_chains,
					model_name,
					attr_name,
					path,
				)
			} else {
				None
			};

			diagnostics.push(Diagnostic {
				range: span_conv(attribute.range()),
				severity: Some(DiagnosticSeverity::ERROR),
				message: diagnostic_message,
				related_information,
				..Default::default()
			});

			ControlFlow::Continue(entered)
		});
	}

	fn diagnose_python_imports(&self, diagnostics: &mut Vec<Diagnostic>, contents: &str, root: Node) {
		let query = PyImports::query();
		let mut cursor = tree_sitter::QueryCursor::new();

		let mut matches = cursor.matches(query, root, contents.as_bytes());
		while let Some(match_) = matches.next() {
			let mut module_path = None;
			let mut import_name = None;
			let mut import_node = None;

			for capture in match_.captures {
				match PyImports::from(capture.index) {
					Some(PyImports::ImportModule) => {
						let capture_text = &contents[capture.node.byte_range()];
						module_path = Some(capture_text.to_string());
					}
					Some(PyImports::ImportName) => {
						let capture_text = &contents[capture.node.byte_range()];
						import_name = Some(capture_text.to_string());
						import_node = Some(capture.node);
					}
					Some(PyImports::ImportAlias) => {
						// We still want to check the original import name, not the alias
					}
					_ => {}
				}
			}

			if let (Some(name), Some(node)) = (import_name, import_node) {
				let full_module_path = if let Some(module) = module_path {
					module // For "from module import name", the module path is just the module
				} else {
					name.clone() // For "import name", the module path is the name itself
				};

				// Only check imports from odoo.addons.module_name pattern
				if !full_module_path.starts_with("odoo.addons.") {
					continue;
				}

				// Try to resolve the module path
				if self.index.resolve_py_module(&full_module_path).is_none() {
					diagnostics.push(Diagnostic {
						range: span_conv(node.range()),
						message: format!("Cannot resolve import '{name}'"),
						severity: Some(DiagnosticSeverity::ERROR),
						..Default::default()
					});
				}
			}
		}
	}

	/// Validate the `definition` parameter for Properties fields.
	///
	/// The definition format is: "many2one_field.properties_definition_field"
	/// - First part must be a Many2one field on the current model
	/// - Second part must be a PropertiesDefinition field on the target model
	fn diagnose_properties_definition(
		&self,
		rope: RopeSlice<'_>,
		diagnostics: &mut Vec<Diagnostic>,
		contents: &str,
		model: Option<&str>,
		node: Node<'_>,
	) {
		use crate::model::FieldKind;

		if node.kind() != "string" {
			return;
		}

		let range = node.byte_range().shrink(1);
		if range.is_empty() {
			return;
		}
		let definition_str = &contents[range.clone()];

		// Validate format: must contain exactly one dot
		let Some(dot_pos) = definition_str.find('.') else {
			diagnostics.push(Diagnostic {
				range: rope_conv(range.map_unit(ByteOffset), rope),
				message: format!(
					"Invalid definition format '{}'. Expected: 'many2one_field.properties_definition_field'",
					definition_str
				),
				severity: Some(DiagnosticSeverity::ERROR),
				..Default::default()
			});
			return;
		};

		// Check for multiple dots
		if definition_str[dot_pos + 1..].contains('.') {
			diagnostics.push(Diagnostic {
				range: rope_conv(range.map_unit(ByteOffset), rope),
				message: format!(
					"Invalid definition format '{}'. Expected exactly one dot: 'many2one_field.properties_definition_field'",
					definition_str
				),
				severity: Some(DiagnosticSeverity::ERROR),
				..Default::default()
			});
			return;
		}

		let m2o_field = &definition_str[..dot_pos];
		let propdef_field = &definition_str[dot_pos + 1..];

		if m2o_field.is_empty() {
			diagnostics.push(Diagnostic {
				range: rope_conv((range.start..range.start + 1).map_unit(ByteOffset), rope),
				message: "Many2one field name is missing before the dot".to_string(),
				severity: Some(DiagnosticSeverity::ERROR),
				..Default::default()
			});
			return;
		}

		if propdef_field.is_empty() {
			diagnostics.push(Diagnostic {
				range: rope_conv((range.start + dot_pos..range.end).map_unit(ByteOffset), rope),
				message: "PropertiesDefinition field name is missing after the dot".to_string(),
				severity: Some(DiagnosticSeverity::ERROR),
				..Default::default()
			});
			return;
		}

		// Validate m2o_field is a Many2one field on this model
		let Some(model_name) = model else {
			return; // Can't validate without model context
		};

		let Some(model_key) = _G(model_name) else {
			return;
		};

		let m2o_field_range = (range.start..range.start + m2o_field.len()).map_unit(ByteOffset);
		let propdef_field_range = (range.start + dot_pos + 1..range.end).map_unit(ByteOffset);

		// First, validate the m2o field and get the comodel in a block to release the lock
		let comodel = {
			let Some(entry) = self.index.models.populate_properties(model_key.into(), &[]) else {
				return;
			};

			let Some(fields) = entry.fields.as_ref() else {
				return;
			};

			let Some(m2o_field_key) = _G(m2o_field) else {
				diagnostics.push(Diagnostic {
					range: rope_conv(m2o_field_range.clone(), rope),
					message: format!("Field '{}' not found on model '{}'", m2o_field, model_name),
					severity: Some(DiagnosticSeverity::ERROR),
					..Default::default()
				});
				return;
			};

			let Some(m2o_field_entry) = fields.get(&m2o_field_key) else {
				diagnostics.push(Diagnostic {
					range: rope_conv(m2o_field_range.clone(), rope),
					message: format!("Field '{}' not found on model '{}'", m2o_field, model_name),
					severity: Some(DiagnosticSeverity::ERROR),
					..Default::default()
				});
				return;
			};

			// Check that it's a Many2one field
			match &m2o_field_entry.kind {
				FieldKind::Relational(comodel) => {
					let type_str = _R(m2o_field_entry.type_);
					if type_str != "Many2one" {
						diagnostics.push(Diagnostic {
							range: rope_conv(m2o_field_range, rope),
							message: format!(
								"Field '{}' must be a Many2one field, but it's a {} field",
								m2o_field, type_str
							),
							severity: Some(DiagnosticSeverity::ERROR),
							..Default::default()
						});
						return;
					}
					*comodel
				}
				FieldKind::Value => {
					let type_str = _R(m2o_field_entry.type_);
					diagnostics.push(Diagnostic {
						range: rope_conv(m2o_field_range, rope),
						message: format!(
							"Field '{}' must be a Many2one field, but it's a {} field",
							m2o_field, type_str
						),
						severity: Some(DiagnosticSeverity::ERROR),
						..Default::default()
					});
					return;
				}
				FieldKind::Related(path) => {
					// For related fields, we'd need to resolve the path - skip for now
					debug!(
						"Properties definition references a related field '{}' -> '{}', skipping deep validation",
						m2o_field, path
					);
					return;
				}
			}
		};
		// Lock is now released

		// Now validate the PropertiesDefinition field on the comodel
		let comodel_name = _R(comodel);
		let Some(comodel_entry) = self.index.models.populate_properties(comodel.into(), &[]) else {
			return;
		};

		let Some(comodel_fields) = comodel_entry.fields.as_ref() else {
			return;
		};

		let Some(propdef_field_key) = _G(propdef_field) else {
			diagnostics.push(Diagnostic {
				range: rope_conv(propdef_field_range, rope),
				message: format!("Field '{}' not found on model '{}'", propdef_field, comodel_name),
				severity: Some(DiagnosticSeverity::ERROR),
				..Default::default()
			});
			return;
		};

		let Some(propdef_entry) = comodel_fields.get(&propdef_field_key) else {
			diagnostics.push(Diagnostic {
				range: rope_conv(propdef_field_range, rope),
				message: format!("Field '{}' not found on model '{}'", propdef_field, comodel_name),
				severity: Some(DiagnosticSeverity::ERROR),
				..Default::default()
			});
			return;
		};

		// Check that it's a PropertiesDefinition field
		let type_str = _R(propdef_entry.type_);
		if type_str != "PropertiesDefinition" {
			diagnostics.push(Diagnostic {
				range: rope_conv(propdef_field_range, rope),
				message: format!(
					"Field '{}' must be a PropertiesDefinition field, but it's a {} field",
					propdef_field, type_str
				),
				severity: Some(DiagnosticSeverity::ERROR),
				..Default::default()
			});
		}
	}

	pub(crate) fn diagnose_mapped(
		&self,
		rope: RopeSlice<'_>,
		diagnostics: &mut Vec<Diagnostic>,
		contents: &str,
		root: Node<'_>,
		model: Option<&str>,
		match_: &QueryMatch<'_, '_>,
		mapped_range: std::ops::Range<usize>,
		expect_field: bool,
	) {
		let Some(Mapped {
			mut needle,
			model,
			single_field,
			mut range,
		}) = self.gather_mapped(
			root,
			match_,
			None,
			mapped_range,
			model,
			contents,
			false,
			(!expect_field).then_some(true),
		)
		else {
			return;
		};
		let mut model = _I(model);
		if single_field {
			if let Some(dot) = needle.find('.') {
				let message_range = range.start.0 + dot..range.end.0;
				diagnostics.push(Diagnostic {
					range: rope_conv(message_range.map_unit(ByteOffset), rope),
					severity: Some(DiagnosticSeverity::ERROR),
					message: "Dotted access is not supported in this context".to_string(),
					..Default::default()
				});
				needle = &needle[..dot];
				range = (range.start.0..range.start.0 + dot).map_unit(ByteOffset);
			}
		} else {
			match (self.index.models).resolve_mapped(&mut model, &mut needle, Some(&mut range)) {
				Ok(()) => {}
				Err(ResolveMappedError::NonRelational) => {
					diagnostics.push(Diagnostic {
						range: rope_conv(range, rope),
						severity: Some(DiagnosticSeverity::ERROR),
						message: format!("`{needle}` is not a relational field"),
						..Default::default()
					});
					return;
				}
				Err(ResolveMappedError::Properties { .. }) => {
					// Properties fields support dynamic property access in domains.
					// We can't validate the property name since it's defined in PropertiesDefinition.
					// Just return without error - this is valid syntax.
					return;
				}
			}
		}
		if needle.is_empty() {
			// Nothing to compare yet, keep going.
			return;
		}
		let mut has_property = false;
		if self.index.models.contains_key(&model) {
			let Some(entry) = self.index.models.populate_properties(model.into(), &[]) else {
				return;
			};
			static MAPPED_BUILTINS: phf::Set<&str> = phf::phf_set!(
				"id",
				"ids",
				"display_name",
				"create_date",
				"write_date",
				"create_uid",
				"write_uid"
			);
			if MAPPED_BUILTINS.contains(needle) {
				return;
			}
			if let Some(key) = _G(needle) {
				if expect_field {
					let Some(fields) = entry.fields.as_ref() else { return };
					has_property = fields.contains_key(&key)
				} else {
					let Some(methods) = entry.methods.as_ref() else { return };
					has_property = methods.contains_key(&key);
				}
			}
		}
		if !has_property {
			diagnostics.push(Diagnostic {
				range: rope_conv(range, rope),
				severity: Some(DiagnosticSeverity::ERROR),
				message: format!(
					"Model `{}` has no {} `{needle}`",
					_R(model),
					if expect_field { "field" } else { "method" }
				),
				..Default::default()
			});
		}
	}

	/// Diagnose a domain expression, including operator validation and subdomain recursion.
	///
	/// This function walks through domain tuples and:
	/// 1. Validates field names exist on the model
	/// 2. Validates operators are valid Odoo domain operators
	/// 3. Recursively processes subdomains for `any`/`not any` operators
	///
	/// # Parameters
	/// - `comodel_name`: The model that this domain filters on
	/// - `domain_node`: A tree-sitter node representing the domain list `[...]`
	/// - `depth`: Current recursion depth (for subdomain nesting)
	/// - `max_depth`: Maximum allowed recursion depth
	#[allow(clippy::too_many_arguments)]
	fn diagnose_domain(
		&self,
		rope: RopeSlice<'_>,
		diagnostics: &mut Vec<Diagnostic>,
		contents: &str,
		root: Node<'_>,
		comodel_name: &str,
		domain_node: Node<'_>,
		match_: &QueryMatch<'_, '_>,
		depth: usize,
		max_depth: usize,
	) {
		if depth > max_depth {
			// Prevent infinite recursion
			return;
		}

		// Validate domain structure (Polish notation) at the top level only
		if depth == 0 {
			self.validate_domain_structure_diagnostics(rope, diagnostics, contents, domain_node);
		}

		for child in domain_node.named_children(&mut domain_node.walk()) {
			match child.kind() {
				"tuple" | "parenthesized_expression" => {
					// Domain tuple: ("field", "operator", value)
					// May be wrapped in parentheses for line continuation
					let tuple_node = if child.kind() == "parenthesized_expression" {
						// Unwrap parenthesized expression to get the actual tuple
						match child.named_child(0) {
							Some(inner) if inner.kind() == "tuple" => inner,
							_ => continue,
						}
					} else {
						child
					};

					self.diagnose_domain_tuple(
						rope,
						diagnostics,
						contents,
						root,
						comodel_name,
						tuple_node,
						match_,
						depth,
						max_depth,
					);
				}
				"string" => {
					// Domain-level boolean operator: '&', '|', '!'
					let range = child.byte_range().shrink(1);
					let operator = &contents[range.clone()];
					if !domain::is_domain_operator(operator) {
						// Not a valid domain operator - could be an error or just a non-standard element
						// We'll be lenient here since the domain structure might be dynamic
					}
				}
				"list" => {
					// Nested list - this could be a subdomain or just a malformed domain
					// We don't diagnose this directly; it should be handled via tuple processing
				}
				_ => {
					// Other node types (identifiers, calls, etc.) - these could be
					// dynamic domain construction, so we don't flag them
				}
			}
		}
	}

	/// Validate the structure of a domain expression (Polish notation).
	///
	/// Checks that domain-level operators (&, |, !) have the correct arity
	/// and that the overall structure is well-formed.
	fn validate_domain_structure_diagnostics(
		&self,
		rope: RopeSlice<'_>,
		diagnostics: &mut Vec<Diagnostic>,
		contents: &str,
		domain_node: Node<'_>,
	) {
		// Collect domain elements and their positions
		let mut elements = Vec::new();
		let mut element_ranges = Vec::new();
		let mut has_dynamic_elements = false;

		for child in domain_node.named_children(&mut domain_node.walk()) {
			match child.kind() {
				"tuple" | "parenthesized_expression" => {
					// Domain term
					elements.push(domain::DomainElement::Term);
					element_ranges.push(child.byte_range());
				}
				"string" => {
					// Could be a domain-level operator
					let range = child.byte_range();
					if range.len() >= 2 {
						let inner_range = range.clone().shrink(1);
						let op = &contents[inner_range];
						if domain::is_domain_operator(op) {
							elements.push(domain::DomainElement::Operator(op.to_string()));
							element_ranges.push(range);
						} else {
							// Unknown string element - might be a term reference or error
							// Be lenient and treat as term
							elements.push(domain::DomainElement::Term);
							element_ranges.push(range);
						}
					}
				}
				"list" => {
					// Nested list without proper tuple structure
					// Could be malformed or dynamic
					elements.push(domain::DomainElement::Term);
					element_ranges.push(child.byte_range());
				}
				"identifier" | "call" | "attribute" | "binary_operator" | "unary_operator" => {
					// Dynamic domain construction - skip structure validation
					has_dynamic_elements = true;
				}
				_ => {
					// Other elements - might be dynamic
					has_dynamic_elements = true;
				}
			}
		}

		// Skip validation if domain has dynamic elements
		if has_dynamic_elements || elements.is_empty() {
			return;
		}

		// Validate structure
		let validation = domain::validate_domain_structure(&elements);
		if !validation.is_valid {
			if let Some(error_msg) = validation.error {
				// Determine the range for the diagnostic
				let diag_range = if let Some(pos) = validation.error_position {
					// Error at specific element
					element_ranges.get(pos).cloned().unwrap_or(domain_node.byte_range())
				} else {
					// General structure error - use the whole domain
					domain_node.byte_range()
				};

				diagnostics.push(Diagnostic {
					range: rope_conv(diag_range.map_unit(ByteOffset), rope),
					severity: Some(DiagnosticSeverity::ERROR),
					message: error_msg,
					..Default::default()
				});
			}
		}
	}

	/// Diagnose domain operator and subdomain starting from a captured field string node.
	///
	/// This function is called for MAPPED captures in method call domains (search, etc.)
	/// where we have the field string node but need to find the containing tuple to
	/// validate the operator.
	#[allow(clippy::too_many_arguments)]
	fn diagnose_domain_tuple_from_field(
		&self,
		rope: RopeSlice<'_>,
		diagnostics: &mut Vec<Diagnostic>,
		contents: &str,
		root: Node<'_>,
		model: Option<&str>,
		field_node: Node<'_>,
		match_: &QueryMatch<'_, '_>,
		max_depth: usize,
	) {
		// Navigate up to find the containing tuple
		let Some(tuple_node) = field_node.parent().filter(|p| p.kind() == "tuple") else {
			// Could be in parenthesized_expression
			let Some(paren) = field_node.parent().filter(|p| p.kind() == "parenthesized_expression") else {
				return;
			};
			let Some(tuple_node) = paren.parent().filter(|p| p.kind() == "tuple") else {
				return;
			};
			// Fall through with tuple_node from parenthesized context
			return self.diagnose_domain_tuple_operator(
				rope,
				diagnostics,
				contents,
				root,
				model,
				tuple_node,
				match_,
				0,
				max_depth,
			);
		};

		self.diagnose_domain_tuple_operator(
			rope,
			diagnostics,
			contents,
			root,
			model,
			tuple_node,
			match_,
			0,
			max_depth,
		);
	}

	/// Validate the operator in a domain tuple and recurse into subdomains if applicable.
	#[allow(clippy::too_many_arguments)]
	fn diagnose_domain_tuple_operator(
		&self,
		rope: RopeSlice<'_>,
		diagnostics: &mut Vec<Diagnostic>,
		contents: &str,
		root: Node<'_>,
		model: Option<&str>,
		tuple_node: Node<'_>,
		match_: &QueryMatch<'_, '_>,
		depth: usize,
		max_depth: usize,
	) {
		if depth > max_depth {
			return;
		}

		let Some(model_name) = model else {
			return;
		};

		// Get elements of the domain tuple
		let mut cursor = tuple_node.walk();
		let mut children = tuple_node.named_children(&mut cursor);
		let field_node = children.next();
		let operator_node = children.next();
		let value_node = children.next();

		// Validate operator (second element)
		let Some(operator_node) = operator_node else {
			return;
		};

		if operator_node.kind() != "string" {
			return;
		}

		let operator_range = operator_node.byte_range();
		if operator_range.len() < 3 {
			// Too short to be a valid string with quotes
			return;
		}
		let operator_range = operator_range.shrink(1);
		let operator = &contents[operator_range.clone()];

		if !domain::is_valid_operator(operator) {
			diagnostics.push(Diagnostic {
				range: rope_conv(operator_range.map_unit(ByteOffset), rope),
				severity: Some(DiagnosticSeverity::ERROR),
				message: format!(
					"Invalid domain operator `{}`. Valid operators: {}",
					operator,
					domain::format_valid_operators()
				),
				..Default::default()
			});
			return;
		}

		// Check operator-field type compatibility (emit warnings for unusual combinations)
		// Skip this check for subdomain operators ('any', 'not any') as they have specialized error handling
		if !domain::is_subdomain_operator(operator) {
			if let Some(field_node) = field_node {
				if field_node.kind() == "string" {
					let field_range = field_node.byte_range();
					if field_range.len() >= 3 {
						let field_name = &contents[field_range.shrink(1)];
						let base_field_name = field_name.split('.').next().unwrap_or(field_name);
						
						// Get field type
						if let Some(model_key) = _G(model_name) {
							if let Some(entry) = self.index.models.populate_properties(model_key.into(), &[]) {
								if let Some(fields) = entry.fields.as_ref() {
									if let Some(field_key) = _G(base_field_name) {
										if let Some(field) = fields.get(&field_key) {
											let type_str = _R(field.type_);
											let field_type = domain::FieldTypeCategory::from_field_type(type_str);
											let (_, is_warning) = field_type.check_operator(operator);
											
											if is_warning {
												diagnostics.push(Diagnostic {
													range: rope_conv(operator_range.clone().map_unit(ByteOffset), rope),
													severity: Some(DiagnosticSeverity::WARNING),
													message: format!(
														"Operator `{}` is unusual for {} field `{}`",
														operator, type_str, base_field_name
													),
													..Default::default()
												});
											}
										}
									}
								}
							}
						}
					}
				}
			}
		}

		// Validate value type for 'in' and 'not in' operators
		if domain::LIST_VALUE_OPERATORS.contains(domain::normalize_operator(&operator.to_lowercase())) {
			if let Some(value_node) = value_node {
				// 'in' and 'not in' require list/tuple values
				if !matches!(value_node.kind(), "list" | "tuple" | "parenthesized_expression") {
					// Check if it's a parenthesized list/tuple
					let is_paren_list = value_node.kind() == "parenthesized_expression"
						&& value_node.named_child(0).map(|c| matches!(c.kind(), "list" | "tuple")).unwrap_or(false);
					
					if !is_paren_list {
						diagnostics.push(Diagnostic {
							range: rope_conv(value_node.byte_range().map_unit(ByteOffset), rope),
							severity: Some(DiagnosticSeverity::ERROR),
							message: format!(
								"Operator `{}` requires a list or tuple value, got {}",
								operator,
								value_node.kind()
							),
							..Default::default()
						});
					}
				}
			}
		}

		// Handle subdomain operators ('any', 'not any')
		if domain::is_subdomain_operator(operator) {
			let Some(value_node) = value_node else {
				return;
			};

			// For subdomain operators, the value must be a list
			if value_node.kind() != "list" {
				diagnostics.push(Diagnostic {
					range: rope_conv(value_node.byte_range().map_unit(ByteOffset), rope),
					severity: Some(DiagnosticSeverity::ERROR),
					message: format!(
						"Operator `{}` requires a domain list as value, got {}",
						operator,
						value_node.kind()
					),
					..Default::default()
				});
				return;
			}

			// Get the field name to resolve its comodel
			let Some(field_node) = field_node else {
				return;
			};

			if field_node.kind() != "string" {
				return;
			}

			let field_range = field_node.byte_range().shrink(1);
			let field_name = &contents[field_range.clone()];
			let base_field_name = field_name.split('.').next().unwrap_or(field_name);

			// Resolve the field's comodel
			if let Some(sub_comodel) = self.index.models.get_field_comodel(model_name, base_field_name) {
				let sub_comodel_name = _R(sub_comodel);
				// Recurse into the subdomain
				self.diagnose_domain(
					rope,
					diagnostics,
					contents,
					root,
					sub_comodel_name,
					value_node,
					match_,
					depth + 1,
					max_depth,
				);
			} else {
				// Field is not relational or doesn't exist
				if let Some(model_key) = _G(model_name) {
					if let Some(entry) = self.index.models.populate_properties(model_key.into(), &[]) {
						if let Some(fields) = entry.fields.as_ref() {
							if let Some(field_key) = _G(base_field_name) {
								if fields.contains_key(&field_key) {
									diagnostics.push(Diagnostic {
										range: rope_conv(operator_range.map_unit(ByteOffset), rope),
										severity: Some(DiagnosticSeverity::ERROR),
										message: format!(
											"Operator `{}` can only be used with relational fields (Many2one, One2many, Many2many), but `{}` is not relational",
											operator,
											base_field_name
										),
										..Default::default()
									});
								}
							}
						}
					}
				}
			}
		}
	}

	/// Diagnose a single domain tuple: ("field", "operator", value)
	#[allow(clippy::too_many_arguments)]
	fn diagnose_domain_tuple(
		&self,
		rope: RopeSlice<'_>,
		diagnostics: &mut Vec<Diagnostic>,
		contents: &str,
		root: Node<'_>,
		comodel_name: &str,
		tuple_node: Node<'_>,
		match_: &QueryMatch<'_, '_>,
		depth: usize,
		max_depth: usize,
	) {
		// Get the three elements of the domain tuple
		let mut cursor = tuple_node.walk();
		let mut children = tuple_node.named_children(&mut cursor);
		let field_node = children.next();
		let operator_node = children.next();
		let value_node = children.next();

		// Validate field name (first element)
		if let Some(field_node) = field_node {
			if field_node.kind() == "string" {
				self.diagnose_mapped(
					rope,
					diagnostics,
					contents,
					root,
					Some(comodel_name),
					match_,
					field_node.byte_range(),
					true,
				);
			}
		}

		// Validate operator (second element)
		let Some(operator_node) = operator_node else {
			return;
		};

		if operator_node.kind() != "string" {
			// Operator is not a string literal - could be dynamic, skip validation
			return;
		}

		let operator_node_range = operator_node.byte_range();
		if operator_node_range.len() < 3 {
			// Too short to be a valid quoted string
			return;
		}
		let operator_range = operator_node_range.clone().shrink(1);
		let operator = &contents[operator_range.clone()];

		if !domain::is_valid_operator(operator) {
			diagnostics.push(Diagnostic {
				range: rope_conv(operator_range.map_unit(ByteOffset), rope),
				severity: Some(DiagnosticSeverity::ERROR),
				message: format!(
					"Invalid domain operator `{}`. Valid operators: {}",
					operator,
					domain::format_valid_operators()
				),
				..Default::default()
			});
			return;
		}

		// Check operator-field type compatibility (emit warnings for unusual combinations)
		// Skip this check for subdomain operators ('any', 'not any') as they have specialized error handling
		if !domain::is_subdomain_operator(operator) {
			if let Some(field_node) = field_node {
				if field_node.kind() == "string" {
					let field_range = field_node.byte_range();
					if field_range.len() >= 3 {
						let field_name = &contents[field_range.shrink(1)];
						let base_field_name = field_name.split('.').next().unwrap_or(field_name);
						
						// Get field type
						if let Some(model_key) = _G(comodel_name) {
							if let Some(entry) = self.index.models.populate_properties(model_key.into(), &[]) {
								if let Some(fields) = entry.fields.as_ref() {
									if let Some(field_key) = _G(base_field_name) {
										if let Some(field) = fields.get(&field_key) {
											let type_str = _R(field.type_);
											let field_type = domain::FieldTypeCategory::from_field_type(type_str);
											let (_, is_warning) = field_type.check_operator(operator);
											
											if is_warning {
												diagnostics.push(Diagnostic {
													range: rope_conv(operator_range.clone().map_unit(ByteOffset), rope),
													severity: Some(DiagnosticSeverity::WARNING),
													message: format!(
														"Operator `{}` is unusual for {} field `{}`",
														operator, type_str, base_field_name
													),
													..Default::default()
												});
											}
										}
									}
								}
							}
						}
					}
				}
			}
		}

		// Validate value type for 'in' and 'not in' operators
		if domain::LIST_VALUE_OPERATORS.contains(domain::normalize_operator(&operator.to_lowercase())) {
			if let Some(value_node) = value_node {
				// 'in' and 'not in' require list/tuple values
				if !matches!(value_node.kind(), "list" | "tuple" | "parenthesized_expression") {
					// Check if it's a parenthesized list/tuple
					let is_paren_list = value_node.kind() == "parenthesized_expression"
						&& value_node.named_child(0).map(|c| matches!(c.kind(), "list" | "tuple")).unwrap_or(false);
					
					if !is_paren_list {
						diagnostics.push(Diagnostic {
							range: rope_conv(value_node.byte_range().map_unit(ByteOffset), rope),
							severity: Some(DiagnosticSeverity::ERROR),
							message: format!(
								"Operator `{}` requires a list or tuple value, got {}",
								operator,
								value_node.kind()
							),
							..Default::default()
						});
					}
				}
			}
		}

		// Handle subdomain operators ('any', 'not any')
		if domain::is_subdomain_operator(operator) {
			let Some(value_node) = value_node else {
				return;
			};

			// For subdomain operators, the value must be a list (the subdomain)
			if value_node.kind() != "list" {
				diagnostics.push(Diagnostic {
					range: rope_conv(value_node.byte_range().map_unit(ByteOffset), rope),
					severity: Some(DiagnosticSeverity::ERROR),
					message: format!(
						"Operator `{}` requires a domain list as value, got {}",
						operator,
						value_node.kind()
					),
					..Default::default()
				});
				return;
			}

			// Get the field name to resolve its comodel
			let Some(field_node) = field_node else {
				return;
			};

			if field_node.kind() != "string" {
				return;
			}

			let field_range = field_node.byte_range().shrink(1);
			let field_name = &contents[field_range.clone()];

			// Extract the base field name (first part before any dots)
			let base_field_name = field_name.split('.').next().unwrap_or(field_name);

			// Resolve the field's comodel
			if let Some(sub_comodel) = self.index.models.get_field_comodel(comodel_name, base_field_name) {
				let sub_comodel_name = _R(sub_comodel);
				// Recurse into the subdomain with the resolved comodel
				self.diagnose_domain(
					rope,
					diagnostics,
					contents,
					root,
					sub_comodel_name,
					value_node,
					match_,
					depth + 1,
					max_depth,
				);
			} else {
				// Field is not relational or doesn't exist
				// The field validation in diagnose_mapped will catch non-existent fields,
				// so we only need to check if it's non-relational
				if let Some(model_key) = _G(comodel_name) {
					if let Some(entry) = self.index.models.populate_properties(model_key.into(), &[]) {
						if let Some(fields) = entry.fields.as_ref() {
							if let Some(field_key) = _G(base_field_name) {
								if fields.contains_key(&field_key) {
									// Field exists but is not relational
									diagnostics.push(Diagnostic {
										range: rope_conv(operator_node_range.clone().map_unit(ByteOffset), rope),
										severity: Some(DiagnosticSeverity::ERROR),
										message: format!(
											"Operator `{}` can only be used with relational fields (Many2one, One2many, Many2many), but `{}` is not relational",
											operator,
											base_field_name
										),
										..Default::default()
									});
								}
							}
						}
					}
				}
			}
		}
	}

	fn diagnose_manifest_dependencies(&self, diagnostics: &mut Vec<Diagnostic>, contents: &str, root: Node) {
		use ts_macros::query;

		query! {
			ManifestDepsQuery(Dependency);

			((dictionary
				(pair
					(string (string_content) @_depends)
					(list
						(string) @DEPENDENCY
					)
				)
			) (#eq? @_depends "depends"))
		}

		// Get all available modules
		let all_available_modules = self.index.get_all_available_modules();

		let mut cursor = QueryCursor::new();
		let mut captures = cursor.captures(ManifestDepsQuery::query(), root, contents.as_bytes());

		while let Some((match_, idx)) = captures.next() {
			let capture = match_.captures[*idx];
			match ManifestDepsQuery::from(capture.index) {
				Some(ManifestDepsQuery::Dependency) => {
					let dep_node = capture.node;
					// Get the string content without quotes
					let dep_range = dep_node.byte_range();
					let dep_with_quotes = &contents[dep_range.clone()];

					// Skip if not a proper string
					if !dep_with_quotes.starts_with('"') && !dep_with_quotes.starts_with('\'') {
						continue;
					}

					// Extract the dependency name without quotes
					let dep_name = &contents[dep_range.shrink(1)];
					let dep_symbol = _I(dep_name);

					// Check if the dependency is available
					if !all_available_modules.contains(&dep_symbol) {
						// Adjust the range to start after the opening quote
						let mut range = dep_node.range();
						range.start_point.column += 2; // Skip quote and following space
						range.end_point.column -= 1;

						diagnostics.push(Diagnostic {
							range: span_conv(range),
							severity: Some(DiagnosticSeverity::ERROR),
							message: format!("Module '{dep_name}' is not available in your path"),
							..Default::default()
						});
					}
				}
				None => {}
			}
		}
	}

	/// Build related information for auto_install module diagnostics
	fn build_auto_install_related_info(
		&self,
		module_name: crate::index::ModuleName,
		_missing_deps_with_chains: &[(crate::index::ModuleName, Vec<crate::index::ModuleName>)],
		model_name: ModelName,
		attr_name: &str,
		current_path: &str,
	) -> Option<Vec<DiagnosticRelatedInformation>> {
		let mut related_info = Vec::new();

		// Try to find where the property is defined
		// First check if property is already in the index (for loaded modules)
		let mut property_found = false;
		if let Some(model_entry) = self.index.models.get(&model_name) {
			// Check fields
			if let Some(fields) = model_entry.fields.as_ref()
				&& let Some(field) = fields.get(&_I(attr_name))
				&& let Some(field_module) = self.index.find_module_of(&field.location.path.to_path())
				&& field_module == module_name
				&& let Some(uri) = Uri::from_file_path(field.location.path.to_path())
			{
				related_info.push(DiagnosticRelatedInformation {
					location: Location {
						uri,
						range: field.location.range,
					},
					message: format!("This field is defined in `{}`", _R(module_name)),
				});
				property_found = true;
			}

			// Check methods if field not found
			if !property_found
				&& let Some(methods) = model_entry.methods.as_ref()
				&& let Some(method) = methods.get(&_I(attr_name))
				&& let Some(loc) = method.locations.first()
				&& let Some(method_module) = self.index.find_module_of(&loc.path.to_path())
				&& method_module == module_name
				&& let Some(uri) = Uri::from_file_path(loc.path.to_path())
			{
				related_info.push(DiagnosticRelatedInformation {
					location: Location { uri, range: loc.range },
					message: format!("This method is defined in `{}`", _R(module_name)),
				});
				property_found = true;
			}
		}

		// If property not found in index, point to the module's models directory
		if !property_found && let Some(location) = self.get_module_models_location(module_name) {
			related_info.push(DiagnosticRelatedInformation {
				location,
				message: format!("This property is defined in `{}`", _R(module_name)),
			});
		}

		// Find current module's manifest to suggest adding dependency
		if let Some(location) = self.find_manifest_depends_location(current_path) {
			related_info.push(DiagnosticRelatedInformation {
				location,
				message: format!(
					"To expose this property, depend directly on `{}` or all of its reverse dependencies",
					_R(module_name)
				),
			});
		}

		// Find where auto_install is defined
		if let Some(location) = self.find_auto_install_location(module_name) {
			related_info.push(DiagnosticRelatedInformation {
				location,
				message: format!(
					"`{}` is defined as a bridge module here, alongside its reverse dependencies",
					_R(module_name)
				),
			});
		}

		debug!("Total related information entries: {}", related_info.len());
		for (i, info) in related_info.iter().enumerate() {
			debug!("  {}. {}", i + 1, info.message);
		}

		(!related_info.is_empty()).then_some(related_info)
	}

	/// Get the location of a module's models directory or main file
	fn get_module_models_location(&self, module_name: crate::index::ModuleName) -> Option<Location> {
		// Find the module in the index
		for root_entry in self.index.roots.iter() {
			let (root_path, modules) = root_entry.pair();
			if let Some(module_entry) = modules.get(&module_name) {
				let module_path = root_path.join(module_entry.path.as_str());

				// Try models/__init__.py first
				let models_init = module_path.join("models").join("__init__.py");
				if models_init.exists()
					&& let Some(uri) = Uri::from_file_path(&models_init)
				{
					return Some(Location {
						uri,
						range: Default::default(), // Point to start of file
					});
				}

				// Fallback to module directory
				if let Some(uri) = Uri::from_file_path(&module_path) {
					return Some(Location {
						uri,
						range: Default::default(),
					});
				}
			}
		}
		None
	}

	/// Find the depends location in the current module's manifest
	fn find_manifest_depends_location(&self, current_path: &str) -> Option<Location> {
		use ts_macros::query;
		tracing::warn!("find_manifest_depends_location called with path: {}", current_path);

		// Define a simple query for finding the depends list
		query! {
			DependsListQuery(DependsList);

			((dictionary
				(pair
					(string (string_content) @_depends)
					(list) @DEPENDS_LIST
				)
			) (#eq? @_depends "depends"))
		}

		let path_buf = std::path::PathBuf::from(current_path);
		if let Some(current_module) = self.index.find_module_of(&path_buf) {
			tracing::warn!("Found module: {:?}", current_module);
			// Find the module's manifest
			for root_entry in self.index.roots.iter() {
				let (root_path, modules) = root_entry.pair();
				if let Some(module_entry) = modules.get(&current_module) {
					let mut manifest_path = root_path.clone();
					manifest_path.push(module_entry.path.as_str());
					manifest_path.push("__manifest__.py");
					tracing::warn!("Manifest path: {:?}, exists: {}", manifest_path, manifest_path.exists());

					if let Ok(contents) = crate::test_utils::fs::read_to_string(&manifest_path) {
						let uri = Uri::from_file_path(&manifest_path).unwrap();
						// Try to parse and find depends
						let mut parser = python_parser();
						if let Some(ast) = parser.parse(&contents, None) {
							let mut cursor = QueryCursor::new();
							let mut captures =
								cursor.captures(DependsListQuery::query(), ast.root_node(), contents.as_bytes());

							if let Some((match_, idx)) = captures.next() {
								let capture = match_.captures[*idx];
								if let Some(DependsListQuery::DependsList) = DependsListQuery::from(capture.index) {
									return Some(Location {
										uri,
										range: span_conv(capture.node.range()),
									});
								}
							}
						}

						// If no depends found or parsing failed, just point to beginning of manifest
						return Some(Location {
							uri,
							range: Default::default(), // Points to start of file
						});
					}
					break;
				}
			}
		}
		None
	}

	/// Find where auto_install is defined in a module's manifest
	fn find_auto_install_location(&self, module_name: crate::index::ModuleName) -> Option<Location> {
		use ts_macros::query;

		// Define a query that handles Python's True (capital T) which is parsed as identifier
		query! {
			AutoInstallQuery(AutoInstallValue);

			((dictionary
				(pair
					(string (string_content) @_auto_install)
					[(true) (identifier)] @AUTO_INSTALL_VALUE
				)
			) (#eq? @_auto_install "auto_install"))
		}

		// Find the module's manifest
		for root_entry in self.index.roots.iter() {
			let (root_path, modules) = root_entry.pair();
			if let Some(module_entry) = modules.get(&module_name) {
				let mut manifest_path = root_path.clone();
				manifest_path.push(module_entry.path.as_str());
				manifest_path.push("__manifest__.py");

				if let Ok(contents) = crate::test_utils::fs::read_to_string(&manifest_path) {
					let uri = Uri::from_file_path(&manifest_path).unwrap();
					let mut parser = python_parser();
					if let Some(ast) = parser.parse(&contents, None) {
						let mut cursor = QueryCursor::new();
						let mut captures =
							cursor.captures(AutoInstallQuery::query(), ast.root_node(), contents.as_bytes());

						if let Some((match_, _)) = captures.next() {
							// Find the value capture
							for capture in match_.captures {
								if let Some(AutoInstallQuery::AutoInstallValue) = AutoInstallQuery::from(capture.index)
								{
									// Return the location of the value (True/true/1/"true"/etc)
									return Some(Location {
										uri,
										range: span_conv(capture.node.range()),
									});
								}
							}
						}
					}
				}
				break;
			}
		}
		None
	}

	/// Diagnose missing super() calls in overridden methods.
	/// Warns when a method overrides a parent method but doesn't call super().
	pub fn diagnose_missing_super(
		&self,
		path_sym: PathSymbol,
		diagnostics: &mut Vec<Diagnostic>,
	) {
		// Track where new diagnostics start for sorting later
		let start_idx = diagnostics.len();

		// Get all models defined in this file
		let models_in_file = self.index.models.models_in_file(&path_sym);

		for (model_name, is_inheriting) in models_in_file {
			// Only check models that are inheriting (descendants)
			if !is_inheriting {
				continue;
			}

			// Ensure properties are populated. Returns None if model not found, which is ok.
			let _ = self.index.models.populate_properties(model_name, &[]);

			let Some(entry) = self.index.models.get(&model_name) else {
				continue;
			};
			
			let Some(methods) = &entry.methods else {
				continue;
			};

			for (method_sym, method) in methods.iter() {
				let method_name = _R(*method_sym);

				// For each location of this method in the current file,
				// check if it's a descendant (override) and if there's an earlier definition
				for loc in &method.locations {
					if loc.path != path_sym || !loc.active {
						continue;
					}

					// Find if there's a "parent" location (base or earlier descendant) for this method
					// The parent is any location that isn't in the current file
					let parent_loc = method.locations.iter().find(|other_loc| {
						other_loc.path != path_sym && other_loc.active
					});

					let Some(parent_loc) = parent_loc else {
						// No parent location found - this method is not an override
						continue;
					};

					if !loc.calls_super {
						// Build related information showing parent method location
						let related_info = Uri::from_file_path(parent_loc.path.to_path())
							.map(|uri| {
								vec![DiagnosticRelatedInformation {
									location: Location {
										uri,
										range: parent_loc.range,
									},
									message: format!(
										"Parent method defined in `{}`",
										_R(model_name)
									),
								}]
							});

						diagnostics.push(Diagnostic {
							range: loc.range,
							severity: Some(DiagnosticSeverity::WARNING),
							message: format!(
								"Method `{method_name}` overrides parent but does not call `super().{method_name}()`"
							),
							related_information: related_info,
							..Default::default()
						});
					}
				}
			}
		}

		// Sort newly added diagnostics by position for deterministic ordering
		diagnostics[start_idx..].sort_by_key(|d| (d.range.start.line, d.range.start.character));
	}

	/// Diagnose controller routes for issues like invalid type, auth, duplicate routes, etc.
	fn diagnose_controller_routes(
		&self,
		rope: RopeSlice<'_>,
		diagnostics: &mut Vec<Diagnostic>,
		contents: &str,
		root: Node,
		_module: crate::index::ModuleName,
	) {
		use crate::index::{RouteQuery, controller::{AuthType, RouteType}};

		let query = RouteQuery::query();
		let mut cursor = QueryCursor::new();
		let mut matches = cursor.matches(query, root, contents.as_bytes());

		// Track routes we've seen in this file for duplicate detection
		let mut seen_routes: std::collections::HashMap<String, Range> = std::collections::HashMap::new();

		while let Some(match_) = matches.next() {
			// Extract route decorator arguments
			let Some(route_args_node) = match_
				.nodes_for_capture_index(RouteQuery::RouteArgs as _)
				.next()
			else {
				continue;
			};

			// Extract method name node for location reference
			let method_name_node = match_
				.nodes_for_capture_index(RouteQuery::MethodName as _)
				.next();

			// Parse route arguments
			let mut cursor_inner = route_args_node.walk();
			for child in route_args_node.named_children(&mut cursor_inner) {
				match child.kind() {
					"keyword_argument" => {
						if let Some(key_node) = child.child_by_field_name("name") {
							let key = &contents[key_node.byte_range()];
							if let Some(value_node) = child.child_by_field_name("value") {
								match key {
									"type" => {
										// Validate route type
										if value_node.kind() == "string" {
											let value_range = value_node.byte_range();
											if value_range.len() >= 2 {
												let value = &contents[value_range.clone().shrink(1)];
												if value.parse::<RouteType>().is_err() {
													diagnostics.push(Diagnostic {
														range: rope_conv(value_range.map_unit(ByteOffset), rope),
														severity: Some(DiagnosticSeverity::ERROR),
														message: format!(
															"Invalid route type '{}'. Valid types: {}",
															value,
															RouteType::all_values().join(", ")
														),
														..Default::default()
													});
												}
											}
										}
									}
									"auth" => {
										// Validate auth type
										if value_node.kind() == "string" {
											let value_range = value_node.byte_range();
											if value_range.len() >= 2 {
												let value = &contents[value_range.clone().shrink(1)];
												if value.parse::<AuthType>().is_err() {
													diagnostics.push(Diagnostic {
														range: rope_conv(value_range.map_unit(ByteOffset), rope),
														severity: Some(DiagnosticSeverity::ERROR),
														message: format!(
															"Invalid auth type '{}'. Valid types: {}",
															value,
															AuthType::all_values().join(", ")
														),
														..Default::default()
													});
												}
											}
										}
									}
									_ => {}
								}
							}
						}
					}
					"string" => {
						// First positional argument - route path
						let path_range = child.byte_range();
						if path_range.len() >= 2 {
							let path = contents[path_range.clone().shrink(1)].to_string();
							if !path.is_empty() {
								let range: Range = rope_conv(path_range.map_unit(ByteOffset), rope);
								// Check for duplicate routes in the same file
								if seen_routes.contains_key(&path) {
									diagnostics.push(Diagnostic {
										range,
										severity: Some(DiagnosticSeverity::WARNING),
										message: format!("Duplicate route '{}' in this module", path),
										..Default::default()
									});
								} else {
									seen_routes.insert(path.clone(), range);
								}

								// Validate model converters in the path
								self.validate_model_converters_in_path(
									diagnostics,
									&path,
									range,
								);
							}
						}
					}
					"list" => {
						// Multiple paths - check each one
						let mut list_cursor = child.walk();
						for item in child.named_children(&mut list_cursor) {
							if item.kind() == "string" {
								let path_range = item.byte_range();
								if path_range.len() >= 2 {
									let path = contents[path_range.clone().shrink(1)].to_string();
									if !path.is_empty() {
										// Check for duplicates
										if seen_routes.contains_key(&path) {
											diagnostics.push(Diagnostic {
												range: rope_conv(path_range.clone().map_unit(ByteOffset), rope),
												severity: Some(DiagnosticSeverity::WARNING),
												message: format!("Duplicate route '{}' in this module", path),
												..Default::default()
											});
										} else {
											seen_routes.insert(path.clone(), rope_conv(path_range.clone().map_unit(ByteOffset), rope));
										}

										// Validate model converters
										self.validate_model_converters_in_path(
											diagnostics,
											&path,
											rope_conv(path_range.map_unit(ByteOffset), rope),
										);
									}
								}
							}
						}
					}
					_ => {}
				}
			}

			// Check if URL parameters match method parameters
			if let Some(method_params_node) = match_
				.nodes_for_capture_index(RouteQuery::MethodParams as _)
				.next()
			{
				// Collect method parameter names (excluding 'self')
				let mut method_params = std::collections::HashSet::new();
				let mut params_cursor = method_params_node.walk();
				for child in method_params_node.named_children(&mut params_cursor) {
					let name = match child.kind() {
						"identifier" => {
							let n = &contents[child.byte_range()];
							if n != "self" { Some(n.to_string()) } else { None }
						}
						"default_parameter" | "typed_parameter" | "typed_default_parameter" => {
							child.named_child(0).map(|n| {
								let name = &contents[n.byte_range()];
								if name != "self" { Some(name.to_string()) } else { None }
							}).flatten()
						}
						"list_splat_pattern" | "dictionary_splat_pattern" => {
							// *args or **kwargs - these can accept any parameter
							None
						}
						_ => None,
					};
					if let Some(n) = name {
						method_params.insert(n);
					}
				}

				// Check if we have *args or **kwargs (which can accept any URL params)
				let has_varargs = method_params_node.named_children(&mut params_cursor)
					.any(|c| matches!(c.kind(), "list_splat_pattern" | "dictionary_splat_pattern"));

				if !has_varargs {
					// Extract URL parameters from the route paths and check they're in method params
					for child in route_args_node.named_children(&mut route_args_node.walk()) {
						if child.kind() == "string" {
							let path_range = child.byte_range();
							if path_range.len() >= 2 {
								let path = &contents[path_range.clone().shrink(1)];
								let url_params = crate::index::controller::parse_url_params(path);
								
								for param in url_params {
									let param_name: &str = param.name.as_ref();
									if !method_params.contains(param_name) {
										// URL parameter not in method signature
										// We report on the method name for clarity
										if let Some(method_node) = method_name_node {
											diagnostics.push(Diagnostic {
												range: rope_conv(method_node.byte_range().map_unit(ByteOffset), rope),
												severity: Some(DiagnosticSeverity::WARNING),
												message: format!(
													"URL parameter '{}' is not in method signature",
													param_name
												),
												..Default::default()
											});
										}
									}
								}
							}
						}
					}
				}
			}
		}
	}

	/// Validate model converters in a route path.
	/// 
	/// Checks that models referenced in `<model("model.name"):var>` converters exist.
	fn validate_model_converters_in_path(
		&self,
		diagnostics: &mut Vec<Diagnostic>,
		path: &str,
		path_range: Range,
	) {
		use crate::index::controller::{parse_url_params, RouteConverter};

		let params = parse_url_params(path);
		for param in params {
			match &param.converter {
				RouteConverter::Model(model_name) | RouteConverter::Models(model_name) => {
					let model_str: &str = model_name.as_ref();
					if !model_str.is_empty() {
						// Use the same pattern as other model validation in this file:
						// _G returns None if the string was never interned (definitely unknown)
						// If interned, check if the model actually exists in the index
						let model_key = _G(model_str);
						let has_model = model_key.map(|key| self.index.models.contains_key(&key));
						if !has_model.unwrap_or(false) {
							diagnostics.push(Diagnostic {
								range: path_range,
								severity: Some(DiagnosticSeverity::ERROR),
								message: format!(
									"Unknown model '{}' in route converter",
									model_str
								),
								..Default::default()
							});
						}
					}
				}
				_ => {}
			}
		}
	}
}
