#![allow(clippy::disallowed_methods)]

use std::ops::DerefMut;
use std::{collections::HashMap, path::PathBuf};

use dashmap::DashMap;
use lasso::{Key, Spur};
use smart_default::SmartDefault;
use tokio::sync::RwLock;
use tree_sitter::{Node, QueryCursor, StreamingIterator};
use ts_macros::query;

use crate::component::{ComponentTemplate, PropDescriptor, PropType};
use crate::hook::{HookArgs, HookUsage, ServiceDefinition};
use crate::index::{_I, PathSymbol};
use crate::utils::{ByteOffset, MinLoc, RangeExt, js_parser, rope_conv, span_conv};
use crate::{ImStr, dig, errloc, format_loc, ok};

use super::{_R, Component, ComponentName, Output, TemplateName};

// Three cases:
// static props OR Foo.props: Component props
// static template OR Foo.template: Component template (XML name or inline decl)
// static components OR Foo.components: Component subcomponents
#[rustfmt::skip]
query! {
	#[lang = "tree_sitter_javascript"]
	JsQuery(Name, Prop, Parent, TemplateName, TemplateInline, Subcomponent, Registry, RegistryItem);
((class_declaration
  (identifier) @NAME
  (class_body [
    (field_definition . "static"
      property: (property_identifier) @_props 
      value: [
        (array ((string) @PROP "," ?)*)
        (object [
          (spread_element
            (member_expression
              (identifier) @PARENT (property_identifier) @_props))
          (pair key: [
            (property_identifier) @PROP 
            (string) @PROP]) ])
        (member_expression
          (identifier) @PARENT (property_identifier) @_props)])
    (field_definition . "static"
      property: (property_identifier) @_template 
      value: [ 
        (string) @TEMPLATE_NAME
        (template_string) @TEMPLATE_NAME
        (call_expression
          (identifier) @_xml (template_string) @TEMPLATE_INLINE)])
    (field_definition . "static"
      property: (property_identifier) @_components 
      value: (object [
        (pair (property_identifier) @SUBCOMPONENT)
        (shorthand_property_identifier) @SUBCOMPONENT])) ]?))
  (#eq? @_props "props")
  (#eq? @_template "template")
  (#eq? @_xml "xml")
  (#eq? @_components "components")
  (#match? @NAME "^[A-Z]")
  (#match? @PARENT "^[A-Z]")
  (#match? @SUBCOMPONENT "^[A-Z]"))


((assignment_expression
  left: (member_expression
    (identifier) @NAME (property_identifier) @_props)
  right: [ 
    (array ((string) @PROP "," ?)*)
    (object [ 
      (spread_element
        (member_expression
          (identifier) @PARENT (property_identifier) @_props))
      (pair key: [
        (property_identifier) @PROP 
        (string) @PROP]) ])
    (member_expression
      (identifier) @PARENT (property_identifier) @_props)
    (call_expression
      (member_expression
        (identifier) @_Object (property_identifier) @_assign)
      (arguments [ 
        (member_expression
          (identifier) @PARENT (property_identifier) @_props)
        (object [ 
          (spread_element
            (member_expression
              (identifier) @PARENT (property_identifier) @_props))
          (pair key: [
            (property_identifier) @PROP 
            (string) @PROP]) ]) ]))])
  (#eq? @_props "props")
  (#eq? @_Object "Object")
  (#eq? @_assign "assign")
  (#match? @NAME "^[A-Z]")
  (#match? @PARENT "^[A-Z]"))

((assignment_expression
  left: (member_expression
    (identifier) @NAME (property_identifier) @_template)
  right: [
    (string) @TEMPLATE_NAME
    (template_string) @TEMPLATE_NAME
    (call_expression (identifier) @_xml (template_string) @TEMPLATE_INLINE) ])
  (#eq? @_template "template")
  (#match? @NAME "^[A-Z]"))

((assignment_expression
  left: (member_expression
    (identifier) @NAME (property_identifier) @_components)
  right: (object [
    (pair (property_identifier) @SUBCOMPONENT)
    (shorthand_property_identifier) @SUBCOMPONENT ]))
  (#eq? @_components "components")
  (#match? @SUBCOMPONENT "^[A-Z]"))

// registry.category(CATEGORY).add(FIELD, ..)
(call_expression
  (member_expression (_) @REGISTRY (property_identifier) @_add (#eq? @_add "add"))
  (arguments . (string) @REGISTRY_ITEM))
}

// Query for service registry: registry.category("services").add("name", definition)
// We use registry_category_of_callee() to verify it's a "services" category
#[rustfmt::skip]
query! {
	#[lang = "tree_sitter_javascript"]
	ServiceRegistryQuery(RegistryCall, ServiceName, ServiceObject);
// registry.category("services").add("serviceName", serviceDefinition)
(call_expression
  function: (member_expression
    object: (_) @REGISTRY_CALL
    property: (property_identifier) @_add (#eq? @_add "add"))
  arguments: (arguments
    . (string) @SERVICE_NAME
    . ","
    . (_) @SERVICE_OBJECT))
}

// Query for hook usage in setup() methods and component bodies
#[rustfmt::skip]
query! {
	#[lang = "tree_sitter_javascript"]
	HookUsageQuery(HookCall, HookName, FirstArg, AssignTarget);
// Hook calls: useXxx(...) or onXxx(...)  
// Direct call: useState({})
(call_expression
  function: (identifier) @HOOK_NAME
    (#match? @HOOK_NAME "^(use[A-Z]|on(Mounted|WillStart|WillUpdateProps|WillRender|Rendered|Patched|WillUnmount|WillDestroy|Error))") 
  arguments: (arguments
    . (_)? @FIRST_ARG)) @HOOK_CALL

// Assignment: this.xxx = useXxx(...)
(assignment_expression
  left: (member_expression
    object: (this)
    property: (property_identifier) @ASSIGN_TARGET)
  right: (call_expression
    function: (identifier) @HOOK_NAME
      (#match? @HOOK_NAME "^use[A-Z]")
    arguments: (arguments
      . (_)? @FIRST_ARG)) @HOOK_CALL)
}

pub(super) async fn add_root_js(root: Spur, pathbuf: PathBuf) -> anyhow::Result<Output> {
	let path = PathSymbol::strip_root(root, &pathbuf);
	let contents = ok!(tokio::fs::read(&pathbuf).await, "Could not read {:?}", pathbuf);
	let rope = ropey::Rope::from(String::from_utf8_lossy(&contents));
	let rope = rope.slice(..);
	let mut parser = js_parser();
	let ast = parser.parse(&contents, None).ok_or_else(|| errloc!("AST not parsed"))?;

	let mut components = HashMap::<_, Component>::default();
	let mut widgets = Vec::new();
	let mut actions = Vec::new();
	let mut services = Vec::new();

	// Process component definitions using JsQuery
	{
		let query = JsQuery::query();
		let mut cursor = QueryCursor::new();
		let mut matches = cursor.matches(query, ast.root_node(), contents.as_slice());
		while let Some(match_) = matches.next() {
			let mut component = match match_.captures.first() {
				Some(first) if first.index == JsQuery::Name as u32 => {
					let name = String::from_utf8_lossy(&contents[first.node.byte_range()]);
					let name = _I(&name);
					let component = components.entry(name.into()).or_default();
					if component.location.is_none() {
						component.location = Some(MinLoc {
							path,
							range: span_conv(first.node.range()),
						});
					}
					Some(component)
				}
				_ => None,
			};

			for capture in match_.captures {
				use intmap::Entry;
				match JsQuery::from(capture.index) {
					Some(JsQuery::Prop) => {
						let Some(component) = &mut component else { continue };
						let mut range = capture.node.byte_range();
						if capture.node.kind() == "string" {
							range = range.shrink(1);
						}
						let prop = String::from_utf8_lossy(&contents[range.clone()]);
						let prop = _I(prop).into_usize();
						let entry = match component.props.entry(prop as _) {
							Entry::Occupied(entry) => entry.into_mut(),
							Entry::Vacant(entry) => entry.insert(PropDescriptor {
								type_: Default::default(),
								location: MinLoc {
									path,
									range: rope_conv(range.map_unit(ByteOffset), rope),
								},
							}),
						};
						if let Some(descriptor) = capture.node.next_named_sibling() {
							entry.type_ = parse_prop_type(descriptor, &contents, Some(entry.type_));
						}
					}
					Some(JsQuery::Parent) => {
						let Some(component) = &mut component else { continue };
						let parent = String::from_utf8_lossy(&contents[capture.node.byte_range()]);
						let parent = _I(parent);
						component.ancestors.push(parent.into());
					}
					Some(JsQuery::TemplateName) => {
						let Some(component) = &mut component else { continue };
						let name = String::from_utf8_lossy(&contents[capture.node.byte_range().shrink(1)]);
						let name = _I(&name);
						component.template = Some(ComponentTemplate::Name(name.into()));
					}
					Some(JsQuery::TemplateInline) => {
						let Some(component) = &mut component else { continue };
						let range = capture.node.byte_range().shrink(1).map_unit(ByteOffset);
						component.template = Some(ComponentTemplate::Inline(rope_conv(range, rope)));
					}
					Some(JsQuery::Subcomponent) => {
						let Some(component) = &mut component else { continue };
						let subcomponent = String::from_utf8_lossy(&contents[capture.node.byte_range()]);
						let subcomponent = _I(&subcomponent);
						component.subcomponents.push(subcomponent.into());
					}
					Some(JsQuery::RegistryItem) => {
						let Some(registry) = match_.nodes_for_capture_index(JsQuery::Registry as _).next() else {
							continue;
						};
						let range = capture.node.byte_range().shrink(1);
						let field = String::from_utf8_lossy(&contents[range]);
						let loc = MinLoc {
							path,
							range: span_conv(capture.node.range()),
						};
						match registry_category_of_callee(registry, &contents) {
							Some(b"fields") => widgets.push((ImStr::from(field.as_ref()), loc)),
							Some(b"actions") => actions.push((ImStr::from(field.as_ref()), loc)),
							Some(_) | None => {}
						}
					}
					Some(JsQuery::Registry) | Some(JsQuery::Name) | None => {}
				}
			}
		}
	}

	// Process service registrations using ServiceRegistryQuery
	{
		let query = ServiceRegistryQuery::query();
		let mut cursor = QueryCursor::new();
		let mut matches = cursor.matches(query, ast.root_node(), contents.as_slice());
		while let Some(match_) = matches.next() {
			let mut registry_call_node = None;
			let mut service_name_node = None;
			let mut service_object_node = None;

			for capture in match_.captures {
				match ServiceRegistryQuery::from(capture.index) {
					Some(ServiceRegistryQuery::RegistryCall) => {
						registry_call_node = Some(capture.node);
					}
					Some(ServiceRegistryQuery::ServiceName) => {
						service_name_node = Some(capture.node);
					}
					Some(ServiceRegistryQuery::ServiceObject) => {
						service_object_node = Some(capture.node);
					}
					None => {}
				}
			}

			// Verify this is a "services" category registration
			let Some(registry_call) = registry_call_node else { continue };
			let Some(service_name) = service_name_node else { continue };

			if registry_category_of_callee(registry_call, &contents) != Some(b"services") {
				continue;
			}

			let name_range = service_name.byte_range().shrink(1);
			let name = String::from_utf8_lossy(&contents[name_range]);
			let loc = MinLoc {
				path,
				range: span_conv(service_name.range()),
			};

			let mut service = ServiceDefinition {
				name: ImStr::from(name.as_ref()),
				location: Some(loc),
				dependencies: Vec::new(),
				async_methods: Vec::new(),
				module: None,
				builtin: false,
			};

			// Extract async methods and dependencies from service object
			if let Some(obj_node) = service_object_node {
				parse_service_definition(obj_node, &contents, &mut service);
			}

			services.push(service);
		}
	}

	// Process hook usages using HookUsageQuery
	{
		let query = HookUsageQuery::query();
		let mut cursor = QueryCursor::new();
		let mut matches = cursor.matches(query, ast.root_node(), contents.as_slice());
		while let Some(match_) = matches.next() {
			let mut hook_call_node = None;
			let mut hook_name_node = None;
			let mut first_arg_node = None;
			let mut assign_target_node = None;

			for capture in match_.captures {
				match HookUsageQuery::from(capture.index) {
					Some(HookUsageQuery::HookCall) => {
						hook_call_node = Some(capture.node);
					}
					Some(HookUsageQuery::HookName) => {
						hook_name_node = Some(capture.node);
					}
					Some(HookUsageQuery::FirstArg) => {
						first_arg_node = Some(capture.node);
					}
					Some(HookUsageQuery::AssignTarget) => {
						assign_target_node = Some(capture.node);
					}
					None => {}
				}
			}

			let Some(hook_call) = hook_call_node else { continue };
			let Some(hook_name) = hook_name_node else { continue };

			let hook_name_str = String::from_utf8_lossy(&contents[hook_name.byte_range()]);
			let loc = MinLoc {
				path,
				range: span_conv(hook_call.range()),
			};

			// Parse hook arguments based on hook type
			let args = parse_hook_args(&hook_name_str, first_arg_node, &contents);

			// Extract service and ref names for component tracking
			let (service_name, ref_name) = match &args {
				HookArgs::Service(name) => (Some(name.clone()), None),
				HookArgs::Ref(name) => (None, Some(name.clone())),
				_ => (None, None),
			};

			let variable = assign_target_node.map(|node| {
				let var_name = String::from_utf8_lossy(&contents[node.byte_range()]);
				ImStr::from(var_name.as_ref())
			});

			let hook_usage = HookUsage {
				hook_name: ImStr::from(hook_name_str.as_ref()),
				location: loc,
				variable,
				args,
			};

			// Try to find the containing component and add this hook to it
			if let Some(component_name) = find_containing_component(hook_call, &contents) {
				let component = components.entry(component_name.into()).or_default();
				component.hooks.push(hook_usage);
				if let Some(service) = service_name {
					if !component.services.contains(&service) {
						component.services.push(service);
					}
				}
				if let Some(ref_name) = ref_name {
					if !component.refs.contains(&ref_name) {
						component.refs.push(ref_name);
					}
				}
			}
		}
	}

	Ok(Output::JsItems {
		components,
		widgets,
		actions,
		services,
	})
}

/// Parse service definition object to extract async methods and dependencies
fn parse_service_definition(node: Node, contents: &[u8], service: &mut ServiceDefinition) {
	if node.kind() != "object" {
		// Could be an identifier referencing a service definition
		return;
	}

	for child in node.named_children(&mut node.walk()) {
		if child.kind() != "pair" {
			continue;
		}

		let Some(key) = child.named_child(0) else { continue };
		let Some(value) = child.named_child(1) else { continue };

		let mut key_range = key.byte_range();
		if key.kind() == "string" {
			key_range = key_range.shrink(1);
		}
		let key_name = &contents[key_range];

		match key_name {
			b"async" => {
				// async: ["method1", "method2"] or async: true
				if value.kind() == "array" {
					for item in value.named_children(&mut value.walk()) {
						if item.kind() == "string" {
							let method_range = item.byte_range().shrink(1);
							let method = String::from_utf8_lossy(&contents[method_range]);
							service.async_methods.push(ImStr::from(method.as_ref()));
						}
					}
				}
			}
			b"dependencies" => {
				// dependencies: ["service1", "service2"]
				if value.kind() == "array" {
					for item in value.named_children(&mut value.walk()) {
						if item.kind() == "string" {
							let dep_range = item.byte_range().shrink(1);
							let dep = String::from_utf8_lossy(&contents[dep_range]);
							service.dependencies.push(ImStr::from(dep.as_ref()));
						}
					}
				}
			}
			_ => {}
		}
	}
}

/// Parse hook arguments based on hook type
fn parse_hook_args(hook_name: &str, first_arg: Option<Node>, contents: &[u8]) -> HookArgs {
	let Some(arg) = first_arg else {
		return HookArgs::None;
	};

	match hook_name {
		"useService" => {
			if arg.kind() == "string" {
				let service_name = String::from_utf8_lossy(&contents[arg.byte_range().shrink(1)]);
				return HookArgs::Service(ImStr::from(service_name.as_ref()));
			}
		}
		"useRef" | "useForwardRefToParent" => {
			if arg.kind() == "string" {
				let ref_name = String::from_utf8_lossy(&contents[arg.byte_range().shrink(1)]);
				return HookArgs::Ref(ImStr::from(ref_name.as_ref()));
			}
		}
		"useHotkey" => {
			if arg.kind() == "string" {
				let hotkey = String::from_utf8_lossy(&contents[arg.byte_range().shrink(1)]);
				return HookArgs::Hotkey(ImStr::from(hotkey.as_ref()));
			}
		}
		"useBus" => {
			// useBus(bus, "eventName", callback) - second arg is the event name
			if let Some(parent) = arg.parent() {
				if parent.kind() == "arguments" {
					// Try to get second argument (event name)
					let mut idx = 0;
					for child in parent.named_children(&mut parent.walk()) {
						if idx == 1 && child.kind() == "string" {
							let event = String::from_utf8_lossy(&contents[child.byte_range().shrink(1)]);
							return HookArgs::Bus {
								event: Some(ImStr::from(event.as_ref())),
							};
						}
						idx += 1;
					}
				}
			}
			return HookArgs::Bus { event: None };
		}
		"useState" => {
			return HookArgs::State;
		}
		name if name.starts_with("on") => {
			return HookArgs::Lifecycle;
		}
		_ => {}
	}

	HookArgs::Other
}

/// Find the containing component class for a hook call
fn find_containing_component(node: Node, contents: &[u8]) -> Option<Spur> {
	let mut current = node.parent();
	while let Some(parent) = current {
		if parent.kind() == "class_body" {
			// Go up to class_declaration to get the name
			if let Some(class_decl) = parent.parent() {
				if class_decl.kind() == "class_declaration" || class_decl.kind() == "class" {
					// Find the identifier child
					for child in class_decl.named_children(&mut class_decl.walk()) {
						if child.kind() == "identifier" {
							let name = String::from_utf8_lossy(&contents[child.byte_range()]);
							// Only consider classes starting with uppercase (components)
							if name.chars().next().is_some_and(|c| c.is_uppercase()) {
								return Some(_I(&name));
							}
							break;
						}
					}
				}
			}
		}
		current = parent.parent();
	}
	None
}

fn parse_prop_type(node: Node, contents: &[u8], seed: Option<PropType>) -> PropType {
	let mut type_ = seed.unwrap_or_default();
	if node.kind() == "array" {
		// TODO: Is this correct?
		type_.insert(PropType::Optional);
		for child in node.named_children(&mut node.walk()) {
			type_ = parse_prop_type(child, contents, Some(type_));
		}
		return type_;
	}

	fn parse_identifier_prop(node: Node, contents: &[u8], mut type_: PropType) -> PropType {
		debug_assert_eq!(
			node.kind(),
			"identifier",
			"Expected `identifier` node, got {}",
			node.kind()
		);
		match &contents[node.byte_range()] {
			b"String" => type_.insert(PropType::String),
			b"Number" => type_.insert(PropType::Number),
			b"Boolean" => type_.insert(PropType::Boolean),
			b"Object" => type_.insert(PropType::Object),
			b"Function" => type_.insert(PropType::Function),
			b"Array" => type_.insert(PropType::Array),
			_ => {}
		}
		type_
	}

	if node.kind() == "identifier" {
		return parse_identifier_prop(node, contents, type_);
	}

	if node.kind() != "object" {
		return type_;
	}

	// { type: (String | [String, Boolean, ..]), optional?: true }
	for child in node.named_children(&mut node.walk()) {
		if child.kind() == "pair" {
			// (pair left right)
			let prop = child.named_child(0).unwrap();
			let mut range = prop.byte_range();
			if prop.kind() == "string" {
				range = range.shrink(1);
			}
			let value = prop.next_named_sibling().unwrap();
			match &contents[range] {
				b"type" => {
					type_ = parse_prop_type(value, contents, None);
				}
				b"optional" if value.kind() == "true" => {
					type_.insert(PropType::Optional);
				}
				// TODO: Handle 'shape'
				_ => {}
			}
		}
	}

	type_
}

pub type ComponentPrefixTrie = qp_trie::Trie<&'static [u8], ComponentName>;

#[derive(SmartDefault)]
pub struct ComponentIndex {
	#[default(_code = "DashMap::with_shard_amount(4)")]
	inner: DashMap<ComponentName, Component>,
	#[default(_code = "DashMap::with_shard_amount(4)")]
	pub by_template: DashMap<TemplateName, ComponentName>,
	pub by_prefix: RwLock<ComponentPrefixTrie>,
}

impl core::ops::Deref for ComponentIndex {
	type Target = DashMap<ComponentName, Component>;
	#[inline]
	fn deref(&self) -> &Self::Target {
		&self.inner
	}
}

impl DerefMut for ComponentIndex {
	#[inline]
	fn deref_mut(&mut self) -> &mut Self::Target {
		&mut self.inner
	}
}

impl ComponentIndex {
	pub fn extend(&self, components: HashMap<ComponentName, Component>) {
		let mut by_prefix = self.by_prefix.try_write().expect(format_loc!("deadlock"));
		for (name, component) in components {
			by_prefix.insert(_R(name).as_bytes(), name);
			if let Some(ComponentTemplate::Name(template_name)) = component.template.as_ref() {
				self.by_template.insert(*template_name, name);
			}
			self.insert(name, component);
		}
	}
}

/// - `node`: A tree-sitter [Node] from a JS AST
fn registry_category_of_callee<'text>(mut callee: Node, contents: &'text [u8]) -> Option<&'text [u8]> {
	loop {
		// callee ?= registry.category($category)
		if callee.kind() == "call_expression"
			&& let Some(registry_category) = dig!(callee, member_expression)
			&& let Some(registry) = dig!(registry_category, identifier)
			&& b"registry" == &contents[registry.byte_range()]
			&& let Some(prop_category) = dig!(registry_category, property_identifier(1))
			&& b"category" == &contents[prop_category.byte_range()]
			&& let Some(category_node) = dig!(callee, arguments(1).string)
		{
			return Some(&contents[category_node.byte_range().shrink(1)]);
		}

		callee = callee.named_child(0)?;
	}
}

#[cfg(test)]
mod tests {
	use super::registry_category_of_callee;
	use crate::prelude::*;
	use pretty_assertions::assert_eq;

	#[test]
	fn test_registry_category_of_callee() {
		let mut parser = js_parser();
		let contents = br#"registry.category("fields")"#;
		let ast = parser.parse(contents, None).unwrap();
		let call = dig!(ast.root_node(), expression_statement.call_expression).unwrap();
		assert_eq!(registry_category_of_callee(call, contents), Some(b"fields".as_slice()));
	}

	#[test]
	fn test_registry_category_of_callee_nested() {
		let mut parser = js_parser();
		let contents = br#"registry.category("fields").add("foobar").add("barbaz")"#;
		let ast = parser.parse(contents, None).unwrap();
		let call = dig!(ast.root_node(), expression_statement.call_expression).expect(r#"$.add("barbaz")"#);
		assert_eq!(registry_category_of_callee(call, contents), Some(b"fields".as_slice()));
	}
}
