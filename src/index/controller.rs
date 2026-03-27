//! Controller route indexing and data structures.
//!
//! This module provides support for indexing Odoo controller routes defined with
//! `@http.route()` decorators. It tracks route paths, types, authentication methods,
//! URL parameters, and supports controller inheritance.

use std::collections::HashSet;
use std::sync::RwLock;

use dashmap::DashMap;
use qp_trie::Trie;
use smart_default::SmartDefault;
use strum::{AsRefStr, EnumIter, EnumString, IntoStaticStr, VariantNames};

use crate::index::ModuleName;
use crate::model::TrackedMinLoc;
use crate::prelude::*;
use crate::ImStr;

/// HTTP route type as specified in `type=` parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, EnumString, AsRefStr, IntoStaticStr, EnumIter, VariantNames)]
#[strum(serialize_all = "lowercase")]
pub enum RouteType {
	/// Standard HTTP route returning HTML/text/binary responses.
	#[default]
	Http,
	/// JSON-RPC route returning JSON responses.
	Json,
}

impl RouteType {
	/// All valid route type values for completion.
	pub fn all_values() -> &'static [&'static str] {
		Self::VARIANTS
	}
}

/// Authentication type for routes as specified in `auth=` parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, EnumString, AsRefStr, IntoStaticStr, EnumIter, VariantNames)]
#[strum(serialize_all = "lowercase")]
pub enum AuthType {
	/// User must be authenticated. Request runs with user's rights.
	#[default]
	User,
	/// User may or may not be authenticated. Uses Public user if not logged in.
	Public,
	/// No database access. Used for system routes and authentication modules.
	None,
	/// API token authentication via `Authorization: Bearer <token>` header.
	Bearer,
}

impl AuthType {
	/// All valid auth type values for completion.
	pub fn all_values() -> &'static [&'static str] {
		Self::VARIANTS
	}
}

/// HTTP methods that can be specified in `methods=` parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EnumString, AsRefStr, IntoStaticStr, EnumIter, VariantNames)]
#[strum(serialize_all = "UPPERCASE", ascii_case_insensitive)]
pub enum HttpMethod {
	Get,
	Post,
	Put,
	Delete,
	Patch,
	Head,
	Options,
}

impl HttpMethod {
	/// All valid HTTP methods for completion.
	pub fn all_values() -> &'static [&'static str] {
		Self::VARIANTS
	}
}

/// URL parameter converter type extracted from route path.
///
/// Odoo uses Werkzeug routing which supports various converters:
/// - `<name>` - default string converter
/// - `<int:id>` - integer converter
/// - `<path:filepath>` - path converter (captures slashes)
/// - `<model("res.partner"):partner>` - Odoo model converter
/// - `<models("product.template"):products>` - multiple models converter
/// - `<any(css,js):ext>` - any of specific values
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteConverter {
	/// Default string converter.
	String,
	/// Integer converter: `<int:id>`.
	Int,
	/// Path converter (captures slashes): `<path:filepath>`.
	Path,
	/// Model converter: `<model("res.partner"):partner>`.
	/// The parameter contains the model name.
	Model(ImStr),
	/// Multiple models converter: `<models("product.template"):products>`.
	/// The parameter contains the model name.
	Models(ImStr),
	/// Any of specific values: `<any(css,js):ext>`.
	/// The parameter contains the allowed values.
	Any(Vec<ImStr>),
}

impl RouteConverter {
	/// Get a human-readable description of this converter.
	pub fn description(&self) -> String {
		match self {
			Self::String => "string".to_string(),
			Self::Int => "int".to_string(),
			Self::Path => "path".to_string(),
			Self::Model(model) => format!("model(\"{}\")", model),
			Self::Models(model) => format!("models(\"{}\")", model),
			Self::Any(values) => {
				let vals: Vec<&str> = values.iter().map(|s| s.as_ref()).collect();
				format!("any({})", vals.join(","))
			}
		}
	}
}

/// A URL parameter extracted from route path.
#[derive(Debug, Clone)]
pub struct RouteParam {
	/// Parameter name as it appears in the URL and method signature.
	pub name: ImStr,
	/// Converter type (determines how the URL segment is parsed).
	pub converter: RouteConverter,
	/// Position in the URL path (0-indexed).
	pub position: usize,
}

/// Whether readonly is static or dynamic (callback function).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadonlyValue {
	/// Static boolean value.
	Static(bool),
	/// Dynamic callback function - marked as dynamic.
	Dynamic,
}

/// Represents a controller class definition.
#[derive(Debug, Clone)]
pub struct ControllerClass {
	/// Class name.
	pub name: ImStr,
	/// Parent controller class name (if inheriting from another controller).
	pub parent: Option<ImStr>,
	/// Module this controller belongs to.
	pub module: ModuleName,
	/// Location of the class definition.
	pub location: MinLoc,
}

