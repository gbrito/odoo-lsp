//! Methods related to type analysis. The two most important methods are
//! [`Index::model_of_range`] and [`Index::type_of`].

use core::borrow::Borrow;
use std::{
	fmt::{Debug, Write},
	ops::ControlFlow,
	sync::{Arc, OnceLock, atomic::Ordering},
};

use dashmap::DashMap;
use fomat_macros::fomat;
use lasso::Spur;
use ropey::Rope;
use tracing::{instrument, trace};
use tree_sitter::{Node, QueryCursor, StreamingIterator};

use crate::{
	ImStr, dig, format_loc,
	index::{_G, _I, _R, Index, Symbol},
	model::{Method, ModelName, PropertyInfo},
	prelude::PathSymbol,
	test_utils,
	utils::{ByteOffset, ByteRange, Defer, PreTravel, RangeExt, TryResultExt, python_next_named_sibling, python_parser, rope_conv},
};
use ts_macros::query;

mod scope;
pub use scope::{ImportInfo, ImportMap, Scope};

pub fn type_cache() -> &'static TypeCache {
	static CACHE: OnceLock<TypeCache> = OnceLock::new();
	CACHE.get_or_init(TypeCache::default)
}

macro_rules! _T {
	(@ $builtin:expr) => {
		$crate::analyze::type_cache().get_or_intern(Type::PyBuiltin($builtin.into()))
	};
	($model:literal) => {
		$crate::analyze::type_cache().get_or_intern(Type::Model($model.into()))
	};
	($expr:expr) => {
		$crate::analyze::type_cache().get_or_intern($expr)
	};
}

macro_rules! _TR {
	($expr:expr) => {
		$crate::analyze::type_cache().resolve($expr)
	};
}

pub static MODEL_METHODS: phf::Set<&str> = phf::phf_set!(
	"create",
	"copy",
	"name_create",
	"browse",
	"filtered",
	"filtered_domain",
	"sorted",
	"search",
	"search_fetch",
	"name_search",
	"ensure_one",
	"with_context",
	"with_user",
	"with_company",
	"with_env",
	"sudo",
	"exists",
	"concat",
	// TODO: Limit to Forms only
	"new",
	"edit",
	"save",
);

/// Describes an attribute of the Odoo Environment (odoo.api.Environment).
#[derive(Debug, Clone, Copy)]
pub struct EnvAttribute {
	/// The attribute name (e.g., "user", "company")
	pub name: &'static str,
	/// The type as displayed in hover (e.g., "res.users", "int", "bool")
	pub type_display: &'static str,
	/// If this attribute returns a model, the model name (for go-to-definition)
	pub model: Option<&'static str>,
	/// The LSP completion item kind
	pub kind: EnvAttrKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvAttrKind {
	/// A model recordset (e.g., user, company)
	Model,
	/// A method/function (e.g., ref)
	Method,
	/// A simple property (e.g., uid, lang, su)
	Property,
}

/// All known attributes of odoo.api.Environment
pub static ENV_ATTRIBUTES: &[EnvAttribute] = &[
	EnvAttribute {
		name: "user",
		type_display: "res.users",
		model: Some("res.users"),
		kind: EnvAttrKind::Model,
	},
	EnvAttribute {
		name: "company",
		type_display: "res.company",
		model: Some("res.company"),
		kind: EnvAttrKind::Model,
	},
	EnvAttribute {
		name: "companies",
		type_display: "res.company",
		model: Some("res.company"),
		kind: EnvAttrKind::Model,
	},
	EnvAttribute {
		name: "uid",
		type_display: "int",
		model: None,
		kind: EnvAttrKind::Property,
	},
	EnvAttribute {
		name: "lang",
		type_display: "str | None",
		model: None,
		kind: EnvAttrKind::Property,
	},
	EnvAttribute {
		name: "su",
		type_display: "bool",
		model: None,
		kind: EnvAttrKind::Property,
	},
	EnvAttribute {
		name: "cr",
		type_display: "Cursor",
		model: None,
		kind: EnvAttrKind::Property,
	},
	EnvAttribute {
		name: "context",
		type_display: "dict",
		model: None,
		kind: EnvAttrKind::Property,
	},
	EnvAttribute {
		name: "ref",
		type_display: "fn(xml_id) -> record",
		model: None,
		kind: EnvAttrKind::Method,
	},
];

/// The subset of types that may resolve to a model.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Type {
	Env,
	/// \*.env.ref()
	RefFn,
	/// Functions that return another model, regardless of input.
	ModelFn(ImStr),
	Model(ImStr),
	/// Unresolved model.
	Record(ImStr),
	Super,
	Method(ModelName, ImStr),
	/// Module-level function reference (file path, function name).
	Function(PathSymbol, Symbol<crate::model::Function>),
	/// To hardcode some methods, such as dict.items()
	PythonMethod(TypeId, ImStr),
	/// `odoo.http.request`
	HttpRequest,
	Dict(TypeId, TypeId),
	/// A bag of enumerated properties and their types
	DictBag(Vec<(DictKey, TypeId)>),
	/// Equivalent to Value, but may have a better semantic name
	PyBuiltin(ImStr),
	List(ListElement),
	Tuple(Vec<TypeId>),
	Iterable(Option<TypeId>),
	/// Union of multiple possible types (e.g., `X | Y`, `Optional[X]` = `X | None`).
	/// Invariant: len >= 2 (single-element "unions" are simplified to the element itself).
	/// Always flattened and deduplicated.
	Union(Vec<TypeId>),
	/// Python's None type.
	None,
	/// Can never be resolved, useful for non-model bindings.
	Value,
}

impl Type {
	#[inline]
	fn is_dictlike(&self) -> bool {
		matches!(self, Type::Dict(..) | Type::DictBag(..))
	}
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub enum ListElement {
	Vacant,
	Occupied(TypeId),
}

impl Debug for ListElement {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::Vacant => f.write_str("..."),
			Self::Occupied(inner) => inner.fmt(f),
		}
	}
}

impl From<ListElement> for Option<TypeId> {
	#[inline]
	fn from(value: ListElement) -> Self {
		match value {
			ListElement::Vacant => None,
			ListElement::Occupied(inner) => Some(inner),
		}
	}
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub enum DictKey {
	String(ImStr),
	Type(TypeId),
}

impl Debug for DictKey {
	#[inline]
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::String(key) => key.fmt(f),
			Self::Type(key) => key.fmt(f),
		}
	}
}

#[derive(Clone, Debug)]
pub enum FunctionParam {
	Param(ImStr),
	/// `(positional_separator)`
	PosEnd,
	/// `(keyword_separator)` or `(list_splat_pattern)`
	EitherEnd(Option<ImStr>),
	/// `(default_parameter)`
	Named(ImStr),
	Kwargs(ImStr),
}

impl core::fmt::Display for FunctionParam {
	fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			FunctionParam::Param(param) => f.write_str(param),
			FunctionParam::PosEnd => f.write_char('/'),
			FunctionParam::EitherEnd(None) => f.write_char('*'),
			FunctionParam::EitherEnd(Some(param)) => write!(f, "*{param}"),
			FunctionParam::Named(param) => write!(f, "{param}=..."),
			FunctionParam::Kwargs(param) => write!(f, "**{param}"),
		}
	}
}

#[derive(Default)]
pub struct TypeCache {
	types: boxcar::Vec<Type>,
	ids: DashMap<Type, TypeId>,
}

impl TypeCache {
	#[inline]
	pub fn get_or_intern(&self, type_: Type) -> TypeId {
		if let Some(id) = self.ids.get(&type_) {
			return *id;
		}
		self.intern(type_)
	}
	fn intern(&self, type_: Type) -> TypeId {
		let id = TypeId(self.types.push(type_.clone()).try_into().unwrap());
		self.ids.insert(type_, id);
		id
	}
	#[inline]
	pub fn resolve<T: Borrow<TypeId>>(&self, id: T) -> &Type {
		unsafe { self.types.get_unchecked(id.borrow().0 as usize) }
	}
	/// Creates a union type from multiple types.
	/// - Flattens nested unions
	/// - Deduplicates identical types
	/// - Returns the single type if only one remains after deduplication
	/// - Returns None if no types provided
	pub fn union(&self, types: impl IntoIterator<Item = TypeId>) -> Option<TypeId> {
		let mut flat: Vec<TypeId> = Vec::new();

		for tid in types {
			match self.resolve(tid) {
				Type::Union(inner) => flat.extend(inner),
				_ => flat.push(tid),
			}
		}

		// Deduplicate: sort by inner u32 for consistent ordering, then dedup
		flat.sort_by_key(|t| t.0);
		flat.dedup();

		match flat.len() {
			0 => None,
			1 => Some(flat[0]),
			_ => Some(self.get_or_intern(Type::Union(flat))),
		}
	}
}

#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct TypeId(u32);

impl TypeId {
	#[inline]
	pub fn is_dictlike(&self) -> bool {
		type_cache().resolve(self).is_dictlike()
	}
	#[inline]
	pub fn is_dict(&self) -> bool {
		matches!(type_cache().resolve(self), Type::Dict(..))
	}
	#[inline]
	pub fn is_dictbag(&self) -> bool {
		matches!(type_cache().resolve(self), Type::DictBag(..))
	}
}

impl Debug for TypeId {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		_TR!(*self).fmt(f)
	}
}

pub fn normalize<'r, 'n>(node: &'r mut Node<'n>) -> &'r mut Node<'n> {
	let mut cursor = node.walk();
	while matches!(
		node.kind(),
		"expression_statement" | "parenthesized_expression" | "module"
	) {
		let Some(child) = node.named_children(&mut cursor).find(|child| child.kind() != "comment") else {
			break;
		};
		*node = child;
	}
	node
}

