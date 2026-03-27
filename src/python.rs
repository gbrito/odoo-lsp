use std::borrow::Cow;
use std::ops::Deref;
use std::path::Path;

use lasso::Spur;
use ropey::Rope;
use tower_lsp_server::ls_types::*;
use tracing::{debug, instrument, trace, warn};
use tree_sitter::{Node, QueryCapture, QueryMatch};
use ts_macros::query;

use crate::prelude::*;

use crate::analyze::{Type, type_cache};
use crate::index::{_G, _I, _R, PathSymbol, index_models};
use crate::model::{ModelName, ModelType};
use crate::xml::determine_csv_xmlid_subgroup;
use crate::{backend::Backend, backend::Text};

use std::collections::HashMap;

mod completions;
mod diagnostics;
mod inlay_hints;
mod semantic_tokens;
mod symbols;

#[cfg(test)]
mod tests;

#[rustfmt::skip]
query! {
	PyCompletions(Request, XmlId, Mapped, MappedTarget, Depends, ReadFn, Model, Prop, ForXmlId, Scope, FieldDescriptor, FieldType, HasGroups);

(call [
  (attribute [
    (identifier) @_env
    (attribute (_) (identifier) @_env)] (identifier) @_ref)
  (attribute
    (identifier) @REQUEST (identifier) @_render)
  (attribute
    (_) (identifier) @FOR_XML_ID)
  (attribute
  	(_) (identifier) @HAS_GROUPS) ]
  (argument_list . (string) @XML_ID)
  (#eq? @_env "env")
  (#eq? @_ref "ref")
  (#eq? @REQUEST "request")
  (#eq? @_render "render")
  (#eq? @FOR_XML_ID "_for_xml_id")
  (#match? @HAS_GROUPS "^(user_has_groups|has_group)$")
)

(subscript [
  (identifier) @_env
  (attribute (_) (identifier) @_env)]
  (string) @MODEL
  (#eq? @_env "env"))

((class_definition
  (block
    (expression_statement
      (assignment
        (identifier) @PROP [
        (string) @MODEL
        (list ((string) @MODEL ","?)*)
        (call
          (attribute
            (identifier) @_fields (identifier) @FIELD_TYPE (#eq? @_fields "fields"))
          (argument_list
            . [
              ((comment)+ (string) @MODEL)
              (string) @MODEL ]?
            // handles `related` `compute` `search` and `inverse`
            ((keyword_argument (identifier) @FIELD_DESCRIPTOR (_)) ","?)*)) ])))))

(call [
  (attribute
    (_) @MAPPED_TARGET (identifier) @_mapper)
  (attribute
    (identifier) @_api (identifier) @DEPENDS)]
  (argument_list (string) @MAPPED)
  (#match? @_mapper "^(mapp|filter|sort|group)ed$")
  (#eq? @_api "api")
  (#match? @DEPENDS "^(depends|constrains|onchange)$"))

((call
  (attribute
    (_) @MAPPED_TARGET (identifier) @_search)
  (argument_list [
    (list [
      (tuple . (string) @MAPPED)
      (parenthesized_expression (string) @MAPPED)])
    (keyword_argument
      (identifier) @_domain
      (list [
        (tuple . (string) @MAPPED)
        (parenthesized_expression (string) @MAPPED)]))]))
  (#eq? @_domain "domain")
  (#match? @_search "^(search(_(read|count))?|_?read_group|filtered_domain|_where_calc)$"))

((call
  (attribute
    (_) @MAPPED_TARGET (identifier) @READ_FN)
  (argument_list [
    (list (string) @MAPPED)
    (keyword_argument
      (identifier) @_domain
      (list (string) @MAPPED)) ]))
  (#match? @_domain "^(groupby|aggregates)$")
  (#match? @READ_FN "^(_?read(_group)?|flush_model)$"))

((call
  (attribute
    (_) @MAPPED_TARGET (identifier) @DEPENDS)
  (argument_list . [
    (set (string) @MAPPED)
    (dictionary [
      (pair key: (string) @MAPPED)
      (ERROR (string) @MAPPED)
      (ERROR) @MAPPED ])
    (_ [
      (set (string) @MAPPED)
      (dictionary [
        (pair key: (string) @MAPPED)
        (ERROR (string) @MAPPED) ]) ]) ]))
  (#match? @DEPENDS "^(create|write|copy)$"))

((class_definition
  (block [
    (function_definition) @SCOPE
    (decorated_definition
      (decorator
        (call
          (attribute (identifier) @_api (identifier) @_depends)
          (argument_list ((string) @_ ","?)*)))
      (function_definition) @SCOPE) ]))
  (#eq? @_api "api")
  (#eq? @_depends "depends"))

(class_definition
  (block
    (decorated_definition
      (decorator (_) @_)
      (function_definition) @SCOPE)*)
  (#not-match? @_ "^api.depends"))
}

#[rustfmt::skip]
query! {
	PyImports(ImportModule, ImportName, ImportAlias);

(import_from_statement
  module_name: (dotted_name) @IMPORT_MODULE
  name: (dotted_name) @IMPORT_NAME)

(import_from_statement
  module_name: (dotted_name) @IMPORT_MODULE
  name: (aliased_import
    name: (dotted_name) @IMPORT_NAME
    alias: (identifier) @IMPORT_ALIAS))

(import_statement
  name: (dotted_name) @IMPORT_NAME)

(import_statement
  name: (aliased_import
    name: (dotted_name) @IMPORT_NAME
    alias: (identifier) @IMPORT_ALIAS))
}

#[rustfmt::skip]
query! {
	ModuleFunction(FuncName, FuncBody);

(module
  (function_definition
    name: (identifier) @FUNC_NAME) @FUNC_BODY)

(module
  (decorated_definition
    (function_definition
      name: (identifier) @FUNC_NAME)) @FUNC_BODY)
}

/// (module (_)*)
pub(crate) fn top_level_stmt(module: Node, offset: usize) -> Option<Node> {
	module
		.named_children(&mut module.walk())
		.find(|child| child.byte_range().contains_end(offset))
}

/// Get the top-level statement (direct child of module) containing the given node.
fn top_level_stmt_of_node(node: Node) -> Option<Node> {
	let mut current = node;
	while let Some(parent) = current.parent() {
		if parent.kind() == "module" {
			return Some(current);
		}
		current = parent;
	}
	None
}

/// Recursively searches for a class definition with the given name in the AST.
fn find_class_definition<'a>(
	node: tree_sitter::Node<'a>,
	contents: &str,
	class_name: &str,
) -> Option<tree_sitter::Node<'a>> {
	use crate::utils::PreTravel;

	PreTravel::new(node)
		.find(|node| {
			node.kind() == "class_definition"
				&& node
					.child_by_field_name("name")
					.map(|name_node| class_name == &contents[name_node.byte_range()])
					.unwrap_or(false)
		})
		.and_then(|node| node.child_by_field_name("name"))
}

#[derive(Debug)]
struct Mapped<'text> {
	needle: &'text str,
	model: &'text str,
	single_field: bool,
	range: ByteRange,
}

#[derive(Debug, Clone)]
struct ImportInfo {
	module_path: String,
	imported_name: String,
	alias: Option<String>,
}

type ImportMap = HashMap<String, ImportInfo>;

/// Python extensions.
impl Backend {
	/// Helper function to resolve import-based jump-to-definition requests.
	/// Returns the location if successful, None if not found, or an error if resolution fails.
	fn resolve_import_location(&self, imports: &ImportMap, identifier: &str) -> anyhow::Result<Option<Location>> {
		let Some(import_info) = imports.get(identifier) else {
			return Ok(None);
		};

		// Enhanced debugging with alias information
		if let Some(alias) = &import_info.alias {
			debug!(
				"Found aliased import '{}' -> '{}' from module '{}'",
				alias, import_info.imported_name, import_info.module_path
			);
		} else {
			debug!(
				"Found direct import '{}' from module '{}'",
				import_info.imported_name, import_info.module_path
			);
		}

		let Some(file_path) = self.index.resolve_py_module(&import_info.module_path) else {
			debug!("Failed to resolve module path: {}", import_info.module_path);
			return Ok(None);
		};

		debug!("Resolved file path: {}", file_path.display());

		let target_contents = ok!(
			test_utils::fs::read_to_string(&file_path),
			"Failed to read target file {}",
			file_path.display(),
		);

		let class_name = &import_info.imported_name;
		if let Some(alias) = &import_info.alias {
			debug!(
				"Looking for original class '{}' (aliased as '{}') in target file",
				class_name, alias
			);
		} else {
			debug!("Looking for class '{}' in target file", class_name);
		}

		let mut target_parser = python_parser();

		let Some(target_ast) = target_parser.parse(&target_contents, None) else {
			debug!("Failed to parse target file with tree-sitter");
			return Ok(Some(Location {
				uri: path_to_uri(file_path)?,
				range: Range::new(Position::new(0, 0), Position::new(0, 0)),
			}));
		};

		if let Some(class_node) = find_class_definition(target_ast.root_node(), &target_contents, class_name) {
			let range = class_node.range();
			if let Some(alias) = &import_info.alias {
				debug!(
					"Found class '{}' (aliased as '{}') at line {}, col {}",
					class_name, alias, range.start_point.row, range.start_point.column
				);
			} else {
				debug!(
					"Found class '{}' at line {}, col {}",
					class_name, range.start_point.row, range.start_point.column
				);
			}
			return Ok(Some(Location {
				uri: path_to_uri(file_path)?,
				range: span_conv(range),
			}));
		}

		if let Some(alias) = &import_info.alias {
			debug!(
				"Class '{}' (aliased as '{}') not found in target file using tree-sitter",
				class_name, alias
			);
		} else {
			debug!("Class '{}' not found in target file using tree-sitter", class_name);
		}
		Ok(Some(Location {
			uri: path_to_uri(file_path)?,
			range: Range::new(Position::new(0, 0), Position::new(0, 0)),
		}))
	}

	#[tracing::instrument(skip_all, ret, fields(uri))]
	pub fn on_change_python(
		&self,
		text: &Text,
		uri: &Uri,
		rope: RopeSlice<'_>,
		old_rope: Option<Rope>,
	) -> anyhow::Result<()> {
		let parser = python_parser();
		self.update_ast(text, uri, rope, old_rope, parser)
	}

	/// Parse import statements from Python content and return a map of imported names to their module paths
	fn parse_imports(&self, contents: &str) -> anyhow::Result<ImportMap> {
		let mut parser = python_parser();
		let ast = parser
			.parse(contents, None)
			.ok_or_else(|| errloc!("Failed to parse Python AST"))?;
		let query = PyImports::query();
		let mut cursor = tree_sitter::QueryCursor::new();
		let mut imports = ImportMap::new();

		debug!("Parsing imports from {} bytes", contents.len());

		let mut matches = cursor.matches(query, ast.root_node(), contents.as_bytes());
		while let Some(match_) = matches.next() {
			let mut module_path = None;
			let mut import_name = None;
			let mut alias = None;

			debug!("Found import match with {} captures", match_.captures.len());

			for capture in match_.captures {
				let capture_text = &contents[capture.node.byte_range()];
				debug!("Capture {}: = '{}'", capture.index, capture_text);

				match PyImports::from(capture.index) {
					Some(PyImports::ImportModule) => {
						module_path = Some(capture_text.to_string());
					}
					Some(PyImports::ImportName) => {
						import_name = Some(capture_text.to_string());
					}
					Some(PyImports::ImportAlias) => {
						alias = Some(capture_text.to_string());
					}
					_ => {}
				}
			}

			if let Some(name) = import_name {
				let full_module_path = if let Some(module) = module_path {
					module // For "from module import name", the module path is just the module
				} else {
					name.clone() // For "import name", the module path is the name itself
				};

				let key = alias.as_ref().unwrap_or(&name).clone();
				debug!("Adding import: {} -> {} (from module {})", key, name, full_module_path);
				imports.insert(
					key,
					ImportInfo {
						module_path: full_module_path,
						imported_name: name,
						alias,
					},
				);
			}
		}

		debug!("Final imports map: {:?}", imports);
		Ok(imports)
	}
	pub fn update_models(&self, text: Text, path: &Path, root: Spur, rope: Rope) -> anyhow::Result<()> {
		use crate::index::index_functions;
		use crate::model::Function;

		let text = match text {
			Text::Full(text) => Cow::from(text),
			// TODO: Limit range of possible updates based on delta
			Text::Delta(_) => Cow::from(rope.slice(..)),
		};
		let path_sym = PathSymbol::strip_root(root, path);

		// Clear call graph data for this file before re-indexing
		self.index.call_graph.clear_file(path_sym);

		// Index models
		let models = index_models(text.as_bytes())?;
		self.index.models.append(path_sym, true, &models);
		for model in models {
			match model.type_ {
				ModelType::Base { name, ancestors } => {
					let model_key = _G(&name).unwrap();
					let mut entry = self
						.index
						.models
						.try_get_mut(&model_key)
						.expect(format_loc!("deadlock"))
						.unwrap();
					entry
						.ancestors
						.extend(ancestors.into_iter().map(|sym| ModelName::from(_I(&sym))));
					drop(entry);
					self.index.models.populate_properties(model_key.into(), &[path_sym]);
				}
				ModelType::Inherit(inherits) => {
					let Some(model) = inherits.first() else { continue };
					let model_key = _G(model).unwrap();
					self.index.models.populate_properties(model_key.into(), &[path_sym]);
				}
			}
		}

		// Index module-level functions
		if let Ok(functions) = index_functions(text.as_bytes(), path_sym) {
			self.index.functions.clear_file(&path_sym);
			self.index.functions.append(
				path_sym,
				functions.into_iter().map(|f| {
					let name_sym: Symbol<Function> = _I(&f.name).into();
					(name_sym, f.location)
				}),
			);
		}

		Ok(())
	}
	pub async fn did_save_python(&self, uri: Uri, root: Spur) -> anyhow::Result<()> {
		let path = uri_to_path(&uri)?;
		let zone;
		_ = {
			let mut document = self
				.document_map
				.get_mut(uri.path().as_str())
				.ok_or_else(|| errloc!("(did_save) did not build document"))?;
			zone = document.damage_zone.take();
			let rope = document.rope.clone();
			let text = Cow::from(&document.rope).into_owned();
			self.update_models(Text::Full(text), &path, root, rope)
		}
		.inspect_err(|err| warn!("{err:?}"));
		if zone.is_some() {
			debug!("diagnostics");
			{
				let mut document = self.document_map.get_mut(uri.path().as_str()).unwrap();
				let rope = document.rope.clone();
				let file_path = uri_to_path(&uri)?;
				self.diagnose_python(
					file_path.to_str().unwrap(),
					rope.slice(..),
					zone,
					&mut document.diagnostics_cache,
				);
				let diags = document.diagnostics_cache.clone();
				self.client.publish_diagnostics(uri, diags, None)
			}
			.await;
		}

		Ok(())
	}
	/// Gathers common information regarding a mapped access aka dot access.
	/// Only makes sense in [`PyCompletions`] queries.
	///
	/// `single_field_override` should be set in special cases where this function may not have the necessary context to
	/// determine whether the needle should be processed in mapped mode or single field mode.
	///
	/// Replacing:
	///
	/// ```text
	///     "foo.bar.baz"
	///           ^cursor
	///      -----------range
	///      ------needle
	/// ```
	///
	/// Not replacing:
	///
	/// ```text
	///     "foo.bar.baz"
	///           ^cursor
	///      -------range
	///      -------needle
	/// ```
	#[instrument(level = "trace", skip_all, ret, fields(range_content = &contents[range.clone()]))]
	fn gather_mapped<'text>(
		&self,
		root: Node,
		match_: &tree_sitter::QueryMatch,
		offset: Option<usize>,
		mut range: core::ops::Range<usize>,
		this_model: Option<&'text str>,
		contents: &'text str,
		for_replacing: bool,
		single_field_override: Option<bool>,
	) -> Option<Mapped<'text>> {
		let mut needle = if for_replacing {
			if range.len() < 2 {
				return None;
			}
			range = range.shrink(1);
			let offset = offset.unwrap_or(range.end);
			// If the offset is before the shrunk range start, clamp it to the start.
			// This handles cases where the cursor is at the opening quote of a string.
			let offset = offset.max(range.start);
			&contents[range.start..offset]
		} else {
			let slice = &contents[range.clone().shrink(1)];
			let relative_start = range.start + 1;
			let offset = offset
				.unwrap_or((range.end - 1).max(relative_start + 1))
				.max(relative_start)
				.min(relative_start + slice.len());
			// assert!(
			// 	offset >= relative_start,
			// 	"offset={} cannot be less than relative_start={}",
			// 	offset,
			// 	relative_start
			// );
			let start = offset - relative_start;
			let slice_till_end = slice.get(start..).unwrap_or("");
			// How many characters until the next period or end-of-string?
			let limit = slice_till_end.find('.').unwrap_or(slice_till_end.len());
			range = relative_start..offset + limit;
			// Cow::from(rope.try_slice(range.clone())?)
			&contents[range.clone()]
		};
		if needle == "|" || needle == "&" {
			return None;
		}

		tracing::trace!("(gather_mapped) {} matches={match_:?}", &contents[range.clone()]);

		let model;
		if let Some(local_model) = match_.nodes_for_capture_index(PyCompletions::MappedTarget as _).next() {
			// Try to resolve the model from the target expression
			if let Some(model_) = (self.index).model_of_range(root, local_model.byte_range().map_unit(ByteOffset), contents) {
				model = _R(model_);
			} else if let Some(this_model) = &this_model {
				// Fall back to this_model if we can't resolve the target
				// This handles cases like Command.create({}) where Command is not a model
				model = this_model;
			} else {
				return None;
			}
		} else if let Some(this_model) = &this_model {
			model = this_model
		} else {
			return None;
		}

		let mut single_field = false;
		if let Some(depends) = match_.nodes_for_capture_index(PyCompletions::Depends as _).next() {
			single_field = matches!(
				&contents[depends.byte_range()],
				"write" | "create" | "constrains" | "onchange"
			);
		} else if let Some(read_fn) = match_.nodes_for_capture_index(PyCompletions::ReadFn as _).next() {
			// read or read_group, fields only
			single_field = true;
			if contents[read_fn.byte_range()].ends_with("read_group") {
				// split off aggregate functions
				needle = match needle.split_once(":") {
					None => needle,
					Some((field, _)) => {
						range = range.start..range.start + field.len();
						field
					}
				}
			}
		} else if let Some(override_) = single_field_override {
			single_field = override_;
		}

		Some(Mapped {
			needle,
			model,
			single_field,
			range: range.map_unit(ByteOffset),
		})
	}
	pub fn python_jump_def(
		&self,
		params: GotoDefinitionParams,
		rope: RopeSlice<'_>,
	) -> anyhow::Result<Option<Location>> {
		let uri = &params.text_document_position_params.text_document.uri;
		let file_path = uri_to_path(uri)?;
		let file_path_str = file_path.to_str().unwrap();
		let ast = self
			.ast_map
			.get(file_path_str)
			.ok_or_else(|| errloc!("Did not build AST for {}", file_path_str))?;
		let ByteOffset(offset) = rope_conv(params.text_document_position_params.position, rope);
		let contents = Cow::from(rope);
		let root = some!(top_level_stmt(ast.root_node(), offset));

		// Parse imports from the current file
		let imports = self.parse_imports(&contents).unwrap_or_default();
		debug!("Parsed imports: {:?}", imports);

		// Check if cursor is on an imported identifier
		if let Some(cursor_node) = ast.root_node().descendant_for_byte_range(offset, offset)
			&& cursor_node.kind() == "identifier"
		{
			let identifier = &contents[cursor_node.byte_range()];
			debug!("Checking identifier '{}' at offset {}", identifier, offset);

			// Try to resolve import location
			if let Some(location) = self.resolve_import_location(&imports, identifier /*, contents_bytes*/)? {
				return Ok(Some(location));
			}
		}

		let query = PyCompletions::query();
		let mut cursor = tree_sitter::QueryCursor::new();
		let mut this_model = ThisModel::default();

		let mut matches = cursor.matches(query, ast.root_node(), contents.as_bytes());
		while let Some(match_) = matches.next() {
			for capture in match_.captures {
				let range = capture.node.byte_range();
				match PyCompletions::from(capture.index) {
					Some(PyCompletions::XmlId) if range.contains(&offset) => {
						let range = range.shrink(1);
						let slice = Cow::from(ok!(rope.try_slice(range.clone())));
						let mut slice = slice.as_ref();
						if match_
							.nodes_for_capture_index(PyCompletions::HasGroups as _)
							.next()
							.is_some()
						{
							let mut ref_ = None;
							determine_csv_xmlid_subgroup(&mut ref_, (slice, range.clone()), offset);
							(slice, _) = some!(ref_);
						}
						return self
							.index
							.jump_def_xml_id(slice, &params.text_document_position_params.text_document.uri);
					}
					Some(PyCompletions::Model) => {
						let range = capture.node.byte_range();
						let is_meta = match_
							.nodes_for_capture_index(PyCompletions::Prop as _)
							.next()
							.map(|prop| matches!(&contents[prop.byte_range()], "_name" | "_inherit"))
							.unwrap_or(true);
						if range.contains(&offset) {
							let range = range.shrink(1);
							let slice = ok!(rope.try_slice(range.clone()));
							let slice = Cow::from(slice);
							return self.index.jump_def_model(&slice);
						} else if range.end < offset && is_meta
						// match_
						// 	.nodes_for_capture_index(PyCompletions::FieldType as _)
						// 	.next()
						// 	.is_none()
						{
							let capture_top_level = top_level_stmt_of_node(capture.node)
								.map(|n| n.byte_range())
								.unwrap_or_else(|| root.byte_range());
							this_model.tag_model(capture.node, match_, capture_top_level, &contents);
						}
					}
					Some(PyCompletions::Mapped) => {
						if range.contains_end(offset)
							&& let Some(mapped) = self.gather_mapped(
								root,
								match_,
								Some(offset),
								range.clone(),
								this_model.inner,
								&contents,
								false,
								None,
							) {
							let mut needle = mapped.needle;
							let mut model = _I(mapped.model);
							if !mapped.single_field {
								some!(self.index.models.resolve_mapped(&mut model, &mut needle, None).ok());
							}
							let model = _R(model);
							return self.index.jump_def_property_name(needle, model);
						} else if let Some(cmdlist) = python_next_named_sibling(capture.node)
							&& Backend::is_commandlist(cmdlist, offset)
						{
							let (needle, _, model) = some!(self.gather_commandlist(
								cmdlist,
								root,
								match_,
								offset,
								range,
								this_model.inner,
								&contents,
								false,
							));
							return self.index.jump_def_property_name(needle, _R(model));
						}
					}
					Some(PyCompletions::FieldDescriptor) => {
						let Some(desc_value) = python_next_named_sibling(capture.node) else {
							continue;
						};

						let descriptor = &contents[capture.node.byte_range()];
						if !desc_value.byte_range().contains_end(offset) {
							continue;
						}
						if matches!(descriptor, "comodel_name") {
							let range = desc_value.byte_range().shrink(1);
							let slice = ok!(rope.try_slice(range.clone()));
							let slice = Cow::from(slice);
							return self.index.jump_def_model(&slice);
						} else if matches!(descriptor, "compute" | "search" | "inverse" | "related" | "inverse_name") {
							let single_field = matches!(descriptor, "related" | "inverse_name");
							let mapped_model = if descriptor == "inverse_name" {
								extract_comodel_name(match_.captures, &contents)
									.map(|comodel_name| &contents[comodel_name.byte_range().shrink(1)])
							} else {
								this_model.inner
							};
							// same as PyCompletions::Mapped
							let Some(mapped) = self.gather_mapped(
								root,
								match_,
								Some(offset),
								desc_value.byte_range(),
								mapped_model,
								&contents,
								false,
								Some(single_field),
							) else {
								break;
							};
							let mut needle = mapped.needle;
							let mut model = _I(mapped.model);
							if !mapped.single_field {
								some!(self.index.models.resolve_mapped(&mut model, &mut needle, None).ok());
							}
							let model = _R(model);
							return self.index.jump_def_property_name(needle, model);
						} else if matches!(descriptor, "groups") {
							let range = desc_value.byte_range().shrink(1);
							let value = Cow::from(ok!(rope.try_slice(range.clone())));
							let mut ref_ = None;
							determine_csv_xmlid_subgroup(&mut ref_, (&value, range), offset);
							let (needle, _) = some!(ref_);
							return self.index.jump_def_xml_id(needle, uri);
						} else if matches!(descriptor, "definition") {
							// Go to definition for Properties definition path
							// Format: "many2one_field.properties_definition_field"
							return self.goto_def_properties_definition(
								desc_value,
								offset,
								this_model.inner,
								&contents,
							);
						}

						return Ok(None);
					}
					Some(PyCompletions::Request)
					| Some(PyCompletions::ForXmlId)
					| Some(PyCompletions::HasGroups)
					| Some(PyCompletions::XmlId)
					| Some(PyCompletions::MappedTarget)
					| Some(PyCompletions::Depends)
					| Some(PyCompletions::Prop)
					| Some(PyCompletions::ReadFn)
					| Some(PyCompletions::Scope)
					| Some(PyCompletions::FieldType)
					| None => {}
				}
			}
		}

		// First check if the cursor is on an attribute of Type::Env
		if let Some((lhs, attr, _range)) = Self::attribute_node_at_offset(offset, root, &contents) {
			if let Some((tid, _scope)) =
				self.index.type_of_range(root, lhs.byte_range().map_unit(ByteOffset), &contents)
			{
				if matches!(type_cache().resolve(tid), Type::Env) {
					return self.index.jump_def_env_attribute(attr);
				}
			}
		}

		let (model, prop, _) = some!(self.attribute_at_offset(offset, root, &contents));
		self.index.jump_def_property_name(prop, model)
	}

	/// Go to definition for Properties field `definition` parameter.
	/// The definition path has format: "many2one_field.properties_definition_field"
	/// Based on cursor position, jumps to either:
	/// - The Many2one field definition (if cursor is on the first part)
	/// - The PropertiesDefinition field definition on the comodel (if cursor is after the dot)
	fn goto_def_properties_definition(
		&self,
		desc_value: Node<'_>,
		offset: usize,
		model: Option<&str>,
		contents: &str,
	) -> anyhow::Result<Option<Location>> {
		use crate::model::FieldKind;

		let range = desc_value.byte_range().shrink(1);
		let value = &contents[range.clone()];

		// Parse the definition path: "m2o_field.propdef_field"
		let Some(dot_pos) = value.find('.') else {
			// If there's no dot, we can only try to resolve the field on the current model
			let model_name = some!(model);
			let model_key = some!(_G(model_name));
			let entry = some!(self.index.models.populate_properties(model_key.into(), &[]));
			let fields = some!(entry.fields.as_ref());
			let field_key = some!(_G(value));
			let field = some!(fields.get(&field_key));
			return Ok(Some(field.location.deref().clone().into()));
		};

		let m2o_field = &value[..dot_pos];
		let propdef_field = &value[dot_pos + 1..];

		// Calculate which part the cursor is on
		let relative_offset = offset - range.start;
		let cursor_on_m2o = relative_offset <= dot_pos;

		let model_name = some!(model);
		let model_key = some!(_G(model_name));

		// Get field info and comodel in a block to release the lock before getting comodel entry
		let comodel = {
			let entry = some!(self.index.models.populate_properties(model_key.into(), &[]));
			let fields = some!(entry.fields.as_ref());
			let m2o_field_key = some!(_G(m2o_field));
			let m2o_field_entry = some!(fields.get(&m2o_field_key));

			if cursor_on_m2o {
				// Cursor is on the Many2one field part - jump to its definition
				return Ok(Some(m2o_field_entry.location.deref().clone().into()));
			}

			// Extract the comodel and location before dropping the lock
			let comodel = match &m2o_field_entry.kind {
				FieldKind::Relational(comodel) => *comodel,
				FieldKind::Value | FieldKind::Related(_) => {
					return Ok(None);
				}
			};
			comodel
		};
		// Lock is now released

		// Get the PropertiesDefinition field on the comodel
		let comodel_entry = some!(self.index.models.populate_properties(comodel.into(), &[]));
		let comodel_fields = some!(comodel_entry.fields.as_ref());
		let propdef_field_key = some!(_G(propdef_field));
		let propdef_entry = some!(comodel_fields.get(&propdef_field_key));

		Ok(Some(propdef_entry.location.deref().clone().into()))
	}

	/// Hover for Properties field `definition` parameter.
	/// The definition path has format: "many2one_field.properties_definition_field"
	/// Based on cursor position, shows hover for either:
	/// - The Many2one field (if cursor is on the first part before the dot)
	/// - The PropertiesDefinition field on the comodel (if cursor is after the dot)
	fn hover_properties_definition(
		&self,
		desc_value: Node<'_>,
		offset: usize,
		model: Option<&str>,
		contents: &str,
		rope: RopeSlice<'_>,
	) -> anyhow::Result<Option<Hover>> {
		use crate::model::FieldKind;

		let range = desc_value.byte_range().shrink(1);
		let value = &contents[range.clone()];

		// Parse the definition path: "m2o_field.propdef_field"
		let Some(dot_pos) = value.find('.') else {
			// If there's no dot, try to show hover for the field on the current model
			let model_name = some!(model);
			let lsp_range = span_conv(desc_value.range());
			return self.index.hover_property_name(value, model_name, Some(lsp_range));
		};

		let m2o_field = &value[..dot_pos];
		let propdef_field = &value[dot_pos + 1..];

		// Calculate which part the cursor is on
		let relative_offset = offset - range.start;
		let cursor_on_m2o = relative_offset <= dot_pos;

		let model_name = some!(model);
		let model_key = some!(_G(model_name));

		if cursor_on_m2o {
			// Cursor is on the Many2one field part - show hover for it
			let m2o_range = (range.start..range.start + m2o_field.len()).map_unit(ByteOffset);
			let lsp_range = rope_conv(m2o_range, rope);
			return self.index.hover_property_name(m2o_field, model_name, Some(lsp_range));
		}

		// Cursor is on the PropertiesDefinition field part
		// First, get the comodel from the Many2one field
		let comodel = {
			let entry = some!(self.index.models.populate_properties(model_key.into(), &[]));
			let fields = some!(entry.fields.as_ref());
			let m2o_field_key = some!(_G(m2o_field));
			let m2o_field_entry = some!(fields.get(&m2o_field_key));

			match &m2o_field_entry.kind {
				FieldKind::Relational(comodel) => *comodel,
				FieldKind::Value | FieldKind::Related(_) => {
					return Ok(None);
				}
			}
		};
		// Lock is now released

		// Show hover for the PropertiesDefinition field on the comodel
		let comodel_name = _R(comodel);
		let propdef_range = (range.start + dot_pos + 1..range.end).map_unit(ByteOffset);
		let lsp_range = rope_conv(propdef_range, rope);
		self.index.hover_property_name(propdef_field, comodel_name, Some(lsp_range))
	}

	/// Resolves the attribute and the object's model at the cursor offset
	/// using [`model_of_range`][Index::model_of_range].
	///
	/// Returns `(model, property, range)`.
	fn attribute_at_offset<'out>(
		&'out self,
		offset: usize,
		root: Node<'out>,
		contents: &'out str,
	) -> Option<(&'out str, &'out str, core::ops::Range<usize>)> {
		let (lhs, field, range) = Self::attribute_node_at_offset(offset, root, contents)?;
		let model = (self.index).model_of_range(root, lhs.byte_range().map_unit(ByteOffset), contents)?;
		Some((_R(model), field, range))
	}
	/// Resolves the attribute at the cursor offset.
	/// Returns `(object, field, range)`
	#[instrument(level = "trace", skip_all, ret)]
	pub fn attribute_node_at_offset<'out>(
		mut offset: usize,
		root: Node<'out>,
		contents: &'out str,
	) -> Option<(Node<'out>, &'out str, core::ops::Range<usize>)> {
		if contents.is_empty() {
			return None;
		}
		offset = offset.clamp(0, contents.len() - 1);
		let mut cursor_node = root.descendant_for_byte_range(offset, offset)?;
		let mut real_offset = None;
		if cursor_node.is_named() && !matches!(cursor_node.kind(), "attribute" | "identifier") {
			// We got our cursor left in the middle of nowhere.
			real_offset = Some(offset);
			offset = offset.saturating_sub(1);
			cursor_node = root.descendant_for_byte_range(offset, offset)?;
		}
		trace!(
			"(attribute_node_to_offset) {} cursor={}\n  sexp={}",
			&contents[cursor_node.byte_range()],
			contents.as_bytes()[offset] as char,
			cursor_node.to_sexp(),
		);
		let lhs;
		let rhs;
		if !cursor_node.is_named() {
			// We landed on one of the punctuations inside the attribute.
			// Need to determine which one it is.
			// We cannot depend on prev_named_sibling because the AST may be all messed up
			let idx = contents[..=offset].bytes().rposition(|c| c == b'.')?;
			let ident = contents[..=idx].bytes().rposition(|c| c.is_ascii_alphanumeric())?;
			lhs = root.descendant_for_byte_range(ident, ident)?;
			rhs = python_next_named_sibling(lhs).and_then(|attr| match attr.kind() {
				"identifier" => Some(attr),
				"attribute" => attr.child_by_field_name("attribute"),
				_ => None,
			});
		} else if cursor_node.kind() == "attribute" {
			lhs = cursor_node.child_by_field_name("object")?;
			rhs = cursor_node.child_by_field_name("attribute");
		} else {
			match cursor_node.parent() {
				Some(parent) if parent.kind() == "attribute" => {
					lhs = parent.child_by_field_name("object")?;
					rhs = Some(cursor_node);
				}
				Some(parent) if parent.kind() == "ERROR" => {
					// (ERROR (_) @cursor_node)
					lhs = cursor_node;
					rhs = None;
				}
				_ => return None,
			}
		}
		trace!(
			"(attribute_node_to_offset) lhs={} rhs={:?}",
			&contents[lhs.byte_range()],
			rhs.as_ref().map(|rhs| &contents[rhs.byte_range()]),
		);
		if lhs == cursor_node {
			// We shouldn't recurse into cursor_node itself.
			return None;
		}
		let Some(rhs) = rhs else {
			// In single-expression mode, rhs could be empty in which case
			// we return an empty needle/range.
			let offset = real_offset.unwrap_or(offset);
			return Some((lhs, "", offset..offset));
		};
		let (field, range) = if rhs.range().start_point.row != lhs.range().end_point.row {
			// tree-sitter has an issue with attributes spanning multiple lines
			// which is NOT valid Python, but allows it anyways because tree-sitter's
			// use cases don't require strict syntax trees.
			let offset = real_offset.unwrap_or(offset);
			("", offset..offset)
		} else {
			let range = rhs.byte_range();
			(&contents[range.clone()], range)
		};

		Some((lhs, field, range))
	}
	pub fn python_references(
		&self,
		params: ReferenceParams,
		rope: RopeSlice<'_>,
	) -> anyhow::Result<Option<Vec<Location>>> {
		let ByteOffset(offset) = rope_conv(params.text_document_position.position, rope);
		let uri = &params.text_document_position.text_document.uri;
		let file_path = uri_to_path(uri)?;
		let file_path_str = file_path.to_str().unwrap();
		let ast = self
			.ast_map
			.get(file_path_str)
			.ok_or_else(|| errloc!("Did not build AST for {}", file_path_str))?;
		let root = some!(top_level_stmt(ast.root_node(), offset));
		let query = PyCompletions::query();
		let contents = Cow::from(rope);
		let mut cursor = tree_sitter::QueryCursor::new();
		let path = some!(params.text_document_position.text_document.uri.to_file_path());
		let current_module = self.index.find_module_of(&path);
		let mut this_model = ThisModel::default();

		let mut matches = cursor.matches(query, ast.root_node(), contents.as_bytes());
		while let Some(match_) = matches.next() {
			for capture in match_.captures {
				let range = capture.node.byte_range();
				match PyCompletions::from(capture.index) {
					Some(PyCompletions::XmlId) if range.contains(&offset) => {
						let range = range.shrink(1);
						let slice = Cow::from(ok!(rope.try_slice(range.clone())));
						let mut slice = slice.as_ref();
						if match_
							.nodes_for_capture_index(PyCompletions::HasGroups as _)
							.next()
							.is_some()
						{
							let mut ref_ = None;
							determine_csv_xmlid_subgroup(&mut ref_, (slice, range.clone()), offset);
							(slice, _) = some!(ref_);
						}
						return self.record_references(&path, slice, current_module);
					}
					Some(PyCompletions::Model) => {
						let range = capture.node.byte_range();
						let is_meta = match_
							.nodes_for_capture_index(PyCompletions::Prop as _)
							.next()
							.map(|prop| matches!(&contents[prop.byte_range()], "_name" | "_inherit"))
							.unwrap_or(true);
						if is_meta && range.contains(&offset) {
							let range = range.shrink(1);
							let slice = ok!(rope.try_slice(range.clone()));
							let slice = Cow::from(slice);
							let slice = some!(_G(slice));
							return self.model_references(&path, &slice.into());
						} else if range.end < offset
							&& match_
								.nodes_for_capture_index(PyCompletions::FieldType as _)
								.next()
								.is_none()
						{
							this_model.tag_model(capture.node, match_, root.byte_range(), &contents);
						}
					}
					Some(PyCompletions::FieldDescriptor) => {
						let Some(desc_value) = python_next_named_sibling(capture.node) else {
							continue;
						};
						let descriptor = &contents[range];
						// TODO: related, when field inheritance is implemented
						if !desc_value.byte_range().contains_end(offset) {
							continue;
						};

						if matches!(descriptor, "comodel_name") {
							let range = desc_value.byte_range().shrink(1);
							let slice = ok!(rope.try_slice(range.clone()));
							let slice = Cow::from(slice);
							let slice = some!(_G(slice));
							return self.model_references(&path, &slice.into());
						} else if matches!(descriptor, "compute" | "search" | "inverse") {
							let range = desc_value.byte_range().shrink(1);
							let model = some!(this_model.inner.as_ref());
							let prop = &contents[range];
							return self.index.method_references(prop, model);
						}

						return Ok(None);
					}
					Some(PyCompletions::Request)
					| Some(PyCompletions::XmlId)
					| Some(PyCompletions::ForXmlId)
					| Some(PyCompletions::HasGroups)
					| Some(PyCompletions::Mapped)
					| Some(PyCompletions::MappedTarget)
					| Some(PyCompletions::Depends)
					| Some(PyCompletions::Prop)
					| Some(PyCompletions::ReadFn)
					| Some(PyCompletions::Scope)
					| Some(PyCompletions::FieldType)
					| None => {}
				}
			}
		}

		let (model, prop, _) = some!(self.attribute_at_offset(offset, root, &contents));
		self.index.method_references(prop, model)
	}

	/// Prepares a rename operation by identifying the symbol at the cursor position.
	/// Returns the symbol and its range if it can be renamed, None otherwise.
	pub fn python_prepare_rename(
		&self,
		params: TextDocumentPositionParams,
		rope: RopeSlice<'_>,
	) -> anyhow::Result<Option<(crate::backend::RenameableSymbol, Range)>> {
		use crate::backend::RenameableSymbol;

		let ByteOffset(offset) = rope_conv(params.position, rope);
		let uri = &params.text_document.uri;
		let file_path = uri_to_path(uri)?;
		let file_path_str = file_path.to_str().unwrap();
		let ast = self
			.ast_map
			.get(file_path_str)
			.ok_or_else(|| errloc!("Did not build AST for {}", file_path_str))?;
		// We need to check for a valid top-level statement but don't use the result
		let _ = some!(top_level_stmt(ast.root_node(), offset));
		let query = PyCompletions::query();
		let contents = Cow::from(rope);
		let mut cursor = tree_sitter::QueryCursor::new();
		let current_module = self.index.find_module_of(&file_path);

		let mut matches = cursor.matches(query, ast.root_node(), contents.as_bytes());
		while let Some(match_) = matches.next() {
			for capture in match_.captures {
				let range = capture.node.byte_range();
				match PyCompletions::from(capture.index) {
					Some(PyCompletions::XmlId) if range.contains(&offset) => {
						let inner_range = range.shrink(1);
						let slice = Cow::from(ok!(rope.try_slice(inner_range.clone())));
						let mut slice = slice.as_ref();
						let mut actual_range = inner_range.clone();

						// Handle CSV groups (e.g., groups="base.group_user,base.group_admin")
						if match_
							.nodes_for_capture_index(PyCompletions::HasGroups as _)
							.next()
							.is_some()
						{
							let mut ref_ = None;
							determine_csv_xmlid_subgroup(&mut ref_, (slice, inner_range), offset);
							if let Some((s, r)) = ref_ {
								slice = s;
								actual_range = r;
							} else {
								continue;
							}
						}

						// Convert to qualified ID
						let qualified_id = if slice.contains('.') {
							slice.to_string()
						} else if let Some(module) = current_module {
							format!("{}.{}", _R(module), slice)
						} else {
							slice.to_string()
						};

						let lsp_range = rope_conv(actual_range.map_unit(ByteOffset), rope);
						return Ok(Some((
							RenameableSymbol::XmlId {
								qualified_id,
								current_module,
							},
							lsp_range,
						)));
					}
					Some(PyCompletions::Model) => {
						let is_meta = match_
							.nodes_for_capture_index(PyCompletions::Prop as _)
							.next()
							.map(|prop| matches!(&contents[prop.byte_range()], "_name" | "_inherit"))
							.unwrap_or(true);
						if is_meta && range.contains(&offset) {
							let inner_range = range.shrink(1);
							let slice = ok!(rope.try_slice(inner_range.clone()));
							let slice = Cow::from(slice);
							let model = some!(_G(&slice));
							let lsp_range = rope_conv(inner_range.map_unit(ByteOffset), rope);
							return Ok(Some((RenameableSymbol::ModelName(model.into()), lsp_range)));
						}
					}
					Some(PyCompletions::FieldDescriptor) => {
						let Some(desc_value) = python_next_named_sibling(capture.node) else {
							continue;
						};
						let descriptor = &contents[range];
						if !desc_value.byte_range().contains_end(offset) {
							continue;
						};

						if matches!(descriptor, "comodel_name") {
							let inner_range = desc_value.byte_range().shrink(1);
							let slice = ok!(rope.try_slice(inner_range.clone()));
							let slice = Cow::from(slice);
							let model = some!(_G(&slice));
							let lsp_range = rope_conv(inner_range.map_unit(ByteOffset), rope);
							return Ok(Some((RenameableSymbol::ModelName(model.into()), lsp_range)));
						}
						// compute, search, inverse - method names - not yet supported
						return Ok(None);
					}
					_ => {}
				}
			}
		}

		// No renameable symbol found at cursor
		Ok(None)
	}

	/// Prepares call hierarchy by identifying a method or function at the cursor position.
	/// Returns a list of CallHierarchyItems (usually just one) if the cursor is on a callable.
	pub fn python_prepare_call_hierarchy(
		&self,
		params: CallHierarchyPrepareParams,
		rope: RopeSlice<'_>,
	) -> anyhow::Result<Option<Vec<CallHierarchyItem>>> {
		use crate::backend::CallHierarchyData;
		use crate::call_graph::CallableId;

		let ByteOffset(offset) = rope_conv(params.text_document_position_params.position, rope);
		let uri = &params.text_document_position_params.text_document.uri;
		let file_path = uri_to_path(uri)?;
		let file_path_str = file_path.to_str().unwrap();
		let ast = self
			.ast_map
			.get(file_path_str)
			.ok_or_else(|| errloc!("Did not build AST for {}", file_path_str))?;

		let contents = Cow::from(rope);
		let query = PyCompletions::query();
		let mut cursor = tree_sitter::QueryCursor::new();
		let mut this_model = ThisModel::default();

		// First pass: find model context
		let mut matches = cursor.matches(query, ast.root_node(), contents.as_bytes());
		while let Some(match_) = matches.next() {
			for capture in match_.captures {
				let range = capture.node.byte_range();
				if let Some(PyCompletions::Model) = PyCompletions::from(capture.index) {
					if range.end < offset {
						this_model.tag_model(
							capture.node,
							match_,
							ast.root_node().byte_range(),
							&contents,
						);
					}
				}
			}
		}

		// Check if cursor is on a method definition
		// Look for function_definition nodes
		let root = ast.root_node();
		let mut node_at_cursor = root.descendant_for_byte_range(offset, offset);

		while let Some(node) = node_at_cursor {
			match node.kind() {
				"function_definition" => {
					// Found a function definition - check if it's a method
					let name_node = node.child_by_field_name("name");
					if let Some(name_node) = name_node {
						let name_range = name_node.byte_range();
						// Check if cursor is on or near the function name
						if name_range.contains(&offset) || node.byte_range().start <= offset && offset <= name_range.end + 10 {
							let method_name = &contents[name_range.clone()];
							let lsp_range = span_conv(name_node.range());

							// Check if this is inside a class (making it a method)
							if let Some(model) = this_model.inner {
								let callable = CallableId::method(model, method_name);
								let item = CallHierarchyItem {
									name: method_name.to_string(),
									kind: SymbolKind::METHOD,
									tags: None,
									detail: Some(model.to_string()),
									uri: uri.clone(),
									range: span_conv(node.range()),
									selection_range: lsp_range,
									data: serde_json::to_value(CallHierarchyData { callable }).ok(),
								};
								return Ok(Some(vec![item]));
							} else {
								// Module-level function
								let callable = CallableId::function(file_path_str, method_name);
								let item = CallHierarchyItem {
									name: method_name.to_string(),
									kind: SymbolKind::FUNCTION,
									tags: None,
									detail: Some(file_path_str.to_string()),
									uri: uri.clone(),
									range: span_conv(node.range()),
									selection_range: lsp_range,
									data: serde_json::to_value(CallHierarchyData { callable }).ok(),
								};
								return Ok(Some(vec![item]));
							}
						}
					}
				}
				"call" => {
					// Cursor is on a call expression - try to resolve the callee
					let func_node = node.child_by_field_name("function").or_else(|| node.named_child(0));
					if let Some(func_node) = func_node {
						if func_node.kind() == "attribute" {
							// Method call: obj.method()
							let method_node = func_node.child_by_field_name("attribute");
							if let Some(method_node) = method_node {
								let method_range = method_node.byte_range();
								if method_range.contains(&offset) {
									let method_name = &contents[method_range.clone()];

									// Try to resolve the object's type to get the model
									let obj_node = func_node.child_by_field_name("object");
									if let Some(_obj_node) = obj_node {
									// Use the current model context as fallback
									if let Some(model) = this_model.inner {
										let callable = CallableId::method(model, method_name);
										let item = CallHierarchyItem {
											name: method_name.to_string(),
											kind: SymbolKind::METHOD,
											tags: None,
											detail: Some(model.to_string()),
											uri: uri.clone(),
											range: span_conv(node.range()),
											selection_range: span_conv(method_node.range()),
											data: serde_json::to_value(CallHierarchyData { callable }).ok(),
										};
										return Ok(Some(vec![item]));
									}
									}
								}
							}
						} else if func_node.kind() == "identifier" {
							// Direct function call: func()
							let func_range = func_node.byte_range();
							if func_range.contains(&offset) {
								let func_name = &contents[func_range.clone()];

								// Check if it's a known function in current file or imports
								let callable = CallableId::function(file_path_str, func_name);
								let item = CallHierarchyItem {
									name: func_name.to_string(),
									kind: SymbolKind::FUNCTION,
									tags: None,
									detail: Some(file_path_str.to_string()),
									uri: uri.clone(),
									range: span_conv(node.range()),
									selection_range: span_conv(func_node.range()),
									data: serde_json::to_value(CallHierarchyData { callable }).ok(),
								};
								return Ok(Some(vec![item]));
							}
						}
					}
				}
				_ => {}
			}
			node_at_cursor = node.parent();
		}

		Ok(None)
	}

	pub fn python_hover(&self, params: HoverParams, rope: RopeSlice<'_>) -> anyhow::Result<Option<Hover>> {
		let uri = &params.text_document_position_params.text_document.uri;
		let file_path = uri_to_path(uri)?;
		let file_path_str = file_path.to_str().unwrap();
		let ast = self
			.ast_map
			.get(file_path_str)
			.ok_or_else(|| errloc!("Did not build AST for {}", file_path_str))?;
		let ByteOffset(offset) = rope_conv(params.text_document_position_params.position, rope);

		let contents = Cow::from(rope);
		let root = some!(top_level_stmt(ast.root_node(), offset));
		let query = PyCompletions::query();
		let mut cursor = tree_sitter::QueryCursor::new();
		let mut this_model = ThisModel::default();

		let mut matches = cursor.matches(query, ast.root_node(), contents.as_bytes());
		while let Some(match_) = matches.next() {
			for capture in match_.captures {
				let range = capture.node.byte_range();
				match PyCompletions::from(capture.index) {
					Some(PyCompletions::Model) => {
						if range.contains_end(offset) {
							let range = range.shrink(1);
							let lsp_range = span_conv(capture.node.range());
							let slice = ok!(rope.try_slice(range.clone()));
							let slice = Cow::from(slice);
							return self.index.hover_model(&slice, Some(lsp_range), false, None);
						}
						if range.end < offset
							&& match_
								.nodes_for_capture_index(PyCompletions::Prop as _)
								.next()
								.is_some()
						{
							this_model.tag_model(capture.node, match_, root.byte_range(), &contents);
						}
					}
					Some(PyCompletions::Mapped) => {
						if range.contains(&offset) {
							let mapped = some!(self.gather_mapped(
								root,
								match_,
								Some(offset),
								range.clone(),
								this_model.inner,
								&contents,
								false,
								None,
							));
							let mut needle = mapped.needle;
							let mut model = _I(mapped.model);
							let mut range = mapped.range;
							if !mapped.single_field {
								some!(
									self.index
										.models
										.resolve_mapped(&mut model, &mut needle, Some(&mut range))
										.ok()
								);
							}
							let model = _R(model);
							return (self.index).hover_property_name(needle, model, Some(rope_conv(range, rope)));
						} else if let Some(cmdlist) = python_next_named_sibling(capture.node)
							&& Backend::is_commandlist(cmdlist, offset)
						{
							let (needle, range, model) = some!(self.gather_commandlist(
								cmdlist,
								root,
								match_,
								offset,
								range,
								this_model.inner,
								&contents,
								false,
							));
							let range = Some(rope_conv(range, rope));
							return self.index.hover_property_name(needle, _R(model), range);
						}
					}
					Some(PyCompletions::XmlId) if range.contains_end(offset) => {
						let range = range.shrink(1);
						let slice = Cow::from(ok!(rope.try_slice(range.clone())));
						let mut slice = slice.as_ref();
						if match_
							.nodes_for_capture_index(PyCompletions::HasGroups as _)
							.next()
							.is_some()
						{
							let mut ref_ = None;
							determine_csv_xmlid_subgroup(&mut ref_, (slice, range.clone()), offset);
							if let Some((needle, _)) = ref_ {
								slice = needle;
							}
						}
						return (self.index).hover_record(slice, Some(rope_conv(range.map_unit(ByteOffset), rope)));
					}
					Some(PyCompletions::Prop) if range.contains(&offset) => {
						let model = some!(this_model.inner);
						let name = &contents[range];
						let range = span_conv(capture.node.range());
						return self.index.hover_property_name(name, model, Some(range));
					}
					Some(PyCompletions::FieldDescriptor) => {
						let Some(desc_value) = python_next_named_sibling(capture.node) else {
							continue;
						};
						let descriptor = &contents[range];
						if !desc_value.byte_range().contains_end(offset) {
							continue;
						}

						if matches!(descriptor, "comodel_name") {
							let range = desc_value.byte_range().shrink(1);
							let lsp_range = span_conv(desc_value.range());
							let slice = ok!(rope.try_slice(range.clone()));
							let slice = Cow::from(slice);
							return self.index.hover_model(&slice, Some(lsp_range), false, None);
						} else if matches!(descriptor, "compute" | "search" | "inverse" | "related" | "inverse_name") {
							let single_field = matches!(descriptor, "related" | "inverse_name");
							let mapped_model = if descriptor == "inverse_name" {
								extract_comodel_name(match_.captures, &contents)
									.map(|comodel_name| &contents[comodel_name.byte_range().shrink(1)])
							} else {
								this_model.inner
							};
							let mapped = some!(self.gather_mapped(
								root,
								match_,
								Some(offset),
								desc_value.byte_range(),
								mapped_model,
								&contents,
								false,
								Some(single_field)
							));
							let mut needle = mapped.needle;
							let mut model = _I(mapped.model);
							let mut range = mapped.range;
							if !mapped.single_field {
								some!(
									self.index
										.models
										.resolve_mapped(&mut model, &mut needle, Some(&mut range))
										.ok()
								);
							}
							let model = _R(model);
							return (self.index).hover_property_name(needle, model, Some(rope_conv(range, rope)));
						} else if matches!(descriptor, "groups") {
							let range = desc_value.byte_range().shrink(1);
							let value = Cow::from(ok!(rope.try_slice(range.clone())));
							let mut ref_ = None;
							determine_csv_xmlid_subgroup(&mut ref_, (&value, range), offset);
							let (needle, byte_range) = some!(ref_);
							return self
								.index
								.hover_record(needle, Some(rope_conv(byte_range.map_unit(ByteOffset), rope)));
						} else if matches!(descriptor, "definition") {
							// Hover for Properties definition path
							// Format: "many2one_field.properties_definition_field"
							return self.hover_properties_definition(
								desc_value,
								offset,
								this_model.inner,
								&contents,
								rope,
							);
						}

						return Ok(None);
					}
					Some(PyCompletions::Request)
					| Some(PyCompletions::XmlId)
					| Some(PyCompletions::ForXmlId)
					| Some(PyCompletions::HasGroups)
					| Some(PyCompletions::MappedTarget)
					| Some(PyCompletions::Depends)
					| Some(PyCompletions::ReadFn)
					| Some(PyCompletions::Scope)
					| Some(PyCompletions::Prop)
					| Some(PyCompletions::FieldType)
					| None => {}
				}
			}
		}
		// First check if the cursor is on an attribute of Type::Env
		if let Some((lhs, attr, range)) = Self::attribute_node_at_offset(offset, root, &contents) {
			if let Some((tid, _scope)) =
				self.index.type_of_range(root, lhs.byte_range().map_unit(ByteOffset), &contents)
			{
				if matches!(type_cache().resolve(tid), Type::Env) {
					let lsp_range = Some(rope_conv(range.map_unit(ByteOffset), rope));
					return self.index.hover_env_attribute(attr, lsp_range);
				}
			}
		}

		if let Some((model, prop, range)) = self.attribute_at_offset(offset, root, &contents) {
			let lsp_range = Some(rope_conv(range.map_unit(ByteOffset), rope));
			return self.index.hover_property_name(prop, model, lsp_range);
		}

		// No matches, assume arbitrary expression.
		let root = some!(top_level_stmt(ast.root_node(), offset));
		let needle = some!(root.named_descendant_for_byte_range(offset, offset));
		let lsp_range = span_conv(needle.range());
		let (type_, scope) =
			some!((self.index).type_of_range(root, needle.byte_range().map_unit(ByteOffset), &contents));
		if let Some(model) = self.index.try_resolve_model(type_cache().resolve(type_), &scope) {
			let model = _R(model);
			let identifier = (needle.kind() == "identifier").then(|| &contents[needle.byte_range()]);
			return self.index.hover_model(model, Some(lsp_range), true, identifier);
		}

		// Check if we're hovering over a domain operator string
		if let Some(hover) = self.hover_domain_operator(needle, &contents, rope) {
			return Ok(Some(hover));
		}

		self.index.hover_variable(
			(needle.kind() == "identifier").then(|| &contents[needle.byte_range()]),
			type_,
			Some(lsp_range),
		)
	}

	/// Check if the given node is a domain operator and return hover info.
	fn hover_domain_operator(&self, node: Node, contents: &str, rope: RopeSlice<'_>) -> Option<Hover> {
		// Only check string nodes
		if node.kind() != "string" {
			return None;
		}

		let range = node.byte_range();
		if range.len() < 2 {
			return None;
		}

		let inner_range = range.clone().shrink(1);
		let operator = &contents[inner_range];

		// Check for domain-level operators (&, |, !)
		if let Some(doc) = crate::domain::get_domain_operator_hover(operator) {
			return Some(Hover {
				contents: HoverContents::Markup(MarkupContent {
					kind: MarkupKind::Markdown,
					value: doc,
				}),
				range: Some(rope_conv(range.map_unit(ByteOffset), rope)),
			});
		}

		// Check for term-level operators (=, !=, like, in, any, etc.)
		if let Some(doc) = crate::domain::get_operator_hover(operator) {
			return Some(Hover {
				contents: HoverContents::Markup(MarkupContent {
					kind: MarkupKind::Markdown,
					value: doc,
				}),
				range: Some(rope_conv(range.map_unit(ByteOffset), rope)),
			});
		}

		None
	}

	pub(crate) fn python_signature_help(&self, params: SignatureHelpParams) -> anyhow::Result<Option<SignatureHelp>> {
		use std::fmt::Write;

		let uri = &params.text_document_position_params.text_document.uri;
		let document = some!((self.document_map).get(uri.path().as_str()));
		let file_path = uri_to_path(uri)?;
		let ast = some!((self.ast_map).get(file_path.to_str().unwrap()));
		let contents = Cow::from(&document.rope);

		let point = tree_sitter::Point::new(
			params.text_document_position_params.position.line as _,
			params.text_document_position_params.position.character as _,
		);
		let node = some!(ast.root_node().descendant_for_point_range(point, point));
		let mut args = node;
		while let Some(parent) = args.parent() {
			if args.kind() == "argument_list" {
				break;
			}
			args = parent;
		}

		if args.kind() != "argument_list" {
			return Ok(None);
		}

		let active_parameter = 'find_param: {
			let ByteOffset(offset) = rope_conv(params.text_document_position_params.position, document.rope.slice(..));
			if let Some(contents) = contents.get(..=offset)
				&& let Some(idx) = contents.bytes().rposition(|c| c == b',' || c == b'(')
			{
				if contents.as_bytes()[idx] == b'(' {
					break 'find_param Some(0);
				}
				let prev_param = args.descendant_for_byte_range(idx, idx).unwrap().prev_named_sibling();
				for (idx, arg) in args.named_children(&mut args.walk()).enumerate() {
					if Some(arg) == prev_param {
						// the index might be intentionally out of bounds w.r.t the actual number of arguments
						// but this is better than leaving it as None because clients infer it as the first argument
						break 'find_param Some((idx + 1) as u32);
					}
				}
			}

			None
		};

		let callee = some!(args.prev_named_sibling());
		let Some((tid, _)) =
			(self.index).type_of_range(ast.root_node(), callee.byte_range().map_unit(ByteOffset), &contents)
		else {
			return Ok(None);
		};
		let Type::Method(model_key, method) = type_cache().resolve(tid) else {
			return Ok(None);
		};
		let method_key = some!(_G(method));
		let rtype = (self.index).eval_method_rtype(method_key.into(), **model_key, None);
		let model = some!((self.index).models.get(model_key));
		let method_obj = some!(some!(model.methods.as_ref()).get(&method_key));

		let mut label = format!("{method}(");
		let mut parameters = vec![];

		for (idx, param) in method_obj.arguments.as_deref().unwrap_or(&[]).iter().enumerate() {
			let begin;
			if idx == 0 {
				begin = label.len();
				_ = write!(&mut label, "{param}");
			} else {
				begin = label.len() + 2;
				_ = write!(&mut label, ", {param}");
			}
			let end = label.len();
			parameters.push(ParameterInformation {
				label: ParameterLabel::LabelOffsets([begin as _, end as _]),
				documentation: None,
			});
		}

		let rtype = rtype.and_then(|rtype| self.index.type_display(rtype));
		match rtype {
			Some(rtype) => drop(write!(&mut label, ") -> {rtype}")),
			None => label.push_str(") -> ..."),
		};

		let sig = SignatureInformation {
			label,
			active_parameter,
			parameters: Some(parameters),
			documentation: method_obj.docstring.as_ref().map(|doc| {
				Documentation::MarkupContent(MarkupContent {
					kind: MarkupKind::Markdown,
					value: doc.to_string(),
				})
			}),
		};

		Ok(Some(SignatureHelp {
			signatures: vec![sig],
			active_signature: Some(0),
			active_parameter: None,
		}))
	}
	pub(crate) fn python_code_action(
		&self,
		params: CodeActionParams,
		rope: RopeSlice<'_>,
	) -> anyhow::Result<Option<CodeActionResponse>> {
		use std::collections::HashMap;

		let uri = &params.text_document.uri;
		let Some(file_path) = uri.to_file_path() else {
			return Ok(None);
		};
		let path_str = file_path.to_str().ok_or_else(|| anyhow::anyhow!("Invalid path"))?;
		let Some(ast) = self.ast_map.get(path_str) else {
			return Ok(None);
		};
		let ByteOffset(offset) = rope_conv(params.range.end, rope);
		let contents = Cow::from(rope);

		let mut actions: Vec<CodeActionOrCommand> = Vec::new();

		// 1. Model code actions (e.g., jump to model definition)
		let query = PyCompletions::query();
		let mut cursor = tree_sitter::QueryCursor::new();
		let mut matches = cursor.matches(query, ast.root_node(), contents.as_bytes());
		while let Some(match_) = matches.next() {
			for capture in match_.captures {
				let range = capture.node.byte_range();
				match PyCompletions::from(capture.index) {
					Some(PyCompletions::Model) if range.contains_end(offset) => {
						let range = range.shrink(1);
						let slice = ok!(rope.try_slice(range.clone()));
						let slice = Cow::from(slice);
						if let Some(model_actions) = self.index.code_action_for_model(&slice, &file_path)? {
							actions.extend(model_actions);
						}
					}
					_ => {}
				}
			}
		}

		// 2. Missing super() call quick-fix
		let diagnostics = &params.context.diagnostics;
		let missing_super_diags: Vec<_> = diagnostics
			.iter()
			.filter(|d| {
				d.message.contains("does not call `super()")
					&& d.range.start.line >= params.range.start.line
					&& d.range.end.line <= params.range.end.line
			})
			.collect();

		for diag in missing_super_diags {
			// Extract method name from diagnostic message
			let method_name = diag
				.message
				.strip_prefix("Method `")
				.and_then(|s| s.split('`').next());

			let Some(method_name) = method_name else {
				continue;
			};

			// Find the method node at this location
			let start_offset: ByteOffset = rope_conv(diag.range.start, rope);
			let Some(node) = ast.root_node().descendant_for_byte_range(start_offset.0, start_offset.0) else {
				continue;
			};

			// Walk up to find function_definition
			let mut fn_node = node;
			while fn_node.kind() != "function_definition" {
				let Some(parent) = fn_node.parent() else {
					break;
				};
				fn_node = parent;
			}

			if fn_node.kind() != "function_definition" {
				continue;
			}

			// Find the block (body) of the function
			let Some(block) = fn_node.child_by_field_name("body") else {
				continue;
			};

			// Find the first statement in the block
			let Some(first_stmt) = block.named_child(0) else {
				continue;
			};

			// Check if it's a docstring - if so, insert after it
			let insert_after_docstring = first_stmt.kind() == "expression_statement"
				&& first_stmt.named_child(0).is_some_and(|n| n.kind() == "string");

			let insert_node = if insert_after_docstring {
				block.named_child(1).unwrap_or(first_stmt)
			} else {
				first_stmt
			};

			// Get the indentation of the first statement
			let insert_range: Range = span_conv(insert_node.range());
			let stmt_start: ByteOffset = rope_conv(
				Position {
					line: insert_range.start.line,
					character: 0,
				},
				rope,
			);
			let stmt_line = &contents[stmt_start.0..insert_node.start_byte()];
			let indent = stmt_line.chars().take_while(|c| c.is_whitespace()).collect::<String>();

			// Build the super() call text
			let super_call = format!("{}super().{}()\n", indent, method_name);

			// Calculate insert position (at the start of the insert_node line)
			let insert_pos = if insert_after_docstring {
				// Insert on a new line after the docstring
				let docstring_end: Range = span_conv(first_stmt.range());
				Position {
					line: docstring_end.end.line + 1,
					character: 0,
				}
			} else {
				Position {
					line: insert_range.start.line,
					character: 0,
				}
			};

			// Create the text edit
			let edit = TextEdit {
				range: Range {
					start: insert_pos,
					end: insert_pos,
				},
				new_text: super_call,
			};

			// Create workspace edit
			let mut changes = HashMap::new();
			changes.insert(uri.clone(), vec![edit]);

			let workspace_edit = WorkspaceEdit {
				changes: Some(changes),
				..Default::default()
			};

			actions.push(CodeActionOrCommand::CodeAction(CodeAction {
				title: format!("Add missing super().{}() call", method_name),
				kind: Some(CodeActionKind::QUICKFIX),
				diagnostics: Some(vec![diag.clone()]),
				edit: Some(workspace_edit),
				..Default::default()
			}));
		}

		if actions.is_empty() {
			Ok(None)
		} else {
			Ok(Some(actions))
		}
	}

	fn is_commandlist(cmdlist: Node, offset: usize) -> bool {
		matches!(cmdlist.kind(), "list" | "list_comprehension")
			&& cmdlist.byte_range().contains_end(offset)
			&& cmdlist.parent().is_some_and(|parent| parent.kind() == "pair")
	}
	/// `cmdlist` must have been checked by [is_commandlist][Backend::is_commandlist] first
	///
	/// Returns `(needle, range, model)` for a field
	fn gather_commandlist<'text>(
		&self,
		cmdlist: Node,
		root: Node,
		match_: &tree_sitter::QueryMatch,
		offset: usize,
		range: std::ops::Range<usize>,
		this_model: Option<&'text str>,
		contents: &'text str,
		for_replacing: bool,
	) -> Option<(&'text str, ByteRange, Spur)> {
		let mut access = contents[range.shrink(1)].to_string();
		tracing::debug!(
			"gather_commandlist: cmdlist range: {:?}, offset: {}",
			cmdlist.byte_range(),
			offset
		);
		let mut dest = cmdlist.descendant_for_byte_range(offset, offset);
		tracing::debug!("Initial dest: {:?}", dest.map(|n| (n.kind(), n.byte_range())));

		// If we can't find a node at the exact offset, try to find the last string node
		// This handles the case where cursor is after a string without a colon
		if dest.is_none() && offset > cmdlist.start_byte() {
			tracing::debug!("No node at offset {}, trying offset - 1", offset);
			// Try offset - 1 to see if we're just after a string
			if let Some(node) = cmdlist.descendant_for_byte_range(offset - 1, offset - 1) {
				tracing::debug!(
					"Found node at offset - 1: kind={}, range={:?}",
					node.kind(),
					node.byte_range()
				);
				if node.kind() == "string"
					|| (node.kind() == "string_content" && node.parent().map(|p| p.kind()) == Some("string"))
					|| node.kind() == "string_end"
				{
					// Check if this string is not part of a key-value pair (no colon after it)
					let string_node = if node.kind() == "string" {
						node
					// } else if node.kind() == "string_end" && node.parent().map(|p| p.kind()) == Some("string") {
					// 	node.parent()?
					} else {
						node.parent()?
					};
					if let Some(next_sibling) = string_node.next_sibling() {
						tracing::debug!("String has next sibling: {}", next_sibling.kind());
						if next_sibling.kind() != ":" {
							// This is an incomplete field name, provide completions
							dest = Some(string_node);
						}
					} else {
						tracing::debug!("String has no next sibling, treating as incomplete");
						// No next sibling means it's the last element, likely incomplete
						dest = Some(string_node);
					}
				}
			}
		}

		let mut dest = dest?;

		// If we're inside a string_content node, get the parent string node
		if dest.kind() == "string_content" {
			dest = dest.parent()?;
		}

		if dest.kind() != "string" {
			dest = dest.parent()?;
		}
		if dest.kind() != "string" {
			return None;
		}

		// First check if this string is in a broken syntax situation
		// (i.e., it's a key in a dictionary without a following colon)
		let mut is_broken_syntax = false;
		if let Some(parent) = dest.parent() {
			tracing::debug!("String parent kind: {}", parent.kind());
			if parent.kind() == "dictionary" {
				// Check if this string has a colon after it
				if let Some(next_sibling) = dest.next_sibling() {
					tracing::debug!("String next sibling kind: {}", next_sibling.kind());
					if next_sibling.kind() != ":" {
						is_broken_syntax = true;
					}
				} else {
					tracing::debug!("String has no next sibling");
					// No next sibling means it's the last element, likely incomplete
					is_broken_syntax = true;
				}
			} else if parent.kind() == "ERROR" {
				// When there's broken syntax, tree-sitter creates ERROR nodes
				// Check if the parent of the ERROR is a dictionary
				if let Some(grandparent) = parent.parent() {
					tracing::debug!("ERROR parent (grandparent) kind: {}", grandparent.kind());
					if grandparent.kind() == "dictionary" {
						// This is likely a string in a dictionary with broken syntax
						is_broken_syntax = true;
					}
				}
			}
		}

		if is_broken_syntax {
			tracing::debug!("Detected broken syntax: string in dictionary without colon");
			// For broken syntax, we need to continue processing to determine the model
			// but we'll use an empty needle to show all available fields
		}

		let (needle, model_str, range) = if is_broken_syntax {
			// For broken syntax, we don't want to complete the partial field name
			// We want to show all available fields
			// We still need to get the model context, so we'll use the parent model if available
			let range = ByteRange {
				start: ByteOffset(offset),
				end: ByteOffset(offset),
			};
			// Use the this_model if available, otherwise we'll need to determine it from context
			// For command lists without explicit model, we need to continue processing
			// to determine the model from the field context
			let model = this_model.unwrap_or("");
			("", model, range)
		} else {
			// Normal case - complete the field name
			let Mapped {
				needle, model, range, ..
			} = self.gather_mapped(
				root,
				match_,
				Some(offset),
				dest.byte_range(),
				this_model,
				contents,
				for_replacing,
				None,
			)?;
			(needle, model, range)
		};

		tracing::debug!(
			"needle={}, is_broken_syntax={}, model_str={}",
			needle,
			is_broken_syntax,
			model_str
		);

		// recursive descent to collect the chain of fields
		let mut cursor = cmdlist;
		let mut count = 0;
		while count < 30 {
			count += 1;
			let Some(candidate) = cursor.child_with_descendant(dest) else {
				tracing::debug!("child_containing_descendant returned None at count={}", count);
				return None;
			};
			let obj;
			tracing::debug!("candidate kind: {}", candidate.kind());
			if candidate.kind() == "tuple" {
				// (0, 0, {})
				obj = candidate.child_with_descendant(dest)?;
			} else if candidate.kind() == "call" {
				// Command.create({}), but we don't really care if the actual function is called.
				let args = dig!(candidate, argument_list(1))?;
				obj = args.child_with_descendant(dest)?;
			} else {
				return None;
			}
			tracing::debug!("obj kind: {}", obj.kind());
			if obj.kind() == "dictionary" {
				let pair = obj.child_with_descendant(dest)?;
				tracing::debug!("pair kind: {}", pair.kind());
				if pair.kind() != "pair" {
					// Check if this is a broken syntax case (string without colon)
					if pair.kind() == "string" && pair.byte_range().contains(&offset) {
						// This is a string in a dictionary without a colon
						// We're completing field names for this dictionary
						tracing::debug!("Breaking due to broken syntax string in dictionary");
						// Break out of the loop to resolve the model
						break;
					} else if pair.kind() == "ERROR" {
						// When there's broken syntax, tree-sitter might create an ERROR node
						// Check if the ERROR contains our string
						if pair.byte_range().contains(&offset) {
							tracing::debug!("Breaking due to ERROR node containing offset");
							break;
						}
					}
					tracing::debug!("Returning None: pair kind {} is not 'pair'", pair.kind());
					return None;
				}

				let key = dig!(pair, string)?;
				if key.byte_range().contains_end(offset) {
					break;
				}

				cursor = pair.child_with_descendant(dest)?;
				access.push('.');
				access.push_str(&contents[key.byte_range().shrink(1)]);
			} else if obj.kind() == "set" {
				break;
			} else {
				// TODO: (ERROR) case
				return None;
			}
		}

		if count == 30 {
			warn!("recursion limit hit");
		}

		access.push('.'); // to force resolution of the last field
		tracing::debug!("Access path: {}", access);
		tracing::debug!("Initial model before resolve: {}", model_str);
		let access = &mut access.as_str();
		let mut model = _I(model_str);
		if self.index.models.resolve_mapped(&mut model, access, None).is_err() {
			tracing::debug!("resolve_mapped failed for model={} access={}", _R(model), access);
			return None;
		}
		tracing::debug!("Resolved model: {}", _R(model));

		Some((needle, range, model))
	}

}

#[derive(Default, Clone)]
struct ThisModel<'a> {
	inner: Option<&'a str>,
	source: ThisModelKind,
	top_level_range: core::ops::Range<usize>,
}

#[derive(Default, Clone, Copy)]
enum ThisModelKind {
	Primary,
	#[default]
	Inherited,
}

impl<'this> ThisModel<'this> {
	/// Call this on captures of index [`PyCompletions::Model`].
	fn tag_model(
		&mut self,
		model: Node,
		match_: &QueryMatch,
		top_level_range: core::ops::Range<usize>,
		contents: &'this str,
	) {
		if match_
			.nodes_for_capture_index(PyCompletions::FieldType as _)
			.next()
			.is_some()
		{
			// debug_assert!(false, "tag_model called on a class model; handle this manually");
			return;
		}

		debug_assert_eq!(model.kind(), "string");
		let (is_name, mut is_inherit) = match_
			.nodes_for_capture_index(PyCompletions::Prop as _)
			.next()
			.map(|prop| {
				let prop = &contents[prop.byte_range()];
				(prop == "_name", prop == "_inherit")
			})
			.unwrap_or((false, false));
		let top_level_changed = top_level_range != self.top_level_range;
		// If still in same class AND _name already declared, skip.
		is_inherit = is_inherit && (top_level_changed || matches!(self.source, ThisModelKind::Inherited));
		if is_inherit {
			let parent = model.parent().expect(format_loc!("(tag_model) parent"));
			// _inherit = '..' OR _inherit = ['..']
			is_inherit = parent.kind() == "assignment" || parent.kind() == "list" && parent.named_child_count() == 1;
		}
		if is_inherit || is_name && top_level_changed {
			self.inner = Some(&contents[model.byte_range().shrink(1)]);
			self.top_level_range = top_level_range;
			if is_name {
				self.source = ThisModelKind::Primary;
			} else if is_inherit {
				self.source = ThisModelKind::Inherited;
			}
		}
	}
}

fn extract_string_needle_at_offset<'a>(
	rope: RopeSlice<'a>,
	range: core::ops::Range<usize>,
	offset: usize,
) -> anyhow::Result<(Cow<'a, str>, core::ops::Range<ByteOffset>)> {
	let slice = rope.try_slice(range.clone())?;
	let relative_offset = range.start;
	let needle = Cow::from(slice.try_slice(1..offset - relative_offset)?);
	let byte_range = range.shrink(1).map_unit(ByteOffset);
	Ok((needle, byte_range))
}

fn extract_comodel_name<'tree>(captures: &[QueryCapture<'tree>], contents: &str) -> Option<Node<'tree>> {
	for cap in captures {
		if matches!(PyCompletions::from(cap.index), Some(PyCompletions::FieldDescriptor))
			&& &contents[cap.node.byte_range()] == "comodel_name"
		{
			return python_next_named_sibling(cap.node);
		}
	}
	None
}
