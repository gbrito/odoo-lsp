use std::borrow::Cow;
use std::sync::atomic::Ordering::Relaxed;

use tower_lsp_server::ls_types::*;
use tree_sitter::{QueryCursor, StreamingIterator, Tree};

use crate::prelude::*;

use crate::backend::Backend;
use crate::backend::Text;
use crate::index::{_G, JsQuery};
use crate::model::PropertyKind;
use crate::some;
use crate::utils::{ByteOffset, MaxVec, RangeExt, span_conv, uri_to_path};
use crate::ImStr;
use tracing::instrument;
use ts_macros::query;

query! {
	#[lang = "tree_sitter_javascript"]
	OrmCallQuery(OrmObject, CallMethod, ModelArg, MethodArg);
	// Match this.orm.call('model', 'method')
	(call_expression
		function: (member_expression
			object: (member_expression
				object: (this)
				property: (property_identifier) @ORM_OBJECT (#eq? @ORM_OBJECT "orm"))
			property: (property_identifier) @CALL_METHOD (#eq? @CALL_METHOD "call"))
		arguments: (arguments
			. (string) @MODEL_ARG
			. ","
			. (string) @METHOD_ARG))
}

query! {
	#[lang = "tree_sitter_javascript"]
	JsDocSymbolQuery(ClassName, FunctionName, ExportName);

// Class declarations
(class_declaration
  name: (identifier) @CLASS_NAME)

// Function declarations  
(function_declaration
  name: (identifier) @FUNCTION_NAME)

// Export statements with named exports
(export_statement
  declaration: (lexical_declaration
    (variable_declarator
      name: (identifier) @EXPORT_NAME)))
}

// Query for useService() calls - to provide service name completions
query! {
	#[lang = "tree_sitter_javascript"]
	UseServiceQuery(ServiceArg);
// useService("serviceName")
(call_expression
  function: (identifier) @_fn (#eq? @_fn "useService")
  arguments: (arguments . (string) @SERVICE_ARG))
}

// Query for hook calls - to provide hover and go-to-def
query! {
	#[lang = "tree_sitter_javascript"]
	HookCallQuery(HookName, FirstStringArg);
// Hook calls: useXxx(...) or onXxx(...)
(call_expression
  function: (identifier) @HOOK_NAME
    (#match? @HOOK_NAME "^(use[A-Z]|on(Mounted|WillStart|WillUpdateProps|WillRender|Rendered|Patched|WillUnmount|WillDestroy|Error))")
  arguments: (arguments . (string)? @FIRST_STRING_ARG))
}

/// Javascript extensions.
impl Backend {
	pub fn on_change_js(
		&self,
		text: &Text,
		uri: &Uri,
		rope: RopeSlice<'_>,
		old_rope: Option<Rope>,
	) -> anyhow::Result<()> {
		let parser = js_parser();
		self.update_ast(text, uri, rope, old_rope, parser)
	}
	pub fn js_jump_def(&self, params: GotoDefinitionParams, rope: RopeSlice<'_>) -> anyhow::Result<Option<Location>> {
		let uri = &params.text_document_position_params.text_document.uri;
		let file_path = uri_to_path(uri)?;
		let ast = self
			.ast_map
			.get(file_path.to_str().ok_or_else(|| errloc!("non-utf8 path: {:?}", file_path))?)
			.ok_or_else(|| errloc!("Did not build AST for {}", uri.path().as_str()))?;
		let ByteOffset(offset) = rope_conv(params.text_document_position_params.position, rope);
		let contents = Cow::from(rope);

		// try templates first
		let query = JsQuery::query();
		let mut cursor = QueryCursor::new();
		let mut matches = cursor.matches(query, ast.root_node(), contents.as_bytes());
		while let Some(match_) = matches.next() {
			for capture in match_.captures {
				let range = capture.node.byte_range();
				if capture.index == JsQuery::TemplateName as u32 && range.contains(&offset) {
					let key = some!(_G(&contents[range.shrink(1)]));
					return Ok(some!(self.index.templates.get(&key)).location.clone().map(Into::into));
				}
			}
		}

		// try gotodefs for useService() - jump to service registration
		{
			let query = UseServiceQuery::query();
			let mut cursor = QueryCursor::new();
			let mut matches = cursor.matches(query, ast.root_node(), contents.as_bytes());
			while let Some(match_) = matches.next() {
				for capture in match_.captures {
					if capture.index == UseServiceQuery::ServiceArg as u32 {
						let range = capture.node.byte_range();
						if range.contains(&offset) {
							let service_name = &contents[range.shrink(1)];
							if let Some(service) = self.index.services.get(&ImStr::from(service_name.as_ref())) {
								if let Some(loc) = &service.location {
									return Ok(Some(loc.clone().into()));
								}
							}
						}
					}
				}
			}
		}

		// try gotodefs for ORM calls
		let query = OrmCallQuery::query();
		let mut cursor = QueryCursor::new();
		let mut matches = cursor.matches(query, ast.root_node(), contents.as_bytes());
		while let Some(match_) = matches.next() {
			let mut model_arg_node = None;
			let mut method_arg_node = None;

			for capture in match_.captures {
				match OrmCallQuery::from(capture.index) {
					Some(OrmCallQuery::ModelArg) => {
						model_arg_node = Some(capture.node);
					}
					Some(OrmCallQuery::MethodArg) => {
						method_arg_node = Some(capture.node);
					}
					_ => {}
				}
			}

			if let Some(model_node) = model_arg_node
				&& let range = model_node.byte_range()
				&& range.contains_end(offset)
			{
				let range = range.shrink(1);
				let model = &contents[range];
				return self.index.jump_def_model(model);
			}

			if let Some(model_node) = model_arg_node
				&& let Some(method_node) = method_arg_node
				&& let range = method_node.byte_range()
				&& range.contains_end(offset)
			{
				let model = &contents[model_node.byte_range().shrink(1)];
				let method = &contents[range.shrink(1)];
				return self.index.jump_def_property_name(method, model);
			}
		}

		Ok(None)
	}
	pub fn js_references(&self, params: ReferenceParams, rope: RopeSlice<'_>) -> anyhow::Result<Option<Vec<Location>>> {
		let uri = &params.text_document_position.text_document.uri;
		let file_path = uri_to_path(uri)?;
		let ast = self
			.ast_map
			.get(file_path.to_str().ok_or_else(|| errloc!("non-utf8 path: {:?}", file_path))?)
			.ok_or_else(|| errloc!("Did not build AST for {}", uri.path().as_str()))?;
		let ByteOffset(offset) = rope_conv(params.text_document_position.position, rope);
		let contents = Cow::from(rope);
		let query = JsQuery::query();
		let mut cursor = QueryCursor::new();
		let mut matches = cursor.matches(query, ast.root_node(), contents.as_bytes());
		while let Some(match_) = matches.next() {
			for capture in match_.captures {
				let range = capture.node.byte_range();
				if capture.index == JsQuery::TemplateName as u32 && range.contains(&offset) {
					let key = &contents[range.shrink(1)];
					let key = some!(_G(key));
					let template = some!(self.index.templates.get(&key));
					return Ok(Some(
						template
							.descendants
							.iter()
							.flat_map(|tpl| tpl.location.clone().map(Into::into))
							.collect(),
					));
				}
			}
		}

		Ok(None)
	}

	/// Prepares a rename operation by identifying the symbol at the cursor position.
	/// Returns the symbol and its range if it can be renamed, None otherwise.
	pub fn js_prepare_rename(
		&self,
		params: TextDocumentPositionParams,
		rope: RopeSlice<'_>,
	) -> anyhow::Result<Option<(crate::backend::RenameableSymbol, Range)>> {
		use crate::backend::RenameableSymbol;

		let uri = &params.text_document.uri;
		let file_path = uri_to_path(uri)?;
		let ast = self
			.ast_map
			.get(file_path.to_str().ok_or_else(|| errloc!("non-utf8 path: {:?}", file_path))?)
			.ok_or_else(|| errloc!("Did not build AST for {}", uri.path().as_str()))?;
		let ByteOffset(offset) = rope_conv(params.position, rope);
		let contents = Cow::from(rope);
		let query = JsQuery::query();
		let mut cursor = QueryCursor::new();
		let mut matches = cursor.matches(query, ast.root_node(), contents.as_bytes());
		while let Some(match_) = matches.next() {
			for capture in match_.captures {
				let range = capture.node.byte_range();
				if capture.index == JsQuery::TemplateName as u32 && range.contains(&offset) {
					let inner_range = range.shrink(1);
					let template_name = &contents[inner_range.clone()];
					let lsp_range = rope_conv(inner_range.map_unit(ByteOffset), rope);
					return Ok(Some((
						RenameableSymbol::TemplateName(template_name.to_string()),
						lsp_range,
					)));
				}
			}
		}

		Ok(None)
	}

	pub fn js_hover(&self, params: HoverParams, rope: RopeSlice<'_>) -> anyhow::Result<Option<Hover>> {
		let uri = &params.text_document_position_params.text_document.uri;
		let file_path = uri_to_path(uri)?;
		let ast = self
			.ast_map
			.get(file_path.to_str().ok_or_else(|| errloc!("non-utf8 path: {:?}", file_path))?)
			.ok_or_else(|| errloc!("Did not build AST for {}", uri.path().as_str()))?;
		let ByteOffset(offset) = rope_conv(params.text_document_position_params.position, rope);
		let contents = Cow::from(rope);
		let query = JsQuery::query();
		let mut cursor = QueryCursor::new();
		let mut matches = cursor.matches(query, ast.root_node(), contents.as_bytes());
		while let Some(match_) = matches.next() {
			for capture in match_.captures {
				let range = capture.node.byte_range();
				if capture.index == JsQuery::TemplateName as u32 && range.contains(&offset) {
					return Ok(self
						.index
						.hover_template(&contents[range.shrink(1)], Some(span_conv(capture.node.range()))));
				}
				if capture.index == JsQuery::Name as u32 && range.contains(&offset) {
					return Ok(self
						.index
						.hover_component(&contents[range], Some(span_conv(capture.node.range()))));
				}
			}
		}

		// try hover for hook calls (hook name and service argument)
		{
			let query = HookCallQuery::query();
			let mut cursor = QueryCursor::new();
			let mut matches = cursor.matches(query, ast.root_node(), contents.as_bytes());
			while let Some(match_) = matches.next() {
				let mut hook_name_node = None;
				let mut first_string_arg_node = None;

				for capture in match_.captures {
					match HookCallQuery::from(capture.index) {
						Some(HookCallQuery::HookName) => {
							hook_name_node = Some(capture.node);
						}
						Some(HookCallQuery::FirstStringArg) => {
							first_string_arg_node = Some(capture.node);
						}
						None => {}
					}
				}

				// Check if hovering over hook name
				if let Some(hook_node) = hook_name_node {
					let range = hook_node.byte_range();
					if range.contains(&offset) {
						let hook_name = &contents[range.clone()];
						if let Some(hook_def) = self.index.hooks.get(&ImStr::from(hook_name.as_ref())) {
							let mut markdown = format!("**{}**\n\n```typescript\n{}\n```", hook_def.name, hook_def.signature);
							if let Some(desc) = &hook_def.description {
								markdown.push_str("\n\n");
								markdown.push_str(desc);
							}
							markdown.push_str(&format!("\n\n*Source: {}*", hook_def.source_module));

							return Ok(Some(Hover {
								contents: HoverContents::Markup(MarkupContent {
									kind: MarkupKind::Markdown,
									value: markdown,
								}),
								range: Some(span_conv(hook_node.range())),
							}));
						}
					}
				}

				// Check if hovering over service name in useService("xxx")
				if let Some(hook_node) = hook_name_node {
					let hook_name = &contents[hook_node.byte_range()];
					if hook_name == "useService" {
						if let Some(arg_node) = first_string_arg_node {
							let range = arg_node.byte_range();
							if range.contains(&offset) {
								let service_name = &contents[range.shrink(1)];
								if let Some(service) = self.index.services.get(&ImStr::from(service_name.as_ref())) {
									let mut markdown = format!("**Service: {}**", service.name);
									
									if !service.async_methods.is_empty() {
										markdown.push_str("\n\n**Async methods:** ");
										markdown.push_str(&service.async_methods.iter().map(|m| format!("`{}`", m)).collect::<Vec<_>>().join(", "));
									}
									
									if !service.dependencies.is_empty() {
										markdown.push_str("\n\n**Dependencies:** ");
										markdown.push_str(&service.dependencies.iter().map(|d| format!("`{}`", d)).collect::<Vec<_>>().join(", "));
									}
									
									if let Some(module) = &service.module {
										markdown.push_str(&format!("\n\n*Module: {}*", module));
									}

									return Ok(Some(Hover {
										contents: HoverContents::Markup(MarkupContent {
											kind: MarkupKind::Markdown,
											value: markdown,
										}),
										range: Some(span_conv(arg_node.range())),
									}));
								} else {
									// Unknown service
									return Ok(Some(Hover {
										contents: HoverContents::Markup(MarkupContent {
											kind: MarkupKind::Markdown,
											value: format!("**Unknown service:** `{}`", service_name),
										}),
										range: Some(span_conv(arg_node.range())),
									}));
								}
							}
						}
					}
				}
			}
		}

		// try hover for ORM calls
		let query = OrmCallQuery::query();
		let mut cursor = QueryCursor::new();
		let mut matches = cursor.matches(query, ast.root_node(), contents.as_bytes());
		while let Some(match_) = matches.next() {
			let mut model_arg_node = None;
			let mut method_arg_node = None;

			for capture in match_.captures {
				match OrmCallQuery::from(capture.index) {
					Some(OrmCallQuery::ModelArg) => {
						model_arg_node = Some(capture.node);
					}
					Some(OrmCallQuery::MethodArg) => {
						method_arg_node = Some(capture.node);
					}
					_ => {}
				}
			}

			if let Some(model_node) = model_arg_node
				&& let range = model_node.byte_range()
				&& range.contains_end(offset)
			{
				let range = range.shrink(1);
				let model = &contents[range.clone()];
				return (self.index).hover_model(model, Some(rope_conv(range.map_unit(ByteOffset), rope)), false, None);
			}

			if let Some(model_node) = model_arg_node
				&& let Some(method_node) = method_arg_node
				&& let range = method_node.byte_range()
				&& range.contains_end(offset)
			{
				let range = range.shrink(1);
				let model = &contents[model_node.byte_range().shrink(1)];
				let method = &contents[range.clone()];
				return self.index.hover_property_name(
					method,
					model,
					Some(rope_conv(range.map_unit(ByteOffset), rope)),
				);
			}
		}

		Ok(None)
	}

	#[instrument(skip_all)]
	pub async fn js_completions(
		&self,
		params: CompletionParams,
		ast: Tree,
		rope: RopeSlice<'_>,
	) -> anyhow::Result<Option<CompletionResponse>> {
		let position = params.text_document_position.position;
		let ByteOffset(offset) = rope_conv(position, rope);
		let path = some!(params.text_document_position.text_document.uri.to_file_path());
		let completions_limit = self
			.workspaces
			.find_workspace_of(&path, |_, ws| ws.completions.limit)
			.unwrap_or_else(|| self.project_config.completions_limit.load(Relaxed));

		let contents = Cow::from(rope);

		// Check for service name completion in useService("")
		{
			let query = UseServiceQuery::query();
			let mut cursor = QueryCursor::new();
			let mut matches = cursor.matches(query, ast.root_node(), contents.as_bytes());
			while let Some(match_) = matches.next() {
				for capture in match_.captures {
					if capture.index == UseServiceQuery::ServiceArg as u32 {
						let range = capture.node.byte_range();
						let inner_range = range.clone().shrink(1);
						// Check if cursor is inside the string (including empty strings)
						// For empty strings "", range is 2 bytes and inner_range is empty (start == end)
						// We accept cursor positions from range.start (opening quote) to inner_range.end
						// to handle both empty and non-empty strings
						let is_inside = if inner_range.start == inner_range.end {
							// Empty string: accept if cursor is at opening quote or between quotes
							offset >= range.start && offset <= inner_range.end
						} else {
							// Non-empty string: cursor must be inside content
							inner_range.start <= offset && offset <= inner_range.end
						};
						if is_inside {
							// For empty strings or when cursor is before content, prefix is empty
							let prefix = if offset >= inner_range.start {
								&contents[inner_range.start..offset]
							} else {
								""
							};
							let lsp_range = rope_conv(inner_range.map_unit(ByteOffset), rope);

							let mut items = Vec::new();
							for service in self.index.services.iter() {
								if service.name.starts_with(prefix) {
									let mut detail = String::new();
									if !service.async_methods.is_empty() {
										detail.push_str(&format!("async: {}", service.async_methods.len()));
									}
									if let Some(module) = &service.module {
										if !detail.is_empty() {
											detail.push_str(" | ");
										}
										detail.push_str(&format!("module: {}", module));
									}

									items.push(CompletionItem {
										label: service.name.to_string(),
										kind: Some(CompletionItemKind::VALUE),
										detail: if detail.is_empty() { None } else { Some(detail) },
										text_edit: Some(CompletionTextEdit::Edit(TextEdit {
											range: lsp_range,
											new_text: service.name.to_string(),
										})),
										..Default::default()
									});
								}
							}

							// Sort by name
							items.sort_by(|a, b| a.label.cmp(&b.label));

							return Ok(Some(CompletionResponse::List(CompletionList {
								is_incomplete: false,
								items,
							})));
						}
					}
				}
			}
		}

		// Check for hook name completion (when typing an identifier that starts with "use" or "on")
		// We need to detect if user is typing a potential hook call
		if let Some(node) = ast.root_node().named_descendant_for_byte_range(offset.saturating_sub(10), offset) {
			// Check if we're in an identifier context that could be a hook
			let in_identifier = node.kind() == "identifier" 
				|| node.kind() == "property_identifier"
				|| (node.kind() == "call_expression" && offset <= node.byte_range().start + 20);
			
			if in_identifier {
				// Find the identifier text being typed
				let start = if node.kind() == "identifier" || node.kind() == "property_identifier" {
					node.byte_range().start
				} else {
					// Try to find start of current word
					let mut start = offset;
					while start > 0 && contents.as_bytes().get(start - 1).is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_') {
						start -= 1;
					}
					start
				};
				
				let prefix = &contents[start..offset];
				
				// Only complete if prefix looks like a hook (starts with "use" or "on")
				let is_hook_prefix = prefix.starts_with("use") || prefix.starts_with("on") 
					|| prefix == "us" || prefix == "u" || prefix == "o";
				
				if is_hook_prefix && prefix.len() >= 1 {
					let lifecycle_only = prefix.starts_with("on") || prefix == "o";
					let lsp_range = rope_conv(ByteOffset(start)..ByteOffset(offset), rope);
					
					let mut items = Vec::new();
					for hook in self.index.hooks.iter() {
						// Filter based on prefix type
						let hook_is_lifecycle = hook.name.starts_with("on");
						if lifecycle_only && !hook_is_lifecycle {
							continue;
						}
						if !lifecycle_only && hook_is_lifecycle {
							continue;
						}
						
						if hook.name.starts_with(prefix) {
							items.push(CompletionItem {
								label: hook.name.to_string(),
								kind: Some(CompletionItemKind::FUNCTION),
								detail: Some(hook.signature.to_string()),
								documentation: hook.description.as_ref().map(|d| {
									Documentation::MarkupContent(MarkupContent {
										kind: MarkupKind::Markdown,
										value: format!("{}\n\n*Source: {}*", d, hook.source_module),
									})
								}),
								insert_text: Some(format!("{}($0)", hook.name)),
								insert_text_format: Some(InsertTextFormat::SNIPPET),
								text_edit: Some(CompletionTextEdit::Edit(TextEdit {
									range: lsp_range,
									new_text: hook.name.to_string(),
								})),
								..Default::default()
							});
						}
					}
					
					if !items.is_empty() {
						// Sort by name
						items.sort_by(|a, b| a.label.cmp(&b.label));
						
						return Ok(Some(CompletionResponse::List(CompletionList {
							is_incomplete: false,
							items,
						})));
					}
				}
			}
		}

		// Check for ORM call completions
		let query = OrmCallQuery::query();
		let mut cursor = QueryCursor::new();

		// Find the orm.call node that contains the cursor position
		let mut matches = cursor.matches(query, ast.root_node(), contents.as_bytes());
		while let Some(match_) = matches.next() {
			let mut model_arg_node = None;
			let mut method_arg_node = None;

			for capture in match_.captures {
				match OrmCallQuery::from(capture.index) {
					Some(OrmCallQuery::ModelArg) => {
						model_arg_node = Some(capture.node);
					}
					Some(OrmCallQuery::MethodArg) => {
						method_arg_node = Some(capture.node);
					}
					_ => {}
				}
			}

			// Check if cursor is within the model argument
			if let Some(model_node) = model_arg_node {
				let range = model_node.byte_range();
				if range.contains(&offset) {
					// Extract the current prefix (excluding quotes)
					let inner_range = range.shrink(1);
					let prefix = &contents[inner_range.start..offset];
					let lsp_range = rope_conv(inner_range.map_unit(ByteOffset), rope);
					let mut items = MaxVec::new(completions_limit);
					self.index.complete_model(prefix, lsp_range, &mut items)?;

					return Ok(Some(CompletionResponse::List(CompletionList {
						is_incomplete: !items.has_space(),
						items: items.into_inner(),
					})));
				}
			}

			// Check if cursor is within the method argument
			if let Some(method_node) = method_arg_node {
				let range = method_node.byte_range();
				let inner_range = range.clone().shrink(1);
				// Check if cursor is inside the string (including empty strings)
				let is_inside = if inner_range.start == inner_range.end {
					offset >= range.start && offset <= inner_range.end
				} else {
					inner_range.start <= offset && offset <= inner_range.end
				};
				if is_inside {
					// Extract the model name from the first argument
					if let Some(model_node) = model_arg_node {
						let model_range = model_node.byte_range().shrink(1);
						let model_name = &contents[model_range];

						// Extract the current method prefix (excluding quotes)
						let prefix = if offset >= inner_range.start {
							&contents[inner_range.start..offset]
						} else {
							""
						};

						let byte_range = inner_range.map_unit(ByteOffset);

						let mut items = MaxVec::new(completions_limit);
						self.index.complete_property_name(
							prefix,
							byte_range,
							model_name.into(),
							rope,
							Some(PropertyKind::Method),
							None,
							true,
							true,
							&mut items,
						)?;

						return Ok(Some(CompletionResponse::List(CompletionList {
							is_incomplete: !items.has_space(),
							items: items.into_inner(),
						})));
					}
				}
			}

			// Check if cursor is in a position where we should start a new string argument
			// This handles cases where the user is typing after the comma but hasn't started the string yet
			if let Some(model_node) = model_arg_node {
				let contents_bytes = contents.as_bytes();
				let model_end = model_node.byte_range().end;
				// Look for comma after model argument
				let mut i = model_end;
				while i < contents_bytes.len() && contents_bytes[i].is_ascii_whitespace() {
					i += 1;
				}
				if i < contents_bytes.len() && contents_bytes[i] == b',' {
					i += 1;
					// Skip whitespace after comma
					while i < contents_bytes.len() && contents_bytes[i].is_ascii_whitespace() {
						i += 1;
					}
					// If cursor is at or after this position and before any method argument
					if offset >= i
						&& (method_arg_node.is_none() || offset < method_arg_node.unwrap().byte_range().start)
					{
						// We're completing the method name
						let model_range = model_node.byte_range().shrink(1);
						let model_name = &contents[model_range];

						let synthetic_range = ByteOffset(i)..ByteOffset(offset.max(i));

						let mut items = MaxVec::new(100);
						self.index.complete_property_name(
							"",
							synthetic_range,
							model_name.into(),
							rope,
							Some(PropertyKind::Method),
							None,
							true,
							true,
							&mut items,
						)?;

						return Ok(Some(CompletionResponse::List(CompletionList {
							is_incomplete: false,
							items: items.into_inner(),
						})));
					}
				}
			}
		}

		Ok(None)
	}

	/// Extract document symbols from JavaScript/OWL files.
	/// Returns OWL components and exported functions/classes as symbols.
	pub fn js_document_symbols(
		&self,
		uri: &Uri,
		rope: RopeSlice<'_>,
	) -> anyhow::Result<Option<Vec<DocumentSymbol>>> {
		let file_path = uri_to_path(uri)?;
		let file_path_str = file_path.to_str().ok_or_else(|| errloc!("non-utf8 path: {:?}", file_path))?;
		let ast = some!(self.ast_map.get(file_path_str));
		let contents = Cow::from(rope);

		let query = JsDocSymbolQuery::query();
		let mut cursor = QueryCursor::new();
		let mut symbols = Vec::new();

		let mut matches = cursor.matches(query, ast.root_node(), contents.as_bytes());
		while let Some(match_) = matches.next() {
			for capture in match_.captures {
				match JsDocSymbolQuery::from(capture.index) {
					Some(JsDocSymbolQuery::ClassName) => {
						let node = capture.node;
						let name = &contents[node.byte_range()];
						let parent = node.parent();
						let class_node = parent.and_then(|p| if p.kind() == "class_declaration" || p.kind() == "class" { Some(p) } else { None });

						let range = class_node.map(|c| span_conv(c.range())).unwrap_or_else(|| span_conv(node.range()));
						let selection_range = span_conv(node.range());

						#[allow(deprecated)]
						symbols.push(DocumentSymbol {
							name: name.to_string(),
							detail: Some("class".to_string()),
							kind: SymbolKind::CLASS,
							tags: None,
							deprecated: None,
							range,
							selection_range,
							children: None,
						});
					}
					Some(JsDocSymbolQuery::FunctionName) => {
						let node = capture.node;
						let name = &contents[node.byte_range()];
						let parent = node.parent();
						let func_node = parent.and_then(|p| if p.kind() == "function_declaration" { Some(p) } else { None });

						let range = func_node.map(|f| span_conv(f.range())).unwrap_or_else(|| span_conv(node.range()));
						let selection_range = span_conv(node.range());

						#[allow(deprecated)]
						symbols.push(DocumentSymbol {
							name: name.to_string(),
							detail: Some("function".to_string()),
							kind: SymbolKind::FUNCTION,
							tags: None,
							deprecated: None,
							range,
							selection_range,
							children: None,
						});
					}
					Some(JsDocSymbolQuery::ExportName) => {
						let node = capture.node;
						let name = &contents[node.byte_range()];
						let selection_range = span_conv(node.range());

						// Check if already added (avoid duplicates with class/function)
						if symbols.iter().any(|s| s.name == name) {
							continue;
						}

						#[allow(deprecated)]
						symbols.push(DocumentSymbol {
							name: name.to_string(),
							detail: Some("export".to_string()),
							kind: SymbolKind::VARIABLE,
							tags: None,
							deprecated: None,
							range: selection_range,
							selection_range,
							children: None,
						});
					}
					_ => {}
				}
			}
		}

		if symbols.is_empty() {
			Ok(None)
		} else {
			Ok(Some(symbols))
		}
	}

	/// Diagnose JavaScript/OWL files for issues.
	/// Currently checks:
	/// - Unknown service names in useService()
	pub fn js_diagnostics(&self, rope: RopeSlice<'_>) -> Vec<Diagnostic> {
		let contents = Cow::from(rope);
		let mut parser = js_parser();
		let Some(ast) = parser.parse(contents.as_bytes(), None) else {
			return vec![];
		};

		let mut diagnostics = Vec::new();

		// Check for unknown service names in useService()
		{
			let query = UseServiceQuery::query();
			let mut cursor = QueryCursor::new();
			let mut matches = cursor.matches(query, ast.root_node(), contents.as_bytes());
			while let Some(match_) = matches.next() {
				for capture in match_.captures {
					if capture.index == UseServiceQuery::ServiceArg as u32 {
						let range = capture.node.byte_range();
						let service_name = &contents[range.shrink(1)];
						
						// Skip empty strings (placeholder during typing)
						if service_name.is_empty() {
							continue;
						}
						
						// Check if service exists
						if !self.index.services.contains_key(&ImStr::from(service_name.as_ref())) {
							diagnostics.push(Diagnostic {
								range: span_conv(capture.node.range()),
								severity: Some(DiagnosticSeverity::WARNING),
								code: Some(NumberOrString::String("unknown-service".to_string())),
								source: Some("odoo-lsp".to_string()),
								message: format!("Unknown service: '{}'", service_name),
								related_information: None,
								tags: None,
								code_description: None,
								data: None,
							});
						}
					}
				}
			}
		}

		diagnostics
	}
}