#[rustfmt::skip]
query! {
	#[derive(Debug)]
	FieldCompletion(Name, SelfParam, Scope);
((class_definition
  (block
    (expression_statement [
      (assignment (identifier) @_name (string) @NAME)
	  (assignment (identifier) @_inherit (list . (string) @NAME)) ])?
    [
      (decorated_definition
        (function_definition
          (parameters . (identifier) @SELF_PARAM)) @SCOPE)
      (function_definition (parameters . (identifier) @SELF_PARAM)) @SCOPE])) @class
  (#eq? @_inherit "_inherit")
  (#match? @_name "^_(name|inherit)$"))
}

#[rustfmt::skip]
query! {
	MappedCall(Callee, Iter);
((call
  (attribute (_) @CALLEE (identifier) @_mapped)
  (argument_list [
    (lambda (lambda_parameters . (identifier) @ITER))
    (keyword_argument
      (identifier) @_func
      (lambda (lambda_parameters . (identifier) @ITER)))]))
  (#match? @_func "^(func|key)$")
  (#match? @_mapped "^(mapp|filter|sort|group)ed$"))
}

#[rustfmt::skip]
query! {
	PythonBuiltinCall(Append, AppendList, AppendMap, AppendMapKey, AppendValue, UpdateMap, UpdateArgs);
// value.append(...) OR value['foobar'].append(...)
(call
  (attribute
    (subscript (identifier) @APPEND_MAP (string (string_content) @APPEND_MAP_KEY))
    (identifier) @_append)
  (argument_list . (_) @APPEND_VALUE)
  (#eq? @_append "append"))

(call
  (attribute
    (identifier) @APPEND_LIST
    (identifier) @APPEND)
  (argument_list . (_) @APPEND_VALUE)
  (#eq? @_append "append"))

// value.update(...)
(call
  (attribute
  	(identifier) @UPDATE_MAP
  	(identifier) @_update)
  (argument_list) @UPDATE_ARGS
  (#eq? @_update "update"))
}

#[rustfmt::skip]
query! {
	PyImportsQuery(ImportModule, ImportName, ImportAlias);

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

/// Parse import statements from Python content and return a map of imported names to their module paths.
pub fn parse_imports(root: Node, contents: &str) -> ImportMap {
	let query = PyImportsQuery::query();
	let mut cursor = QueryCursor::new();
	let mut imports = ImportMap::new();

	let mut matches = cursor.matches(query, root, contents.as_bytes());
	while let Some(match_) = matches.next() {
		let mut module_path = None;
		let mut import_name = None;
		let mut alias = None;

		for capture in match_.captures {
			let capture_text = &contents[capture.node.byte_range()];

			match PyImportsQuery::from(capture.index) {
				Some(PyImportsQuery::ImportModule) => {
					module_path = Some(capture_text.to_string());
				}
				Some(PyImportsQuery::ImportName) => {
					import_name = Some(capture_text.to_string());
				}
				Some(PyImportsQuery::ImportAlias) => {
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

			let key = alias.unwrap_or_else(|| name.clone());
			imports.insert(
				key,
				ImportInfo {
					module_path: full_module_path,
					imported_name: name,
				},
			);
		}
	}

	imports
}

pub type ScopeControlFlow = ControlFlow<Option<Scope>, bool>;
impl Index {
	#[inline]
	pub fn model_of_range(&self, node: Node<'_>, range: ByteRange, contents: &str) -> Option<ModelName> {
		let (type_at_cursor, scope) = self.type_of_range(node, range, contents)?;
		self.try_resolve_model(_TR!(type_at_cursor), &scope)
	}
	pub fn type_of_range(&self, root: Node<'_>, range: ByteRange, contents: &str) -> Option<(TypeId, Scope)> {
		self.type_of_range_with_path(root, range, contents, None)
	}

	pub fn type_of_range_with_path(
		&self,
		root: Node<'_>,
		range: ByteRange,
		contents: &str,
		current_path: Option<PathSymbol>,
	) -> Option<(TypeId, Scope)> {
		// Phase 1: Determine the scope.
		let (self_type, fn_scope, self_param) = determine_scope(root, contents, range.start.0)?;

		// Phase 1.5: Parse imports from the file
		// We need to parse from the module root, not from the class/function scope
		let module_root = {
			let mut node = root;
			while node.kind() != "module" {
				match node.parent() {
					Some(parent) => node = parent,
					None => break,
				}
			}
			node
		};
		let imports = Arc::new(parse_imports(module_root, contents));

		// Phase 2: Build the scope up to offset
		// What contributes to a method scope's variables?
		// 1. Top-level statements; completely opaque to us.
		// 2. Class definitions; technically useless to us.
		//    Self-type analysis only uses a small part of the class definition.
		// 3. Parameters, e.g. self which always has a fixed type
		// 4. Assignments (including walrus-assignment)
		let mut scope = Scope::default();
		let self_type = match self_type {
			Some(type_) => &contents[type_.byte_range().shrink(1)],
			None => "",
		};
		scope.super_ = Some(self_param.into());
		scope.current_path = current_path;
		scope.imports = Some(imports);
		scope.insert(self_param.to_string(), Type::Model(self_type.into()));
		scope.super_ = Some(self_param.into());

		// special case: is this a property?
		// TODO: fields
		let node_at_cursor = fn_scope.descendant_for_byte_range(range.start.0, range.end.0)?;
		if node_at_cursor.kind() == "identifier" && fn_scope.child_by_field_name("name") == Some(node_at_cursor) {
			return Some((
				_T!(Type::Method(
					_I(self_type).into(),
					contents[node_at_cursor.byte_range()].into()
				)),
				scope,
			));
		}

		// Phase 3: With the proper context available, determine the type of the node.
		self.type_of_node(Some(scope), fn_scope, range, contents)
	}
	pub fn type_of_node(
		&self,
		scope: Option<Scope>,
		node: Node<'_>,
		range: ByteRange,
		contents: &str,
	) -> Option<(TypeId, Scope)> {
		let scope = scope.unwrap_or_default();
		let (orig_scope, scope) = Self::walk_scope(node, Some(scope), |scope, node| {
			self.build_scope(scope, node, range.end.0, contents)
		});
		let scope = scope.unwrap_or(orig_scope);
		let node_at_cursor = node.descendant_for_byte_range(range.start.0, range.end.0)?;
		let type_at_cursor = self.type_of(node_at_cursor, &scope, contents)?;
		Some((type_at_cursor, scope))
	}
	/// Builds the scope up to `offset`, in bytes.
	///
	/// #### About [ScopeControlFlow]
	/// This is one of the rare occasions where [ControlFlow] is used. It is similar to
	/// [Result] in that the try-operator (?) can be used to end iteration on a
	/// [ControlFlow::Break]. Otherwise, [ControlFlow::Continue] has a continuation value
	/// that must be passed up the chain, since it indicates whether [Scope::enter] was called.
	pub fn build_scope(&self, scope: &mut Scope, node: Node, offset: usize, contents: &str) -> ScopeControlFlow {
		if node.start_byte() > offset {
			return ControlFlow::Break(Some(core::mem::take(scope)));
		}
		match node.kind() {
			"assignment" | "named_expression" => {
				// (_ left right)
				let lhs = node.named_child(0).unwrap();
				if lhs.kind() == "identifier"
					&& let rhs = python_next_named_sibling(lhs).expect(format_loc!("rhs"))
					&& let Some(id) = self.type_of(rhs, scope, contents)
				{
					let lhs = &contents[lhs.byte_range()];
					scope.insert(lhs.to_string(), _TR!(id).clone());
				} else if lhs.kind() == "subscript"
					&& let Some(map) = dig!(lhs, identifier)
					&& let Some(key) = dig!(lhs, string(1).string_content(1))
					&& let Some(rhs) = python_next_named_sibling(lhs)
					&& let type_ = self.type_of(rhs, scope, contents)
					&& let Some(Type::DictBag(properties)) = scope.get_mut(&contents[map.byte_range()])
				{
					let type_ = type_.unwrap_or_else(|| _T!(Type::Value));
					let key = &contents[key.byte_range()];
					if let Some(idx) = properties.iter().position(|(prop, _)| match prop {
						DictKey::String(prop) => prop.as_str() == key,
						DictKey::Type(_) => false,
					}) {
						properties[idx].1 = type_;
					} else {
						properties.push((DictKey::String(ImStr::from(key)), type_));
					}
				} else if lhs.kind() == "pattern_list"
					&& let Some(rhs) = python_next_named_sibling(lhs)
					&& let Some(type_) = self.type_of(rhs, scope, contents)
				{
					self.destructure_into_patternlist_like(lhs, type_, scope, contents);
				}
			}
			"for_statement" => {
				// (for_statement left right body)
				scope.enter(true);
				let lhs = node.named_child(0).unwrap();

				if let Some(rhs) = python_next_named_sibling(lhs)
					&& let Some(type_) = self.type_of(rhs, scope, contents)
					&& let Some(inner) = self.type_of_iterable(type_)
				{
					self.destructure_into_patternlist_like(lhs, inner, scope, contents);
				}
				return ControlFlow::Continue(true);
			}
			"function_definition" => {
				let mut inherit_super = false;
				let mut node = node;
				while let Some(parent) = node.parent() {
					match parent.kind() {
						"decorated_definition" => {
							node = parent;
							continue;
						}
						"block" => inherit_super = parent.parent().is_some_and(|gp| gp.kind() == "class_definition"),
						_ => {}
					}
					break;
				}
				scope.enter(inherit_super);
				return ControlFlow::Continue(true);
			}
			"list_comprehension" | "set_comprehension" | "dictionary_comprehension" | "generator_expression"
				if node.byte_range().contains(&offset) =>
			{
				// (_ body: _ (for_in_clause left: _ right: _))
				let for_in = node.named_child(1).unwrap();
				if let Some(lhs) = for_in.child_by_field_name("left")
					&& let Some(rhs) = for_in.child_by_field_name("right")
					&& let Some(tid) = self.type_of(rhs, scope, contents)
					&& let Some(inner) = self.type_of_iterable(tid)
				{
					self.destructure_into_patternlist_like(lhs, inner, scope, contents);
				}
			}
			"call" if node.byte_range().contains_end(offset) => {
				// model.{mapped,filtered,sorted,*}(lambda rec: ..)
				let query = MappedCall::query();
				let mut cursor = QueryCursor::new();
				let mut matches = cursor.matches(query, node, contents.as_bytes());
				if let Some(mapped_call) = matches.next()
					&& let callee = mapped_call
						.nodes_for_capture_index(MappedCall::Callee as _)
						.next()
						.unwrap() && let Some(tid) = self.type_of(callee, scope, contents)
				{
					let iter = mapped_call
						.nodes_for_capture_index(MappedCall::Iter as _)
						.next()
						.unwrap();
					let iter = &contents[iter.byte_range()];
					scope.insert(iter.to_string(), _TR!(tid).clone());
				}
			}
			"call" => {
				let query = PythonBuiltinCall::query();
				let mut cursor = QueryCursor::new();
				let mut matches = cursor.matches(query, node, contents.as_bytes());
				let Some(call) = matches.next() else {
					return ControlFlow::Continue(false);
				};
				if let Some(value) = call.nodes_for_capture_index(PythonBuiltinCall::AppendValue as _).next()
					&& let Some(tid) = self.type_of(value, scope, contents)
				{
					if let Some(list) = call.nodes_for_capture_index(PythonBuiltinCall::AppendList as _).next() {
						if let Some(Type::List(slot @ ListElement::Vacant)) =
							scope.get_mut(&contents[list.byte_range()])
						{
							*slot = ListElement::Occupied(tid);
						}
					} else if let Some(map) = call.nodes_for_capture_index(PythonBuiltinCall::AppendMap as _).next() {
						let Some(key) = call
							.nodes_for_capture_index(PythonBuiltinCall::AppendMapKey as _)
							.next()
						else {
							return ControlFlow::Continue(false);
						};
						let key = &contents[key.byte_range()];

						if let Some(Type::DictBag(properties)) = scope.get_mut(&contents[map.byte_range()])
							&& let Some((_, slot)) = properties.iter_mut().find(|(prop, id)| match prop {
								DictKey::String(prop) => {
									prop.as_str() == key && _T!(Type::List(ListElement::Vacant)) == *id
								}
								DictKey::Type(_) => false,
							}) {
							*slot = _T!(Type::List(ListElement::Occupied(tid)));
						}
					}
				} else if let Some(map) = call.nodes_for_capture_index(PythonBuiltinCall::UpdateMap as _).next() {
					let Some(Type::DictBag(properties)) = scope.get_mut(&contents[map.byte_range()]) else {
						return ControlFlow::Continue(false);
					};
					let Some(args) = call.nodes_for_capture_index(PythonBuiltinCall::UpdateArgs as _).next() else {
						return ControlFlow::Continue(false);
					};

					let mut properties = core::mem::take(properties);
					let mut cursor = args.walk();
					let mut children = args.named_children(&mut cursor);
					if let Some(first) = children.by_ref().next()
						&& let Some(tid) = self.type_of(first, scope, contents)
						&& let Type::DictBag(update_props) = _TR!(tid)
					{
						properties.extend(update_props.clone());
					}

					for named_arg in children {
						if named_arg.kind() == "keyword_argument"
							&& let Some(name) = named_arg.child_by_field_name("name")
							&& let Some(value) = named_arg.child_by_field_name("value")
						{
							let key = &contents[name.byte_range()];
							let type_ = self.type_of(value, scope, contents).unwrap_or_else(|| _T!(Type::Value));
							if let Some(idx) = properties.iter().position(|(prop, _)| match prop {
								DictKey::String(prop) => prop.as_str() == key,
								DictKey::Type(_) => false,
							}) {
								properties[idx].1 = type_;
							} else {
								properties.push((DictKey::String(ImStr::from(key)), type_));
							}
						} else if named_arg.kind() == "dictionary_splat"
							&& let Some(value) = named_arg.named_child(0)
							&& let Some(tid) = self.type_of(value, scope, contents)
							&& let Type::DictBag(update_props) = _TR!(tid)
						{
							properties.extend(update_props.clone());
						}
					}

					scope.insert(contents[map.byte_range()].to_string(), Type::DictBag(properties));
				}
			}
			"with_statement" => {
				// with Form(self.env['..']) as alias:
				// TODO: Support more structures as needed
				// (with_statement
				// 	 (with_clause
				//     (with_item
				//       (as_pattern
				//         (call (identifier) ..)
				//         (as_pattern_target (identifier))))))
				if let Some(value) = dig!(node, with_clause.with_item.as_pattern.call)
					&& let Some(target) = python_next_named_sibling(value)
					&& target.kind() == "as_pattern_target"
					&& let Some(alias) = dig!(target, identifier)
					&& let Some(callee) = value.named_child(0)
				{
					// TODO: Remove this hardcoded case
					if callee.kind() == "identifier"
						&& "Form" == &contents[callee.byte_range()]
						&& let Some(first_arg) = value.named_child(1).expect("call node must have argument_list").named_child(0)
						&& let Some(type_) = self.type_of(first_arg, scope, contents)
					{
						let alias = &contents[alias.byte_range()];
						scope.insert(alias.to_string(), _TR!(type_).clone());
					} else if let Some(type_) = self.type_of(value, scope, contents) {
						let alias = &contents[alias.byte_range()];
						scope.insert(alias.to_string(), _TR!(type_).clone());
					}
				}
			}
			"except_clause" => {
				// except ExceptionType as alias:
				// Structure with alias:
				//   (except_clause
				//     (as_pattern
				//       (identifier)           ; exception type
				//       (as_pattern_target
				//         (identifier)))       ; alias
				//     (block ...))
				// Structure without alias:
				//   (except_clause
				//     (identifier)             ; exception type
				//     (block ...))
				// Structure with tuple:
				//   (except_clause
				//     (as_pattern
				//       (tuple ...)            ; exception types
				//       (as_pattern_target
				//         (identifier)))       ; alias
				//     (block ...))
				if let Some(as_pattern) = dig!(node, as_pattern)
					&& let Some(as_pattern_target) = dig!(as_pattern, as_pattern_target(1))
					&& let Some(alias) = dig!(as_pattern_target, identifier)
				{
					let alias_name = &contents[alias.byte_range()];
					// Get the exception type(s)
					if let Some(exc_type) = as_pattern.named_child(0) {
						let type_ = match exc_type.kind() {
							"identifier" => {
								// Single exception type: `except ValueError as e:`
								let exc_name = &contents[exc_type.byte_range()];
								Type::PyBuiltin(exc_name.into())
							}
							"tuple" => {
								// Multiple exception types: `except (ValueError, KeyError) as e:`
								// Create a union of all exception types
								let mut cursor = exc_type.walk();
								let type_ids: Vec<TypeId> = exc_type
									.named_children(&mut cursor)
									.filter(|child| child.kind() == "identifier")
									.map(|child| {
										let exc_name = &contents[child.byte_range()];
										_T!(Type::PyBuiltin(exc_name.into()))
									})
									.collect();
							match type_cache().union(type_ids) {
								Some(tid) => type_cache().resolve(tid).clone(),
								None => Type::PyBuiltin("Exception".into()),
							}
							}
							_ => Type::PyBuiltin("Exception".into()),
						};
						scope.insert(alias_name.to_string(), type_);
					}
				}
			}
			_ => {}
		}

		ControlFlow::Continue(false)
	}
	/// [Type::Value] is not returned by this method.
	pub fn type_of(&self, mut node: Node, scope: &Scope, contents: &str) -> Option<TypeId> {
		// What contributes to value types?
		// 1. *.env['foo'] => Model('foo')
		// 2. *.env.ref(<record-id>) => Model(<model of record-id>)

		// What preserves value types?
		// 1. for foo in bar;
		//    bar: 't => foo: 't
		// 2. foo = bar;
		//    bar: 't => foo: 't (and various other operators)
		// 3. foo.sudo();
		//    foo: 't => foo.sudo(): 't
		//    sudo, with_user, with_env, with_context, ..
		// 4. [foo for foo in bar];
		//    bar: 't => foo: 't
		// 5: foo[..]
		//    foo: 't => foo[..]: 't

		// What transforms value types?
		// 1. foo.bar;
		//    foo: Model('t) => bar: Model('t).field('bar')
		// 2. foo.mapped('..')
		//    foo: Model('t) => _: Model('t).mapped('..')
		// 3. foo.mapped(lambda rec: 't): 't
		#[cfg(debug_assertions)]
		if node.byte_range().len() <= 64 {
			tracing::trace!("type_of {} '{}'", node.kind(), &contents[node.byte_range()]);
		} else {
			tracing::trace!("type_of {} range={:?}", node.kind(), node.byte_range());
		}
		match normalize(&mut node).kind() {
			"subscript" => {
				let lhs = node.child_by_field_name("value")?;
				let rhs = node.child_by_field_name("subscript")?;
				let obj_ty = self.type_of(lhs, scope, contents)?;
				match _TR!(obj_ty) {
					Type::Env if rhs.kind() == "string" => {
						Some(_T!(Type::Model(contents[rhs.byte_range().shrink(1)].into())))
					}
					Type::Env => Some(_T!["unknown"]),
					Type::Model(_) | Type::Record(_) => Some(obj_ty),
					Type::Dict(key, value) => {
						let rhs = self.type_of(rhs, scope, contents);
						// FIXME: We trust that the user makes the correct judgment here and returns the type requested.
						rhs.is_none_or(|rhs| rhs == *key).then_some(*value)
					}
					Type::DictBag(properties) => {
						// compare by key
						if let Some(rhs) = dig!(rhs, string_content(1)) {
							let rhs = &contents[rhs.byte_range()];
							for (key, value) in properties {
								match key {
									DictKey::String(key) if key.as_str() == rhs => {
										return Some(*value);
									}
									DictKey::String(_) | DictKey::Type(_) => {}
								}
							}
							return None;
						}

						// compare by type
						let rhs = self.type_of(rhs, scope, contents)?;
						for (key, value) in properties {
							match key {
								DictKey::Type(key) if *key == rhs => return Some(*value),
								DictKey::Type(_) | DictKey::String(_) => {}
							}
						}

						None
					}
					// FIXME: Again, just trust that the user is doing the right thing.
					Type::List(ListElement::Occupied(slot)) => Some(*slot),
					_ => None,
				}
			}
			"attribute" => self.type_of_attribute_node(node, scope, contents),
			"identifier" => {
				if let Some(parent) = node.parent()
					&& parent.kind() == "attribute"
					&& parent.named_child(0).unwrap() != node
				{
					return self.type_of_attribute_node(parent, scope, contents);
				}

				let key = &contents[node.byte_range()];
				if key == "super" {
					return Some(_T!(Type::Super));
				}
				if let Some(type_) = scope.get(key) {
					return Some(_T!(type_.clone()));
				}
				if key == "request" {
					return Some(_T!(Type::HttpRequest));
				}
				None
			}
			"assignment" => {
				let rhs = node.named_child(1)?;
				self.type_of(rhs, scope, contents)
			}
			"call" => self.type_of_call_node(node, scope, contents),
			"binary_operator" => {
				if let Some(left) = node.child_by_field_name("left")
					&& let Some(left) = self.type_of(left, scope, contents)
				{
					return Some(left);
				}

				self.type_of(node.child_by_field_name("right")?, scope, contents)
			}
			"boolean_operator" => {
				// For `or`: result could be either left or right
				// For `and`: if truthy returns right, if falsy returns left
				// In both cases, the result is a union of both types
				let left = node
					.child_by_field_name("left")
					.and_then(|n| self.type_of(n, scope, contents));
				let right = node
					.child_by_field_name("right")
					.and_then(|n| self.type_of(n, scope, contents));

				match (left, right) {
					(Some(l), Some(r)) => type_cache().union([l, r]),
					(Some(l), None) | (None, Some(l)) => Some(l),
					(None, None) => None,
				}
			}
			"conditional_expression" => {
				// a if b else c
				// In Python's AST: named_child(0) = consequence (a)
				//                  named_child(1) = condition (b)
				//                  named_child(2) = alternative (c)
				let then_ty = node
					.named_child(0)
					.and_then(|child| self.type_of(child, scope, contents));
				let else_ty = node
					.named_child(2)
					.and_then(|child| self.type_of(child, scope, contents));

				match (then_ty, else_ty) {
					(Some(a), Some(b)) => type_cache().union([a, b]),
					(Some(a), None) | (None, Some(a)) => Some(a),
					(None, None) => None,
				}
			}
			"dictionary_comprehension" => {
				let pair = dig!(node, pair)?;
				let mut comprehension_scope;
				let mut pair_scope = scope;
				if let Some(for_in_clause) = dig!(node, for_in_clause(1))
					&& let Some(scrutinee) = for_in_clause.child_by_field_name("left")
					&& let Some(iteratee) = for_in_clause.child_by_field_name("right")
					&& let Some(iter_ty) = self.type_of(iteratee, scope, contents)
					&& let Some(iter_ty) = self.type_of_iterable(iter_ty)
				{
					// FIXME: How to prevent this clone?
					comprehension_scope = Scope::new(Some(scope.clone()));
					self.destructure_into_patternlist_like(scrutinee, iter_ty, &mut comprehension_scope, contents);
					pair_scope = &comprehension_scope;
				}
				let lhs = pair
					.named_child(0)
					.and_then(|lhs| self.type_of(lhs, pair_scope, contents));
				let rhs = pair
					.named_child(1)
					.and_then(|lhs| self.type_of(lhs, pair_scope, contents));
				if lhs.is_some() || rhs.is_some() {
					let value_id = _T!(Type::Value);
					Some(_T!(Type::Dict(lhs.unwrap_or(value_id), rhs.unwrap_or(value_id))))
				} else {
					None
				}
			}
			"dictionary" => {
				let mut properties = vec![];
				for child in node.named_children(&mut node.walk()) {
					if child.kind() == "pair"
						&& let Some(lhs) = child.child_by_field_name("key")
						&& let Some(rhs) = child.child_by_field_name("value")
					{
						let key;
						if let Some(lhs) = dig!(lhs, string_content(1)) {
							key = DictKey::String(ImStr::from(&contents[lhs.byte_range()]));
						} else if matches!(lhs.kind(), "true" | "false" | "string" | "none" | "float" | "integer") {
							key = DictKey::Type(_T!( @contents[lhs.byte_range()]));
						} else if let Some(lhs) = self.type_of(lhs, scope, contents) {
							key = DictKey::Type(lhs);
						} else {
							continue;
						}

						let value = self.type_of(rhs, scope, contents).unwrap_or_else(|| _T!(Type::Value));
						properties.push((key, value));
					}
				}
				Some(_T!(Type::DictBag(properties)))
			}
			"list" => {
				let mut slot = ListElement::Vacant;
				for child in node.named_children(&mut node.walk()) {
					if let Some(child) = self.type_of(child, scope, contents) {
						slot = ListElement::Occupied(child);
						break;
					}
				}
				Some(_T!(Type::List(slot)))
			}
			"expression_list" | "tuple" => {
				let mut cursor = node.walk();
				let value_id = _T!(Type::Value);
				let tuple = node.named_children(&mut cursor).filter_map(|child| {
					if child.kind() == "comment" {
						return None;
					}
					Some(self.type_of(child, scope, contents).unwrap_or(value_id))
				});
				Some(_T!(Type::Tuple(tuple.collect())))
			}
			"string" => Some(_T!( @ "str")),
			"integer" => Some(_T!( @ "int")),
			"float" => Some(_T!( @ "float")),
			"true" | "false" | "comparison_operator" => Some(_T!( @ "bool")),
			"none" => Some(_T!(Type::None)),
			_ => None,
		}
	}
	pub(crate) fn type_of_iterable(&self, tid: TypeId) -> Option<TypeId> {
		match _TR!(tid) {
			Type::Model(_) => Some(tid),
			Type::List(inner) => inner.clone().into(),
			Type::Iterable(inner) => *inner,
			Type::Tuple(elements) => {
				// Union of all tuple element types
				type_cache().union(elements.clone())
			}
			Type::Union(types) => {
				// Union of iterable element types from each union member
				let element_types: Vec<TypeId> = types
					.iter()
					.filter_map(|tid| self.type_of_iterable(*tid))
					.collect();
				type_cache().union(element_types)
			}
			_ => None,
		}
	}
	fn wrap_in_container<F: FnOnce(Type) -> Type>(type_: Type, producer: F) -> Type {
		match type_ {
			Type::Model(..) => type_,
			_ => producer(type_),
		}
	}
	fn type_of_call_node(&self, call: Node<'_>, scope: &Scope, contents: &str) -> Option<TypeId> {
		let func = call.named_child(0)?;
		if func.kind() == "identifier" {
			match &contents[func.byte_range()] {
				"zip" => {
					let args = call.named_child(1)?;
					let mut cursor = args.walk();
					let value_id = _T!(Type::Value);
					let children = args.named_children(&mut cursor).map(|child| {
						let tid = self.type_of(child, scope, contents).unwrap_or(value_id);
						self.type_of_iterable(tid).unwrap_or(value_id)
					});
					let tuple = _T!(Type::Tuple(children.collect()));
					return Some(_T!(Type::Iterable(Some(tuple))));
				}
				"enumerate" => {
					let arg = call.named_child(1)?.named_child(0);
					let arg = arg
						.and_then(|arg| self.type_of(arg, scope, contents))
						.unwrap_or_else(|| _T!(Type::Value));
					let intid = _T!(Type::PyBuiltin("int".into()));
					let tuple = _T!(Type::Tuple(vec![intid, arg]));
					return Some(_T!(Type::Iterable(Some(tuple))));
				}
				"tuple" => {
					let args = call.named_child(1)?;
					if args.kind() == "argument_list" {
						let mut cursor = args.walk();
						let value_id = _T!(Type::Value);
						let children = args
							.named_children(&mut cursor)
							.map(|child| self.type_of(child, scope, contents).unwrap_or(value_id));
						return Some(_T!(Type::Tuple(children.collect())));
					}
				}
				"defaultdict" => {
					let arg = call.named_child(1)?.named_child(0)?;
					if matches!(&contents[arg.byte_range()], "list" | "dict" | "float" | "int") {
						// TODO: consider more general functions and type ctors
						return Some(_T!(Type::Dict(
							_T!(Type::Value),
							_T!(Type::PyBuiltin(contents[arg.byte_range()].into()))
						)));
					}
					if arg.kind() != "lambda" {
						return Some(_T!(Type::Dict(_T!(Type::Value), _T!(Type::Value))));
					}
					// (lambda body: (_))
					let body = arg.child_by_field_name("body")?;
					let body_ty = self.type_of(body, scope, contents).unwrap_or_else(|| _T!(Type::Value));
					return Some(_T!(Type::Dict(_T!(Type::Value), body_ty)));
				}
				"super" => {}
				func_name => {
					// Try to look up as a module-level function
					use crate::model::Function;
					let func_sym: Symbol<Function> = _I(func_name).into();

					// First, try to find in the current file
					if let Some(current_path) = scope.current_path {
						if let Some(funcs) = self.functions.get(&current_path) {
							if funcs.contains_key(&func_sym) {
								// Found function in current file - evaluate its return type
								if let Some(rtype) = self.eval_function_rtype(func_sym, current_path) {
									return Some(rtype);
								}
							}
						}
					}

					// Check if this is an imported function
					if let Some(import_info) = scope.get_import(func_name) {
						// Resolve the import to a file path
						if let Some(file_path) = self.resolve_py_module(&import_info.module_path) {
							let import_func_sym: Symbol<Function> = _I(&import_info.imported_name).into();
							if let Some((path_sym, _)) = self.functions.find_in_file(&file_path, &import_func_sym) {
								if let Some(rtype) = self.eval_function_rtype(import_func_sym, path_sym) {
									return Some(rtype);
								}
							}
						}
					}

					// Fall back to global search
					if let Some((path, _)) = self.functions.find_by_name(&func_sym) {
						if let Some(rtype) = self.eval_function_rtype(func_sym, path) {
							return Some(rtype);
						}
					}

					return None;
				}
			};
		}

		let func = self.type_of(func, scope, contents)?;
		match _TR!(func) {
			Type::RefFn => {
				// (call (_) @func (argument_list . (string) @xml_id))
				let xml_id = call.named_child(1)?.named_child(0)?;
				if xml_id.kind() == "string" {
					Some(_T!(Type::Record(contents[xml_id.byte_range().shrink(1)].into())))
				} else {
					None
				}
			}
			Type::ModelFn(model) => Some(_T!(Type::Model(model.clone()))),
			Type::Super => Some(_T!(scope.get(scope.super_.as_deref()?).cloned()?)),
			Type::Method(model, mapped) if mapped.as_str() == "mapped" => {
				// (call (_) @func (argument_list . [(string) (lambda)] @mapped))
				let mapped = call.named_child(1)?.named_child(0)?;
				match mapped.kind() {
					"string" => {
						let mut model: Spur = (*model).into();
						let mut mapped = &contents[mapped.byte_range().shrink(1)];
						self.models.resolve_mapped(&mut model, &mut mapped, None).ok()?;
						let type_ = self.type_of_attribute(&Type::Model(_R(model).into()), mapped, scope)?;
						let type_ = Index::wrap_in_container(type_, |it| Type::List(ListElement::Occupied(_T!(it))));
						Some(_T!(type_))
					}
					"lambda" => {
						// (lambda (lambda_parameters)? body: (_))
						let mut scope = Scope::new(Some(scope.clone()));
						if let Some(params) = mapped.child_by_field_name(b"parameters") {
							let first_arg = params.named_child(0)?;
							if first_arg.kind() == "identifier" {
								let first_arg = &contents[first_arg.byte_range()];
								scope.insert(first_arg.to_string(), Type::Model(_R(model).into()));
							}
						}
						let body = mapped.child_by_field_name(b"body")?;
						let type_ = self.type_of(body, &scope, contents).unwrap_or_else(|| _T!(Type::Value));
						let type_ = Index::wrap_in_container(_TR!(type_).clone(), |it| {
							Type::List(ListElement::Occupied(_T!(it)))
						});
						Some(_T!(type_))
					}
					_ => None,
				}
			}
			Type::Method(model, grouped) if grouped.as_str() == "grouped" => {
				// (call (_) @func (argument_list . [(string) (lambda)] @mapped))
				let grouped = call.named_child(1)?.named_child(0)?;
				match grouped.kind() {
					"string" => {
						let mut model: Spur = (*model).into();
						let mut grouped = &contents[grouped.byte_range().shrink(1)];
						self.models.resolve_mapped(&mut model, &mut grouped, None).ok()?;
						let model = Type::Model(_R(model).into());
						let groupby = self.type_of_attribute(&model, grouped, scope)?;
						Some(_T!(Type::Dict(_T!(groupby), _T!(model))))
					}
					"lambda" => {
						let mut scope = Scope::new(Some(scope.clone()));
						if let Some(params) = grouped.child_by_field_name(b"parameters") {
							let first_arg = params.named_child(0)?;
							if first_arg.kind() == "identifier" {
								let first_arg = &contents[first_arg.byte_range()];
								scope.insert(first_arg.to_string(), Type::Model(_R(model).into()));
							}
						}
						let body = grouped.child_by_field_name(b"body")?;
						let groupby = self.type_of(body, &scope, contents).unwrap_or_else(|| _T!(Type::Value));
						let model = Type::Model(_R(model).into());
						Some(_T!(Type::Dict(groupby, _T!(model))))
					}
					_ => None,
				}
			}
			Type::Method(model, read_group) if read_group.as_str() == "_read_group" => {
				let mut groupby = vec![];
				let mut aggs = vec![];
				let args = call.named_child(1)?;

				fn gather_attributes<'out>(contents: &'out str, arg: Node, out: &mut Vec<&'out str>) {
					let mut cursor = arg.walk();
					for field in arg.named_children(&mut cursor) {
						if let Some(field) = dig!(field, string_content(1)) {
							let mut field = &contents[field.byte_range()];
							if let Some((inner, _)) = field.split_once(':') {
								field = inner;
							}
							out.push(field);
						}
					}
				}

				for (idx, arg) in args.named_children(&mut args.walk()).enumerate().take(3) {
					if arg.kind() == "keyword_argument" {
						let out = match &contents[arg.child_by_field_name("key")?.byte_range()] {
							"groupby" => &mut groupby,
							"aggregates" => &mut aggs,
							_ => continue,
						};
						let Some(arg) = arg.child_by_field_name("value") else {
							continue;
						};
						if arg.kind() != "list" {
							continue;
						}
						gather_attributes(contents, arg, out);
						continue;
					}

					if arg.kind() != "list" || idx > 2 || idx == 0 {
						continue;
					}

					let out = if idx == 1 { &mut groupby } else { &mut aggs };
					gather_attributes(contents, arg, out);
				}

				groupby.extend(aggs);
				groupby.dedup();
				let model = Type::Model(_R(*model).into());
				let value_id = _T!(Type::Value);
				// FIXME: This is not quite correct as only recordset and numeric aggregations make sense.
				let aggs = groupby
					.into_iter()
					.map(|attr| match self.type_of_attribute(&model, attr, scope) {
						Some(type_) => _T!(type_),
						None => value_id,
					});
				let tuple = _T!(Type::Tuple(aggs.collect()));
				Some(_T!(Type::List(ListElement::Occupied(tuple))))
			}
			Type::Method(model, method) => {
				let method = _G(method)?;
				let args = self.prepare_call_scope(*model, method.into(), call, scope, contents);
				Some(self.eval_method_rtype(method.into(), **model, args)?)
			}
			Type::PythonMethod(dict, method) if dict.is_dict() => {
				let Type::Dict(lhs, rhs) = _TR!(dict) else {
					unreachable!()
				};
				match method.as_str() {
					"items" => {
						let tuple = _T!(Type::Tuple(vec![*lhs, *rhs]));
						Some(_T!(Type::Iterable(Some(tuple))))
					}
					"get" => Some(*rhs),
					_ => None,
				}
			}
			Type::PythonMethod(dictbag, method) if dictbag.is_dictbag() => {
				let Type::DictBag(items) = _TR!(dictbag) else {
					unreachable!()
				};
				match method.as_str() {
					"get" => {
						let args = call.named_child(1)?;
						let arg_as_string = dig!(args, string.string_content(1));
						let argtype = args
							.named_child(0)
							.and_then(|node| self.type_of(node, scope, contents))
							.unwrap_or_else(|| _T!(Type::Value));
						items.iter().find_map(|(key, val)| match key {
							DictKey::String(key) => match arg_as_string {
								None => None,
								Some(arg) => (key.as_str() == &contents[arg.byte_range()]).then_some(*val),
							},
							DictKey::Type(key) => (*key == argtype).then_some(*val),
						})
					}
					_ => None,
				}
			}
			Type::Env
			| Type::Record(..)
			| Type::Model(..)
			| Type::HttpRequest
			| Type::Value
			| Type::PyBuiltin(..)
			| Type::Dict(..)
			| Type::DictBag(..)
			| Type::List(..)
			| Type::Iterable(..)
			| Type::Tuple(..)
			| Type::PythonMethod(..)
			| Type::Union(..)
			| Type::Function(..)
			| Type::None => None,
		}
	}

	#[instrument(skip_all, fields(model, method))]
	fn prepare_call_scope(
		&self,
		model: ModelName,
		method: Symbol<Method>,
		call: Node,
		scope: &Scope,
		contents: &str,
	) -> Option<(Vec<ImStr>, Scope)> {
		// (call
		//   (arguments_list
		//     (_)
		//     (keyword_argument (identifier) (_))))
		let arguments_list = dig!(call, argument_list(1))?;

		let model = self.models.populate_properties(model, &[])?;
		let method = model.methods.as_ref()?.get(&method)?;
		let arguments = method.arguments.clone().unwrap_or_default();
		if arguments.is_empty() {
			return None;
		}

		drop(model);
		let mut argtypes = Scope::new(None);
		let mut args = vec![];
		for (idx, arg) in arguments_list.named_children(&mut arguments_list.walk()).enumerate() {
			if arg.kind() == "keyword_argument"
				&& let Some(key) = arg.child_by_field_name("key")
				&& let Some(value) = arg.child_by_field_name("value")
			{
				let key = &contents[key.byte_range()];
				if !arguments.iter().any(|arg| match arg {
					FunctionParam::Named(arg) => arg.as_str() == key,
					_ => false,
				}) {
					continue;
				}
				let Some(tid) = self.type_of(value, scope, contents) else {
					continue;
				};
				args.push(key.into());
				argtypes.insert(key.to_string(), _TR!(tid).clone());
			} else if let Some(FunctionParam::Param(argname)) = arguments.get(idx)
				&& let Some(tid) = self.type_of(arg, scope, contents)
			{
				args.push(argname.clone());
				argtypes.insert(argname.to_string(), _TR!(tid).clone());
			} else {
				continue;
			}
		}

		Some((args, argtypes))
	}
	#[instrument(skip_all, ret)]
	fn type_of_attribute_node(&self, attribute: Node<'_>, scope: &Scope, contents: &str) -> Option<TypeId> {
		let lhs = attribute.named_child(0)?;
		let lhsid = self.type_of(lhs, scope, contents)?;
		let lhs = _TR!(lhsid);
		let rhs = attribute.named_child(1)?;
		let attrname = &contents[rhs.byte_range()];
		match &contents[rhs.byte_range()] {
			"env" if matches!(lhs, Type::Model(..) | Type::Record(..) | Type::HttpRequest) => Some(Type::Env),
			"website" if matches!(lhs, Type::HttpRequest) => Some(Type::Model("website".into())),
			"ref" if matches!(lhs, Type::Env) => Some(Type::RefFn),
			"user" if matches!(lhs, Type::Env) => Some(Type::Model("res.users".into())),
			"company" | "companies" if matches!(lhs, Type::Env) => Some(Type::Model("res.company".into())),
			"uid" if matches!(lhs, Type::Env) => Some(Type::PyBuiltin("int".into())),
			"lang" if matches!(lhs, Type::Env) => Some(Type::Union(vec![
				_T!(Type::PyBuiltin("str".into())),
				_T!(Type::None),
			])),
			"su" if matches!(lhs, Type::Env) => Some(Type::PyBuiltin("bool".into())),
			"cr" if matches!(lhs, Type::Env) => Some(Type::PyBuiltin("Cursor".into())),
			"context" if matches!(lhs, Type::Env) => Some(Type::PyBuiltin("dict".into())),
			"mapped" | "grouped" | "_read_group" | "read" => {
				let model = self.try_resolve_model(lhs, scope)?;
				Some(Type::Method(model, attrname.into()))
			}
			dict_method @ ("items" | "get") if lhs.is_dictlike() => Some(Type::PythonMethod(lhsid, dict_method.into())),
			func if MODEL_METHODS.contains(func) => match lhs {
				Type::Model(model) => Some(Type::ModelFn(model.clone())),
				Type::Record(xml_id) => {
					let xml_id = _G(xml_id)?;
					let record = self.records.get(&xml_id)?;
					Some(Type::ModelFn(_R(*record.model.as_deref()?).into()))
				}
				_ => None,
			},
			ident if rhs.kind() == "identifier" => self.type_of_attribute(lhs, ident, scope),
			_ => None,
		}
		.map(|it| _T!(it))
	}
	#[instrument(skip_all, fields(attr=attr), ret)]
	pub fn type_of_attribute(&self, type_: &Type, attr: &str, scope: &Scope) -> Option<Type> {
		let model = self.try_resolve_model(type_, scope)?;
		let model_entry = self.models.populate_properties(model, &[])?;
		if let Some(attr_key) = _G(attr)
			&& let Some(attr_kind) = model_entry.prop_kind(attr_key)
		{
			match attr_kind {
				PropertyInfo::Field(type_) => {
					drop(model_entry);
					if let Some(relation) = self.models.resolve_related_field(attr_key.into(), model.into()) {
						return Some(Type::Model(_R(relation).into()));
					}

					match _R(type_) {
						"Selection" | "Char" | "Text" | "Html" => Some(Type::PyBuiltin("str".into())),
						"Integer" => Some(Type::PyBuiltin("int".into())),
						"Float" | "Monetary" => Some(Type::PyBuiltin("float".into())),
						"Date" => Some(Type::PyBuiltin("date".into())),
						"Datetime" => Some(Type::PyBuiltin("datetime".into())),
						_ => None,
					}
				}
				PropertyInfo::Method => Some(Type::Method(model, attr.into())),
			}
		} else {
			match attr {
				"id" if matches!(type_, Type::Model(..) | Type::Record(..)) => Some(Type::PyBuiltin("int".into())),
				"ids" if matches!(type_, Type::Model(..) | Type::Record(..)) => {
					Some(Type::List(ListElement::Occupied(_T!(Type::PyBuiltin("int".into())))))
				}
				"display_name" if matches!(type_, Type::Model(..) | Type::Record(..)) => {
					Some(Type::PyBuiltin("str".into()))
				}
				"create_date" | "write_date" if matches!(type_, Type::Model(..) | Type::Record(..)) => {
					Some(Type::PyBuiltin("datetime".into()))
				}
				"create_uid" | "write_uid" if matches!(type_, Type::Model(..) | Type::Record(..)) => {
					Some(Type::Model("res.users".into()))
				}
				"_fields" if matches!(type_, Type::Model(..) | Type::Record(..)) => {
					Some(Type::Dict(_T!(Type::PyBuiltin("str".into())), _T!["ir.model.fields"]))
				}
				"env" if matches!(type_, Type::Model(..) | Type::Record(..) | Type::HttpRequest) => Some(Type::Env),
				_ => None,
			}
		}
	}
	pub fn has_attribute(&self, type_: &Type, attr: &str, scope: &Scope) -> bool {
		(|| -> Option<()> {
			let model = self.try_resolve_model(type_, scope)?;
			let entry = self.models.populate_properties(model, &[])?;
			let attr = _G(attr)?;
			entry.prop_kind(attr).map(|_| ())
		})()
		.is_some()
	}
	/// Call this method if it's unclear whether `type_` is a [`Type::Model`] and you just want the model's name.
	pub fn try_resolve_model(&self, type_: &Type, scope: &Scope) -> Option<ModelName> {
		match type_ {
			Type::Model(model) => Some(_G(model)?.into()),
			Type::Record(xml_id) => {
				// TODO: Refactor into method
				let xml_id = _G(xml_id)?;
				let record = self.records.get(&xml_id)?;
				record.model
			}
			Type::Super => self.try_resolve_model(scope.get(scope.super_.as_deref()?)?, scope),
			Type::Union(types) => {
				// Return the first model found in the union
				// This is useful for completions - we show fields from any possible model
				for tid in types {
					if let Some(model) = self.try_resolve_model(&type_cache().resolve(*tid), scope) {
						return Some(model);
					}
				}
				None
			}
			_ => None,
		}
	}

	// ========== Call Hierarchy: Call Collection ==========

	/// Process a call node and add it to the call graph if we can resolve the callee.
	fn collect_call(
		&self,
		call: Node,
		scope: &Scope,
		caller: &crate::call_graph::CallableId,
		contents: &str,
		file_path: PathSymbol,
	) {
		use crate::call_graph::{CallType, CallableId};
		use crate::utils::MinLoc;

		let Some(func) = call.named_child(0) else {
			return;
		};

		let call_location = MinLoc {
			path: file_path,
			range: crate::utils::span_conv(call.range()),
		};

		match func.kind() {
			"identifier" => {
				let func_name = &contents[func.byte_range()];
				// Skip "super" - it's handled as attribute call super().method()
				if func_name == "super" {
					return;
				}

				// Try to resolve as function call
				if let Some(callee) = self.resolve_function_callee(func_name, scope, file_path) {
					self.call_graph.add_call(
						Some(caller.clone()),
						callee,
						call_location,
						CallType::Direct,
					);
				}
			}
			"attribute" => {
				// Method call: obj.method()
				let Some(method_node) = func.child_by_field_name("attribute") else {
					return;
				};
				let Some(obj_node) = func.child_by_field_name("object") else {
					return;
				};
				let method_name = &contents[method_node.byte_range()];

				// Check for super().method() pattern
				let is_super_call = obj_node.kind() == "call"
					&& obj_node
						.named_child(0)
						.is_some_and(|id| id.kind() == "identifier" && &contents[id.byte_range()] == "super");

				if is_super_call {
					// Super call - resolve via scope.super_ and ancestors
					if let Some(callee) = self.resolve_super_callee(method_name, scope) {
						self.call_graph.add_call(
							Some(caller.clone()),
							callee,
							call_location,
							CallType::Super,
						);
					}
				} else {
					// Regular method call - resolve object type
					if let Some(tid) = self.type_of(obj_node, scope, contents) {
						let type_ = type_cache().resolve(tid);
						if let Some(model) = self.try_resolve_model(&type_, scope) {
							let callee = CallableId::method(_R(model), method_name);
							self.call_graph.add_call(
								Some(caller.clone()),
								callee,
								call_location,
								CallType::Direct,
							);
						}
					}
				}
			}
			_ => {}
		}
	}

	/// Resolve a function name to a CallableId.
	fn resolve_function_callee(
		&self,
		name: &str,
		scope: &Scope,
		current_path: PathSymbol,
	) -> Option<crate::call_graph::CallableId> {
		use crate::call_graph::CallableId;
		use crate::model::Function;

		let func_sym: Symbol<Function> = _I(name).into();

		// 1. Check current file
		if let Some(funcs) = self.functions.get(&current_path) {
			if funcs.contains_key(&func_sym) {
				return Some(CallableId::function(current_path.as_string(), name));
			}
		}

		// 2. Check imports
		if let Some(import_info) = scope.get_import(name) {
			if let Some(file_path) = self.resolve_py_module(&import_info.module_path) {
				let import_sym: Symbol<Function> = _I(&import_info.imported_name).into();
				if let Some((path_sym, _)) = self.functions.find_in_file(&file_path, &import_sym) {
					return Some(CallableId::function(
						path_sym.as_string(),
						&import_info.imported_name,
					));
				}
			}
		}

		// 3. Global search (fallback)
		if let Some((path, _)) = self.functions.find_by_name(&func_sym) {
			return Some(CallableId::function(path.as_string(), name));
		}

		None
	}

	/// Resolve a super().method() call to a CallableId.
	fn resolve_super_callee(&self, method_name: &str, scope: &Scope) -> Option<crate::call_graph::CallableId> {
		use crate::call_graph::CallableId;

		// Get the current model from scope
		let super_param = scope.super_.as_deref()?;
		let current_type = scope.get(super_param)?;

		let model_name = match current_type {
			Type::Model(name) => name.clone(),
			_ => return None,
		};

		// Look up the model to find its ancestors
		let model_key = _G(&*model_name)?;
		let entry = self.models.get(&model_key)?;

		// The first ancestor is the primary parent for super() calls
		let parent_model = entry.ancestors.first()?;
		let parent_name = _R(*parent_model);

		Some(CallableId::method(parent_name, method_name))
	}

	/// Collect calls from module-level code (outside functions/methods).
	/// This handles calls at the top level of a Python file.
	pub fn collect_module_level_calls(&self, path: PathSymbol, contents: &str, ast: &tree_sitter::Tree) {
		use crate::utils::PreTravel;

		let imports = std::sync::Arc::new(parse_imports(ast.root_node(), contents));
		let mut scope = Scope::default();
		scope.current_path = Some(path);
		scope.imports = Some(imports);

		// Traverse module-level statements
		let module = ast.root_node();

		for node in PreTravel::new(module) {
			if !node.is_named() {
				continue;
			}

			// Skip function and class definitions - they have their own scope
			if matches!(
				node.kind(),
				"function_definition" | "class_definition" | "decorated_definition"
			) {
				continue;
			}

			// For call expressions at module level, collect them with no caller
			if node.kind() == "call" {
				// Check if this call is at module level (not inside a function/class)
				let is_module_level = {
					let mut current = node;
					let mut at_module_level = true;
					while let Some(parent) = current.parent() {
						if matches!(
							parent.kind(),
							"function_definition" | "class_definition"
						) {
							at_module_level = false;
							break;
						}
						current = parent;
					}
					at_module_level
				};

				if is_module_level {
					self.collect_call_optional_caller(node, &scope, None, contents, path);
				}
			}
		}
	}

	/// Process a call node with an optional caller (for module-level calls).
	fn collect_call_optional_caller(
		&self,
		call: Node,
		scope: &Scope,
		caller: Option<&crate::call_graph::CallableId>,
		contents: &str,
		file_path: PathSymbol,
	) {
		use crate::call_graph::{CallType, CallableId};
		use crate::utils::MinLoc;

		let Some(func) = call.named_child(0) else {
			return;
		};

		let call_location = MinLoc {
			path: file_path,
			range: crate::utils::span_conv(call.range()),
		};

		match func.kind() {
			"identifier" => {
				let func_name = &contents[func.byte_range()];
				if func_name == "super" {
					return;
				}

				if let Some(callee) = self.resolve_function_callee(func_name, scope, file_path) {
					self.call_graph.add_call(
						caller.cloned(),
						callee,
						call_location,
						CallType::Direct,
					);
				}
			}
			"attribute" => {
				let Some(method_node) = func.child_by_field_name("attribute") else {
					return;
				};
				let Some(obj_node) = func.child_by_field_name("object") else {
					return;
				};
				let method_name = &contents[method_node.byte_range()];

				// At module level, we likely have things like odoo.api calls
				// Try to resolve the object type
				if let Some(tid) = self.type_of(obj_node, scope, contents) {
					let type_ = type_cache().resolve(tid);
					if let Some(model) = self.try_resolve_model(&type_, scope) {
						let callee = CallableId::method(_R(model), method_name);
						self.call_graph.add_call(
							caller.cloned(),
							callee,
							call_location,
							CallType::Direct,
						);
					}
				}
			}
			_ => {}
		}
	}

	#[inline]
	pub fn type_display(&self, type_: TypeId) -> Option<String> {
		self.type_display_indent(type_, 0)
	}
	fn type_display_indent(&self, type_: TypeId, indent: usize) -> Option<String> {
		match _TR!(type_) {
			Type::Dict(lhs, rhs) => {
				let lhs = self.type_display_indent(*lhs, indent);
				let lhs = lhs.as_deref().unwrap_or("...");
				let rhs = self.type_display_indent(*rhs, indent);
				let rhs = rhs.as_deref().unwrap_or("...");
				Some(fomat! { "dict[" (lhs) ", " (rhs) "]" })
			}
			Type::DictBag(properties) => {
				let preindent = " ".repeat(indent + 2);
				let empty_properties = properties.is_empty();
				let properties_fragment = fomat! {
					for (key, value) in properties {
						(preindent)
						match key {
							DictKey::String(key) => { "\"" (key) "\"" }
							DictKey::Type(key) if key.is_dictlike() => { "{...}" }
							DictKey::Type(key) => { (self.type_display_indent(*key, indent + 2).as_deref().unwrap_or("...")) }
						} ": " (self.type_display_indent(*value, indent + 2).as_deref().unwrap_or("..."))
					} sep { ",\n" }
				};
				let unindent = " ".repeat(indent);
				Some(fomat! {
					if !empty_properties {
						"{\n" (properties_fragment) "\n" (unindent) "}"
					} else {
						"{}"
					}
				})
			}
			Type::PyBuiltin(builtin) => Some(builtin.as_str().into()),
			Type::List(slot) => {
				let slot = match slot {
					ListElement::Vacant => None,
					ListElement::Occupied(slot) => self.type_display_indent(*slot, indent),
				};
				Some(match slot {
					Some(slot) => format!("list[{slot}]"),
					None => "list".into(),
				})
			}
			Type::Env => Some("Environment".into()),
			Type::Model(model) => Some(format!(r#"Model["{model}"]"#)),
			Type::Record(xml_id) => {
				let xml_id = _G(xml_id)?;
				let record = self.records.get(&xml_id)?;
				Some(_R(record.model?).into())
			}
			Type::Tuple(items) => Some(fomat! {
				"tuple["
				for item in items {
					(self.type_display_indent(*item, indent).as_deref().unwrap_or("..."))
				} sep { ", " }
				"]"
			}),
			Type::Iterable(output) => {
				let output = output.and_then(|inner| self.type_display_indent(inner, indent));
				let output = output.as_deref().unwrap_or("...");
				Some(format!("Iterable[{output}]"))
			}
			Type::Union(types) => {
				let mut parts: Vec<String> = types
					.iter()
					.filter_map(|t| self.type_display_indent(*t, indent))
					.collect();
				if parts.is_empty() {
					None
				} else {
					// Sort for canonical representation (deterministic output)
					parts.sort();
					Some(parts.join(" | "))
				}
			}
			Type::Method(model, method) => Some(format!("Method[{}, {}]", _R(model), method)),
			Type::Function(path, name) => Some(format!("Function[{}, {}]", path, _R(name))),
			Type::None => Some("None".into()),
			Type::RefFn | Type::ModelFn(_) | Type::Super | Type::HttpRequest | Type::Value | Type::PythonMethod(..) => {
				if cfg!(debug_assertions) {
					Some(format!("{type_:?}"))
				} else {
					None
				}
			}
		}
	}
	/// Iterates depth-first over `node` using [`PreTravel`]. Automatically calls [`Scope::exit`] at suitable points.
	///
	/// [`ControlFlow::Continue`] accepts a boolean to indicate whether [`Scope::enter`] was called.
	///
	/// To accumulate bindings into a scope, use [`Index::build_scope`].
	pub fn walk_scope<T>(
		node: Node,
		scope: Option<Scope>,
		mut step: impl FnMut(&mut Scope, Node) -> ControlFlow<Option<T>, bool>,
	) -> (Scope, Option<T>) {
		let mut scope = scope.unwrap_or_default();
		let mut scope_ends = vec![];
		for node in PreTravel::new(node) {
			if !node.is_named() {
				continue;
			}
			if let Some(&end) = scope_ends.last()
				&& node.start_byte() > end
			{
				scope.exit();
				scope_ends.pop();
			}
			match step(&mut scope, node) {
				ControlFlow::Break(value) => return (scope, value),
				ControlFlow::Continue(entered) => {
					if entered {
						scope_ends.push(node.end_byte());
					}
				}
			}
		}
		(scope, None)
	}
	/// Resolves the return type of a method as well as populating its arguments and docstring.
	///
	/// `parameters` can be provided using [`Index::prepare_call_scope`].  
	#[instrument(level = "trace", ret, skip(self, model), fields(model = _R(model)))]
	pub fn eval_method_rtype(
		&self,
		method: Symbol<Method>,
		model: Spur,
		parameters: Option<(Vec<ImStr>, Scope)>,
	) -> Option<TypeId> {
		_ = self.models.populate_properties(model.into(), &[]);
		let mut model_entry = self.models.try_get_mut(&model).expect(format_loc!("deadlock"))?;
		let method_obj = model_entry.methods.as_mut()?.get_mut(&method)?;

		if method_obj
			.pending_eval
			.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
			.is_err()
		{
			return None;
		}

		let _guard = Defer(Some(|| {
			if let Some(model_entry) = self.models.get_mut(&model)
				&& let Some(methods) = model_entry.methods.as_ref()
				&& let Some(method) = methods.get(&method)
			{
				method.pending_eval.store(false, Ordering::Relaxed);
			}
		}));

		let (argnames, mut scope) = parameters.unwrap_or_default();
		let cache_key = argnames
			.into_iter()
			.map(|arg| _T!(scope.get(&*arg).cloned().unwrap_or(Type::Value)))
			.collect::<Vec<_>>();
		if let Some(tid) = method_obj.eval_cache.get(&cache_key) {
			drop(model_entry);
			return Some(tid);
		}

		let location = method_obj.locations.first().cloned()?;
		drop(model_entry);

		let ast;
		let contents;
		let end_offset: ByteOffset;
		let path = location.path.to_path();
		if let Some(cached) = self.ast_cache.get(&path) {
			end_offset = rope_conv(location.range.end, cached.rope.slice(..));
			ast = cached.tree.clone();
			contents = String::from(cached.rope.clone());
		} else {
			contents = test_utils::fs::read_to_string(location.path.to_path()).unwrap();
			let rope = Rope::from_str(&contents);
			end_offset = rope_conv(location.range.end, rope.slice(..));
			let mut parser = python_parser();
			ast = parser.parse(contents.as_bytes(), None)?;
			self.ast_cache.insert(
				path,
				Arc::new(crate::index::AstCacheItem {
					tree: ast.clone(),
					rope,
				}),
			);
		}

		/// Checks if a return statement belongs to the given function scope,
		/// not to a nested function definition.
		fn is_return_in_function(return_node: Node, fn_scope: Node) -> bool {
			let mut current = return_node;
			while let Some(parent) = current.parent() {
				if parent.id() == fn_scope.id() {
					return true;
				}
				// If we hit another function_definition before our target, this return belongs to it
				if parent.kind() == "function_definition" && parent.id() != fn_scope.id() {
					return false;
				}
				current = parent;
			}
			false
		}

		let (self_type, fn_scope, self_param) = determine_scope(ast.root_node(), &contents, end_offset.0)?;
		let self_type = match self_type {
			Some(type_) => &contents[type_.byte_range().shrink(1)],
			None => "",
		};
		scope.super_ = Some(self_param.into());
		scope.insert(self_param.to_string(), Type::Model(self_type.into()));
		scope.current_path = Some(location.path);
		
		// Parse imports for resolving function calls
		let imports = std::sync::Arc::new(parse_imports(ast.root_node(), &contents));
		scope.imports = Some(imports);
		
		let offset = fn_scope.end_byte();

		// Create caller ID for call graph
		let caller = crate::call_graph::CallableId::method(_R(model), _R(method));
		let call_path = location.path;

		// Collect ALL return types instead of breaking on the first one
		let mut return_types: Vec<TypeId> = Vec::new();

		let _ = Self::walk_scope(fn_scope, Some(scope), |scope, node| {
			let entered = self.build_scope(scope, node, offset, &contents).map_break(|_| None::<()>)?;

			if node.kind() == "return_statement" && is_return_in_function(node, fn_scope) {
				if let Some(child) = node.named_child(0)
					&& let Some(type_) = self.type_of(child, scope, &contents)
				{
					let type_ = type_cache().resolve(type_);
					let resolved = match self.try_resolve_model(type_, scope) {
						Some(model) => _T!(Type::Model(ImStr::from(_R(model)))),
						None => _T!(type_.clone()),
					};
					return_types.push(resolved);
				}
			}

			// Collect calls for call hierarchy
			if node.kind() == "call" && is_return_in_function(node, fn_scope) {
				self.collect_call(node, scope, &caller, &contents, call_path);
			}

			ControlFlow::Continue(entered)
		});

		let mut model = self.models.try_get_mut(&model).expect(format_loc!("deadlock"))?;
		let method = Arc::make_mut(model.methods.as_mut()?.get_mut(&method)?);

		let docstring = Self::parse_method_docstring(fn_scope, &contents)
			.map(|doc| ImStr::from(Method::postprocess_docstring(doc)));
		method.docstring = docstring;

		if let Some(params) = fn_scope.child_by_field_name("parameters") {
			// python parameters can be delineated by three separators:
			// - parameters before `/` are positional only (`positional_separator`)
			// - parameters between `/` and `*` can be either positional or named
			// - a catch-all positional `*args` (`keyword_separator` or `list_splat_pattern`)
			// - parameters after `*` and before `**` are named (`default_parameter`)
			// - a catch-all named `**kwargs` (`dictionary_splat_pattern` only)
			let mut cursor = params.walk();
			let args = params.named_children(&mut cursor).skip(1).filter_map(|param| {
				Some(match param.kind() {
					"identifier" => FunctionParam::Param(ImStr::from(&contents[param.byte_range()])),
					"positional_separator" => FunctionParam::PosEnd,
					"keyword_separator" => FunctionParam::EitherEnd(None),
					"list_splat_pattern" => {
						FunctionParam::EitherEnd(Some(ImStr::from(&contents[param.named_child(0)?.byte_range()])))
					}
					"dictionary_splat_pattern" => FunctionParam::Kwargs("kwargs".into()),
					"default_parameter" => {
						let name = param.named_child(0)?;
						let name = &contents[name.byte_range()];
						FunctionParam::Named(ImStr::from(name))
					}
					_ => return None,
				})
			});
			method.arguments = Some(args.collect());
		}

		method.pending_eval.store(false, Ordering::Release);

		// Unify all return types into a single type (potentially a Union)
		if let Some(tid) = type_cache().union(return_types) {
			method.eval_cache.insert(cache_key, tid);
			Some(tid)
		} else {
			None
		}
	}
	/// Evaluates the return type of a module-level function.
	#[instrument(level = "trace", ret, skip(self), fields(func = _R(func), path = %path))]
	pub fn eval_function_rtype(
		&self,
		func: Symbol<crate::model::Function>,
		path: PathSymbol,
	) -> Option<TypeId> {
		

		let funcs = self.functions.get(&path)?;
		let func_obj = funcs.get(&func)?;

		if func_obj
			.pending_eval
			.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
			.is_err()
		{
			return None;
		}

		let location = func_obj.location.clone();
		let func_path = path;
		drop(funcs);

		let _guard = Defer(Some(|| {
			if let Some(funcs) = self.functions.get(&func_path)
				&& let Some(func_obj) = funcs.get(&func)
			{
				func_obj.pending_eval.store(false, Ordering::Relaxed);
			}
		}));

		// Check cache first
		let empty_key: Vec<TypeId> = vec![];
		if let Some(funcs) = self.functions.get(&path)
			&& let Some(func_obj) = funcs.get(&func)
			&& let Some(tid) = func_obj.eval_cache.get(&empty_key)
		{
			return Some(tid);
		}

		let ast;
		let contents;
		let file_path = location.path.to_path();

		if let Some(cached) = self.ast_cache.get(&file_path) {
			ast = cached.tree.clone();
			contents = String::from(cached.rope.clone());
		} else {
			let Ok(file_contents) = test_utils::fs::read_to_string(file_path.clone()) else {
				return None;
			};
			contents = file_contents;
			let rope = Rope::from_str(&contents);
			let mut parser = python_parser();
			let Some(parsed_ast) = parser.parse(contents.as_bytes(), None) else {
				return None;
			};
			ast = parsed_ast;
			self.ast_cache.insert(
				file_path,
				Arc::new(crate::index::AstCacheItem {
					tree: ast.clone(),
					rope,
				}),
			);
		}

		// Find the function definition node
		// Use end_offset - 1 to ensure we're inside the function body, not at the very end
		let end_offset: ByteOffset = rope_conv(location.range.end, Rope::from_str(&contents).slice(..));
		let search_offset = end_offset.0.saturating_sub(1);
		let Some(fn_scope_start) = ast.root_node().descendant_for_byte_range(search_offset, search_offset) else {
			return None;
		};

		// Walk up to find the function_definition
		let fn_scope = {
			let mut node = fn_scope_start;
			loop {
				if node.kind() == "function_definition" {
					break node;
				}
				let Some(parent) = node.parent() else {
					trace!("eval_function_rtype: no parent, couldn't find function_definition");
					return None;
				};
				node = parent;
			}
		};

		/// Checks if a return statement belongs to the given function scope,
		/// not to a nested function definition.
		fn is_return_in_function(return_node: Node, fn_scope: Node) -> bool {
			let mut current = return_node;
			while let Some(parent) = current.parent() {
				if parent.id() == fn_scope.id() {
					return true;
				}
				if parent.kind() == "function_definition" && parent.id() != fn_scope.id() {
					return false;
				}
				current = parent;
			}
			false
		}

		let mut scope = Scope::default();
		scope.current_path = Some(path);

		// Parse imports for resolving function calls
		let imports = std::sync::Arc::new(parse_imports(ast.root_node(), &contents));
		scope.imports = Some(imports);

		// Parse function parameters into scope
		if let Some(params) = fn_scope.child_by_field_name("parameters") {
			let mut cursor = params.walk();
			for param in params.named_children(&mut cursor) {
				if param.kind() == "identifier" {
					let name = &contents[param.byte_range()];
					scope.insert(name.to_string(), Type::Value);
				} else if param.kind() == "default_parameter" {
					if let Some(name_node) = param.named_child(0) {
						let name = &contents[name_node.byte_range()];
						// Try to infer type from default value
						if let Some(value_node) = param.named_child(1)
							&& let Some(type_) = self.type_of(value_node, &scope, &contents)
						{
							scope.insert(name.to_string(), type_cache().resolve(type_).clone());
						} else {
							scope.insert(name.to_string(), Type::Value);
						}
					}
				}
			}
		}

		let offset = fn_scope.end_byte();

		// Create caller ID for call graph
		let caller = crate::call_graph::CallableId::function(path.as_string(), _R(func));
		let call_path = path;

		// Collect ALL return types
		let mut return_types: Vec<TypeId> = Vec::new();

		let _ = Self::walk_scope(fn_scope, Some(scope), |scope, node| {
			let entered = self.build_scope(scope, node, offset, &contents).map_break(|_| None::<()>)?;

			if node.kind() == "return_statement" && is_return_in_function(node, fn_scope) {
				if let Some(child) = node.named_child(0)
					&& let Some(type_) = self.type_of(child, scope, &contents)
				{
					let type_ = type_cache().resolve(type_);
					let resolved = match self.try_resolve_model(type_, scope) {
						Some(model) => _T!(Type::Model(ImStr::from(_R(model)))),
						None => _T!(type_.clone()),
					};
					return_types.push(resolved);
				}
			}

			// Collect calls for call hierarchy
			if node.kind() == "call" && is_return_in_function(node, fn_scope) {
				self.collect_call(node, scope, &caller, &contents, call_path);
			}

			ControlFlow::Continue(entered)
		});

		// Cache and return the result
		let result = type_cache().union(return_types);
		if let Some(tid) = result {
			if let dashmap::try_result::TryResult::Present(mut funcs) = self.functions.try_get_mut(&path) {
				if let Some(func_arc) = funcs.get_mut(&func) {
					let func_obj = Arc::make_mut(func_arc);
					func_obj.eval_cache.insert(empty_key, tid);
					func_obj.pending_eval.store(false, Ordering::Release);
				}
			}
		}

		result
	}

	/// `pattern` is `(identifier | pattern_list | tuple_pattern)`, the `a, b` in `for a, b in ...`.
	fn destructure_into_patternlist_like(&self, pattern: Node, tid: TypeId, scope: &mut Scope, contents: &str) {
		if pattern.kind() == "identifier" {
			let name = &contents[pattern.byte_range()];
			scope.insert(name.to_string(), _TR!(tid).clone());
		} else if matches!(pattern.kind(), "pattern_list" | "tuple_pattern") {
			if let Type::Tuple(inner) = _TR!(tid) {
				let mut inner = inner.iter();
				for child in pattern.named_children(&mut pattern.walk()) {
					if matches!(child.kind(), "identifier" | "tuple_pattern")
						&& let Some(type_) = inner.next()
					{
						self.destructure_into_patternlist_like(child, *type_, scope, contents);
					}
				}
			} else if let Some(inner) = self.type_of_iterable(tid) {
				// spread this type to all params
				for child in pattern.named_children(&mut pattern.walk()) {
					if matches!(child.kind(), "identifier" | "tuple_pattern") {
						self.destructure_into_patternlist_like(child, inner, scope, contents);
					}
				}
			}
		}
	}
	fn parse_method_docstring<'out>(fn_scope: Node, contents: &'out str) -> Option<&'out str> {
		let block = fn_scope.child_by_field_name("body")?;
		dig!(block, expression_statement.string.string_content(1)).map(|node| &contents[node.byte_range()])
	}
}

/// Returns `(self_type, fn_scope, self_param)`.
///
/// `fn_scope` is customarily a `function_definition` node.
#[instrument(level = "trace", skip_all, ret)]
pub fn determine_scope<'out, 'node>(
	node: Node<'node>,
	contents: &'out str,
	offset: usize,
) -> Option<(Option<Node<'node>>, Node<'node>, &'out str)> {
	let query = FieldCompletion::query();
	let mut self_type = None;
	let mut self_param = None;
	let mut fn_scope = None;
	let mut cursor = QueryCursor::new();
	let mut matches = cursor.matches(query, node, contents.as_bytes());
	'scoping: while let Some(match_) = matches.next() {
		// @class
		let class = match_.captures.first()?;
		if !class.node.byte_range().contains_end(offset) {
			continue;
		}
		for capture in match_.captures {
			match FieldCompletion::from(capture.index) {
				Some(FieldCompletion::Name) => {
					if self_type.is_none() {
						self_type = Some(capture.node);
					}
				}
				Some(FieldCompletion::SelfParam) => {
					self_param = Some(capture.node);
				}
				Some(FieldCompletion::Scope) => {
					if !capture.node.byte_range().contains_end(offset) {
						continue 'scoping;
					}
					fn_scope = Some(capture.node);
				}
				None => {}
			}
		}
		if fn_scope.is_some() {
			break;
		}
	}
	let fn_scope = fn_scope?;
	let self_param = &contents[self_param?.byte_range()];
	Some((self_type, fn_scope, self_param))
}

#[cfg(test)]
mod tests {
	use pretty_assertions::assert_eq;
	use ropey::Rope;
	use tower_lsp_server::ls_types::Position;
	use tree_sitter::{QueryCursor, StreamingIterator, StreamingIteratorMut};

	use crate::analyze::{FieldCompletion, Type, type_cache};
	use crate::index::_I;
	use crate::utils::{ByteOffset, acc_vec, python_parser, rope_conv};
	use crate::{index::Index, test_utils::cases::foo::prepare_foo_index};

	#[test]
	fn test_field_completion() {
		let mut parser = python_parser();
		let contents = br#"
class Foo(models.AbstractModel):
	_name = 'foo'
	_description = 'What?'
	_inherit = 'inherit_foo'
	foo = fields.Char(related='related')
	@api.depends('mapped')
	def foo(self):
		pass
"#;
		let ast = parser.parse(&contents[..], None).unwrap();
		let query = FieldCompletion::query();
		let mut cursor = QueryCursor::new();
		let actual = cursor
			.matches(query, ast.root_node(), &contents[..])
			.map(|match_| {
				match_
					.captures
					.iter()
					.map(|capture| FieldCompletion::from(capture.index))
					.collect::<Vec<_>>()
			})
			.fold_mut(vec![], acc_vec);
		// Allow nested patterns
		let actual = actual.iter().map(Vec::as_slice).collect::<Vec<_>>();
		use FieldCompletion as T;
		assert!(
			matches!(
				&actual[..],
				[
					[None, None, Some(T::Name), Some(T::Scope), Some(T::SelfParam)],
					[None, None, Some(T::Name), Some(T::Scope), Some(T::SelfParam)]
				]
			),
			"{actual:?}"
		)
	}

	#[test]
	fn test_determine_scope() {
		let mut parser = python_parser();
		let contents = r#"
class Foo(models.Model):
	_name = 'foo'
	def scope(self):
		pass
"#;
		let ast = parser.parse(contents, None).unwrap();
		let rope = Rope::from(contents);
		let fn_start: ByteOffset = rope_conv(Position { line: 3, character: 1 }, rope.slice(..));
		let fn_scope = ast
			.root_node()
			.named_descendant_for_byte_range(fn_start.0, fn_start.0)
			.unwrap();
		super::determine_scope(ast.root_node(), contents, fn_start.0)
			.unwrap_or_else(|| panic!("{}", fn_scope.to_sexp()));
	}

	#[test]
	fn test_resolve_method_returntype() {
		let index = Index {
			models: prepare_foo_index(),
			..Default::default()
		};

		assert_eq!(
			index.eval_method_rtype(_I("test").into(), _I("bar"), None),
			Some(type_cache().get_or_intern(Type::Model("foo".into())))
		)
	}

	#[test]
	fn test_super_analysis() {
		let index = Index {
			models: prepare_foo_index(),
			..Default::default()
		};

		assert_eq!(
			index.eval_method_rtype(_I("test").into(), _I("quux"), None),
			Some(type_cache().get_or_intern(Type::Model("foo".into())))
		)
	}
}