/// A controller route definition extracted from `@http.route()` decorator.
#[derive(Debug, Clone)]
pub struct ControllerRoute {
	/// Primary route path (first path if multiple defined).
	pub path: ImStr,
	/// All route paths (if multiple paths defined via list).
	pub paths: Vec<ImStr>,
	/// Route type (json or http).
	pub route_type: RouteType,
	/// Authentication method.
	pub auth: AuthType,
	/// Allowed HTTP methods (None = all methods allowed).
	pub methods: Option<Vec<HttpMethod>>,
	/// CSRF protection enabled (default: true for http, false for json).
	pub csrf: Option<bool>,
	/// CORS Access-Control-Allow-Origin value.
	pub cors: Option<ImStr>,
	/// Website route flag (enables website features).
	pub website: bool,
	/// Readonly mode (use read-only database replica).
	pub readonly: Option<ReadonlyValue>,
	/// Python method name.
	pub method_name: ImStr,
	/// Controller class name.
	pub controller_class: ImStr,
	/// URL parameters parsed from path.
	pub params: Vec<RouteParam>,
	/// Method parameters from function signature (excluding self).
	pub method_params: Vec<ImStr>,
	/// Location of the route decorator.
	pub location: TrackedMinLoc,
	/// Module this route belongs to.
	pub module: ModuleName,
	/// Parent route path being overridden (for inheritance tracking).
	pub overrides: Option<ImStr>,
	/// Whether this route is marked as deleted (for incremental updates).
	pub deleted: bool,
}

/// Unique identifier for a route (interned path string).
pub type RoutePath = Symbol<ControllerRoute>;

/// Index of controller routes.
#[derive(SmartDefault)]
pub struct RouteIndex {
	/// Primary index: route path -> route definition.
	#[default(_code = "DashMap::with_shard_amount(4)")]
	inner: DashMap<RoutePath, ControllerRoute>,

	/// Routes by module (for per-module duplicate detection).
	#[default(_code = "DashMap::with_shard_amount(4)")]
	by_module: DashMap<ModuleName, HashSet<RoutePath>>,

	/// Routes by controller class.
	#[default(_code = "DashMap::with_shard_amount(4)")]
	by_controller: DashMap<ImStr, HashSet<RoutePath>>,

	/// Prefix trie for route path completions.
	pub by_prefix: RwLock<Trie<ImStr, HashSet<RoutePath>>>,

	/// Controller classes indexed by name.
	#[default(_code = "DashMap::with_shard_amount(4)")]
	pub controllers: DashMap<ImStr, ControllerClass>,
}

impl RouteIndex {
	/// Insert a new route into the index.
	pub fn insert(&self, route: ControllerRoute) {
		let path_key: RoutePath = _I(&route.path).into();

		// Add to module index
		self.by_module.entry(route.module).or_default().insert(path_key);

		// Add to controller index
		self.by_controller
			.entry(route.controller_class.clone())
			.or_default()
			.insert(path_key);

		// Add to prefix trie for completion
		if let Ok(mut by_prefix) = self.by_prefix.write() {
			by_prefix
				.entry(route.path.clone())
				.or_insert_with(Default::default)
				.insert(path_key);
		}

		self.inner.insert(path_key, route);
	}

	/// Get a route by its path.
	pub fn get(&self, path: &str) -> Option<dashmap::mapref::one::Ref<'_, RoutePath, ControllerRoute>> {
		let key: RoutePath = _G(path)?.into();
		self.inner.get(&key)
	}

	/// Get all routes for a module.
	pub fn by_module(&self, module: &ModuleName) -> Vec<dashmap::mapref::one::Ref<'_, RoutePath, ControllerRoute>> {
		self.by_module
			.get(module)
			.map(|paths| paths.iter().filter_map(|p| self.inner.get(p)).collect())
			.unwrap_or_default()
	}

	/// Get all routes for a controller class.
	pub fn by_controller(&self, controller: &str) -> Vec<dashmap::mapref::one::Ref<'_, RoutePath, ControllerRoute>> {
		let controller_key: ImStr = controller.into();
		self.by_controller
			.get(&controller_key)
			.map(|paths| paths.iter().filter_map(|p| self.inner.get(p)).collect())
			.unwrap_or_default()
	}

	/// Find routes by prefix for completion.
	pub fn complete_by_prefix(&self, prefix: &str) -> Vec<ImStr> {
		if let Ok(by_prefix) = self.by_prefix.read() {
			by_prefix
				.iter_prefix(prefix.as_bytes())
				.map(|(k, _)| k.clone())
				.collect()
		} else {
			vec![]
		}
	}

	/// Check for duplicate route in same module.
	pub fn has_duplicate_in_module(&self, path: &str, module: &ModuleName) -> bool {
		if let Some(paths) = self.by_module.get(module) {
			let key: Option<RoutePath> = _G(path).map(|s| s.into());
			if let Some(key) = key {
				paths.contains(&key)
			} else {
				false
			}
		} else {
			false
		}
	}

	/// Clear routes for a file (used when re-indexing).
	pub fn clear_file(&self, path: &PathSymbol) {
		// Find routes whose location matches this file
		let to_remove: Vec<RoutePath> = self
			.inner
			.iter()
			.filter(|r| r.location.path == *path)
			.map(|r| *r.key())
			.collect();

		for key in to_remove {
			if let Some((_, route)) = self.inner.remove(&key) {
				// Clean up secondary indexes
				if let Some(mut paths) = self.by_module.get_mut(&route.module) {
					paths.remove(&key);
				}
				if let Some(mut paths) = self.by_controller.get_mut(&route.controller_class) {
					paths.remove(&key);
				}
				// Note: We don't clean up by_prefix here for efficiency
				// The stale entries will be overwritten on re-index
			}
		}
	}

	/// Iterate over all routes.
	pub fn iter(&self) -> impl Iterator<Item = dashmap::mapref::multiple::RefMulti<'_, RoutePath, ControllerRoute>> {
		self.inner.iter()
	}

	/// Retain only routes matching the predicate.
	pub fn retain<F>(&self, mut f: F)
	where
		F: FnMut(&RoutePath, &mut ControllerRoute) -> bool,
	{
		self.inner.retain(|k, v| f(k, v));
	}

	/// Get total number of indexed routes.
	pub fn len(&self) -> usize {
		self.inner.len()
	}

	/// Check if the index is empty.
	pub fn is_empty(&self) -> bool {
		self.inner.is_empty()
	}
}

/// Parse URL parameters from a route path.
///
/// # Examples
///
/// ```ignore
/// let params = parse_url_params("/order/<int:order_id>");
/// assert_eq!(params[0].name, "order_id");
/// assert_eq!(params[0].converter, RouteConverter::Int);
///
/// let params = parse_url_params("/product/<model(\"product.template\"):product>");
/// assert_eq!(params[0].name, "product");
/// assert!(matches!(params[0].converter, RouteConverter::Model(_)));
/// ```
pub fn parse_url_params(path: &str) -> Vec<RouteParam> {
	let mut params = Vec::new();
	let mut pos = 0;

	// Manual parsing for <type:name> or <name> patterns
	let mut chars = path.char_indices().peekable();
	while let Some((i, c)) = chars.next() {
		if c == '<' {
			let start = i + 1;
			let mut end = start;
			let mut depth = 1;

			// Find matching '>' handling nested parentheses
			while let Some((j, ch)) = chars.next() {
				match ch {
					'<' => depth += 1,
					'>' => {
						depth -= 1;
						if depth == 0 {
							end = j;
							break;
						}
					}
					_ => {}
				}
			}

			if end > start {
				let param_str = &path[start..end];
				if let Some(param) = parse_single_param(param_str, pos) {
					params.push(param);
					pos += 1;
				}
			}
		}
	}

	params
}

/// Parse a single URL parameter definition.
fn parse_single_param(s: &str, position: usize) -> Option<RouteParam> {
	// Handle colon-separated converter:name pattern
	// But be careful with model("..."):name where the colon is inside quotes

	// Find the last colon that's not inside parentheses or quotes
	let mut paren_depth: i32 = 0;
	let mut in_quotes = false;
	let mut quote_char = '"';
	let mut last_colon = None;

	for (i, c) in s.char_indices() {
		match c {
			'"' | '\'' if !in_quotes => {
				in_quotes = true;
				quote_char = c;
			}
			c if in_quotes && c == quote_char => {
				in_quotes = false;
			}
			'(' if !in_quotes => paren_depth += 1,
			')' if !in_quotes => paren_depth = paren_depth.saturating_sub(1),
			':' if !in_quotes && paren_depth == 0 => {
				last_colon = Some(i);
			}
			_ => {}
		}
	}

	if let Some(colon_pos) = last_colon {
		let converter_part = &s[..colon_pos];
		let name = &s[colon_pos + 1..];

		let converter = parse_converter(converter_part);

		Some(RouteParam {
			name: name.into(),
			converter,
			position,
		})
	} else {
		// No converter specified, just a name - default to string
		Some(RouteParam {
			name: s.into(),
			converter: RouteConverter::String,
			position,
		})
	}
}

/// Parse a converter specification.
fn parse_converter(s: &str) -> RouteConverter {
	let s = s.trim();

	if s == "int" {
		RouteConverter::Int
	} else if s == "string" || s.is_empty() {
		RouteConverter::String
	} else if s == "path" {
		RouteConverter::Path
	} else if let Some(rest) = s.strip_prefix("model(") {
		// Extract model name: model("res.partner")
		let model_name = rest
			.strip_suffix(')')
			.unwrap_or(rest)
			.trim_matches('"')
			.trim_matches('\'');
		RouteConverter::Model(model_name.into())
	} else if let Some(rest) = s.strip_prefix("models(") {
		let model_name = rest
			.strip_suffix(')')
			.unwrap_or(rest)
			.trim_matches('"')
			.trim_matches('\'');
		RouteConverter::Models(model_name.into())
	} else if let Some(rest) = s.strip_prefix("any(") {
		let values_str = rest.strip_suffix(')').unwrap_or(rest);
		let values: Vec<ImStr> = values_str.split(',').map(|s| s.trim().into()).collect();
		RouteConverter::Any(values)
	} else {
		// Unknown converter, treat as string
		RouteConverter::String
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_parse_url_params_simple() {
		let params = parse_url_params("/user/<username>");
		assert_eq!(params.len(), 1);
		assert_eq!(<ImStr as AsRef<str>>::as_ref(&params[0].name), "username");
		assert_eq!(params[0].converter, RouteConverter::String);
	}

	#[test]
	fn test_parse_url_params_int() {
		let params = parse_url_params("/order/<int:order_id>");
		assert_eq!(params.len(), 1);
		assert_eq!(<ImStr as AsRef<str>>::as_ref(&params[0].name), "order_id");
		assert_eq!(params[0].converter, RouteConverter::Int);
	}

	#[test]
	fn test_parse_url_params_path() {
		let params = parse_url_params("/files/<path:filepath>");
		assert_eq!(params.len(), 1);
		assert_eq!(<ImStr as AsRef<str>>::as_ref(&params[0].name), "filepath");
		assert_eq!(params[0].converter, RouteConverter::Path);
	}

	#[test]
	fn test_parse_url_params_model() {
		let params = parse_url_params("/product/<model(\"product.template\"):product>");
		assert_eq!(params.len(), 1);
		assert_eq!(<ImStr as AsRef<str>>::as_ref(&params[0].name), "product");
		assert!(matches!(
			&params[0].converter,
			RouteConverter::Model(m) if <ImStr as AsRef<str>>::as_ref(m) == "product.template"
		));
	}

	#[test]
	fn test_parse_url_params_models() {
		let params = parse_url_params("/compare/<models(\"product.template\"):products>");
		assert_eq!(params.len(), 1);
		assert_eq!(<ImStr as AsRef<str>>::as_ref(&params[0].name), "products");
		assert!(matches!(
			&params[0].converter,
			RouteConverter::Models(m) if <ImStr as AsRef<str>>::as_ref(m) == "product.template"
		));
	}

	#[test]
	fn test_parse_url_params_any() {
		let params = parse_url_params("/static.<any(css,js,json):ext>");
		assert_eq!(params.len(), 1);
		assert_eq!(<ImStr as AsRef<str>>::as_ref(&params[0].name), "ext");
		assert!(matches!(
			&params[0].converter,
			RouteConverter::Any(values) if values.len() == 3
		));
	}

	#[test]
	fn test_parse_url_params_multiple() {
		let params = parse_url_params("/shop/<int:category_id>/product/<int:product_id>");
		assert_eq!(params.len(), 2);
		assert_eq!(<ImStr as AsRef<str>>::as_ref(&params[0].name), "category_id");
		assert_eq!(params[0].position, 0);
		assert_eq!(<ImStr as AsRef<str>>::as_ref(&params[1].name), "product_id");
		assert_eq!(params[1].position, 1);
	}

	#[test]
	fn test_route_type_from_str() {
		assert_eq!("http".parse::<RouteType>(), Ok(RouteType::Http));
		assert_eq!("json".parse::<RouteType>(), Ok(RouteType::Json));
		assert!("invalid".parse::<RouteType>().is_err());
	}

	#[test]
	fn test_auth_type_from_str() {
		assert_eq!("user".parse::<AuthType>(), Ok(AuthType::User));
		assert_eq!("public".parse::<AuthType>(), Ok(AuthType::Public));
		assert_eq!("none".parse::<AuthType>(), Ok(AuthType::None));
		assert_eq!("bearer".parse::<AuthType>(), Ok(AuthType::Bearer));
		assert!("invalid".parse::<AuthType>().is_err());
	}

	#[test]
	fn test_http_method_from_str() {
		assert_eq!("GET".parse::<HttpMethod>(), Ok(HttpMethod::Get));
		assert_eq!("get".parse::<HttpMethod>(), Ok(HttpMethod::Get));
		assert_eq!("POST".parse::<HttpMethod>(), Ok(HttpMethod::Post));
		assert!("invalid".parse::<HttpMethod>().is_err());
	}
}
