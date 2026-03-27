use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::Ordering::Relaxed;

use ropey::Rope;
use serde_json::Value;
use tower_lsp_server::LanguageServer;
use tower_lsp_server::jsonrpc::Result;
use tower_lsp_server::ls_types::notification::{DidChangeConfiguration, Notification};
use tower_lsp_server::ls_types::request::WorkDoneProgressCreate;
use tower_lsp_server::ls_types::*;
use tracing::{debug, error, info, instrument, warn};

use crate::xml::add_xml_snippets;
use crate::{GITVER, NAME, VERSION, await_did_open_document, format_loc, loc};

use crate::backend::{Backend, Document, Language, Text};
use crate::index::{_G, _I, _R};
use crate::{backend, some, utils::*};

// Helper methods for call hierarchy lazy evaluation
impl Backend {
	/// Trigger lazy evaluation of a specific method to ensure its calls are collected.
	fn ensure_method_evaluated(&self, model: &str, method: &str) {
		use crate::model::Method;

		if let Some(model_key) = _G(model) {
			let method_sym: crate::index::Symbol<Method> = _I(method).into();
			// This triggers eval_method_rtype which collects calls.
			// Returns None if the method doesn't exist or has no return type, which is expected.
			let _ = self.index.eval_method_rtype(method_sym, model_key, None);
		}
	}

	/// Trigger lazy evaluation of all methods with the given name across all models.
	/// This is needed for incoming calls because any model could be calling the method.
	fn ensure_all_methods_evaluated(&self, method_name: &str) {
		use crate::model::Method;

		let method_sym: crate::index::Symbol<Method> = _I(method_name).into();

		// Iterate through all models and evaluate any method with this name
		for entry in self.index.models.iter() {
			let model_key = *entry.key();
			// Ensure properties are populated. Returns None if model not found, which is ok.
			let _ = self.index.models.populate_properties(model_key, &[]);

			// Check if this model has the method
			if let Some(model) = self.index.models.get(&model_key) {
				if let Some(methods) = &model.methods {
					if methods.contains_key(&method_sym) {
						// Returns None if no return type, which is expected.
						let _ = self.index.eval_method_rtype(method_sym, model_key.into(), None);
					}
				}
			}
		}
	}

	/// Trigger lazy evaluation of a function to ensure its calls are collected.
	fn ensure_function_evaluated(&self, path: &str, name: &str) {
		use crate::model::Function;

		let file_path = std::path::Path::new(path);
		let func_sym: crate::index::Symbol<Function> = _I(name).into();

		if let Some((path_sym, _)) = self.index.functions.find_in_file(file_path, &func_sym) {
			// This triggers eval_function_rtype which collects calls.
			// Returns None if no return type, which is expected.
			let _ = self.index.eval_function_rtype(func_sym, path_sym);
		}
	}
}

impl LanguageServer for Backend {
	#[instrument(skip_all, fields(params), ret)]
	async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
		self.init_workspaces(&params);

		if let Some(WorkspaceClientCapabilities {
			did_change_configuration:
				Some(DynamicRegistrationClientCapabilities {
					dynamic_registration: Some(true),
				}),
			..
		}) = params.capabilities.workspace.as_ref()
		{
			self.capabilities.can_notify_changed_config.store(true, Relaxed);
		}

		if let Some(WorkspaceClientCapabilities {
			did_change_watched_files:
				Some(DidChangeWatchedFilesClientCapabilities {
					dynamic_registration: Some(true),
					..
				}),
			..
		}) = params.capabilities.workspace.as_ref()
		{
			self.capabilities.can_notify_changed_watched_files.store(true, Relaxed);
		}
		if let Some(WorkspaceClientCapabilities {
			workspace_folders: Some(true),
			..
		}) = params.capabilities.workspace.as_ref()
		{
			self.capabilities.workspace_folders.store(true, Relaxed);
		}

		if let Some(TextDocumentClientCapabilities {
			diagnostic: Some(..), ..
		}) = params.capabilities.text_document
		{
			debug!("Client supports pull diagnostics");
			self.capabilities.pull_diagnostics.store(true, Relaxed);
		}

		if let Some(WindowClientCapabilities {
			work_done_progress: Some(true),
			..
		}) = params.capabilities.window
		{
			self.capabilities.can_create_wdp.store(true, Relaxed);
		}

		Ok(InitializeResult {
			server_info: Some(ServerInfo {
				name: NAME.to_string(),
				version: Some(format!("v{VERSION} git:{GITVER}")),
			}),
			offset_encoding: None,
			capabilities: ServerCapabilities {
				definition_provider: Some(OneOf::Left(true)),
				hover_provider: Some(HoverProviderCapability::Simple(true)),
				references_provider: Some(OneOf::Left(true)),
				workspace_symbol_provider: Some(OneOf::Left(true)),
				diagnostic_provider: Some(DiagnosticServerCapabilities::Options(Default::default())),
				// XML code actions are done in 1 pass only
				code_action_provider: Some(CodeActionProviderCapability::Simple(true)),
				execute_command_provider: Some(ExecuteCommandOptions {
					commands: vec!["goto_owl".to_string(), "jump_view".to_string()],
					..Default::default()
				}),
				text_document_sync: Some(TextDocumentSyncCapability::Options(TextDocumentSyncOptions {
					change: Some(TextDocumentSyncKind::INCREMENTAL),
					save: Some(TextDocumentSyncSaveOptions::Supported(true)),
					open_close: Some(true),
					..Default::default()
				})),
				completion_provider: Some(CompletionOptions {
					resolve_provider: Some(true),
					trigger_characters: Some(
						['"', '\'', '.', '_', ',', ' ', '(', ')']
							.iter()
							.map(char::to_string)
							.collect(),
					),
					all_commit_characters: None,
					work_done_progress_options: Default::default(),
					completion_item: Some(CompletionOptionsCompletionItem {
						label_details_support: Some(true),
					}),
				}),
				signature_help_provider: Some(SignatureHelpOptions {
					trigger_characters: Some(['('].iter().map(char::to_string).collect()),
					retrigger_characters: Some([','].iter().map(char::to_string).collect()),
					work_done_progress_options: Default::default(),
				}),
				document_symbol_provider: Some(OneOf::Left(true)),
				inlay_hint_provider: Some(OneOf::Right(InlayHintServerCapabilities::Options(
					InlayHintOptions {
						resolve_provider: Some(false),
						work_done_progress_options: Default::default(),
					},
				))),
				semantic_tokens_provider: Some(SemanticTokensServerCapabilities::SemanticTokensOptions(
					SemanticTokensOptions {
						legend: SemanticTokensLegend {
							token_types: vec![
								SemanticTokenType::CLASS,     // 0: Model names
								SemanticTokenType::PROPERTY,  // 1: Field names
								SemanticTokenType::METHOD,    // 2: Method names
								SemanticTokenType::DECORATOR, // 3: @api.* decorators
								SemanticTokenType::TYPE,      // 4: Field types (fields.Char, etc.)
								SemanticTokenType::STRING,    // 5: XML IDs in strings
							],
							token_modifiers: vec![
								SemanticTokenModifier::DEFINITION,
								SemanticTokenModifier::READONLY,
								SemanticTokenModifier::DEFAULT_LIBRARY,
							],
						},
						range: Some(true),
						full: Some(SemanticTokensFullOptions::Bool(true)),
						..Default::default()
					},
				)),
				// Note: type_hierarchy_provider is not available in tower-lsp-server 0.23.0
				workspace: Some(WorkspaceServerCapabilities {
					workspace_folders: Some(WorkspaceFoldersServerCapabilities {
						supported: Some(true),
						change_notifications: Some(OneOf::Left(true)),
					}),
					file_operations: None,
				}),
				rename_provider: Some(OneOf::Right(RenameOptions {
					prepare_provider: Some(true),
					work_done_progress_options: Default::default(),
				})),
				call_hierarchy_provider: Some(CallHierarchyServerCapability::Simple(true)),
				..ServerCapabilities::default()
			},
		})
	}
	#[instrument(skip_all)]
	async fn shutdown(&self) -> Result<()> {
		Ok(())
	}
	#[instrument(skip_all, ret, fields(uri = params.text_document.uri.as_str()))]
	async fn did_close(&self, params: DidCloseTextDocumentParams) {
		let path = params.text_document.uri.path().as_str();
		await_did_open_document!(self, path);

		self.document_map.remove(path);
		self.record_ranges.remove(path);

		let file_path = params.text_document.uri.to_file_path().unwrap();
		self.ast_map.remove(file_path.to_str().unwrap());

		self.client
			.publish_diagnostics(params.text_document.uri, vec![], None)
			.await;
	}
	#[instrument(skip_all, ret)]
	async fn initialized(&self, _: InitializedParams) {
		let mut registrations = vec![];
		if self.capabilities.can_notify_changed_config.load(Relaxed) {
			registrations.push(Registration {
				id: "odoo-lsp/did-change-config".to_string(),
				method: DidChangeConfiguration::METHOD.to_string(),
				register_options: None,
			});
		}
		if self.capabilities.can_notify_changed_watched_files.load(Relaxed) {
			registrations.push(Registration {
				id: "odoo-lsp/did-change-odoo-lsp".to_string(),
				method: notification::DidChangeWatchedFiles::METHOD.to_string(),
				register_options: Some(
					serde_json::to_value(DidChangeWatchedFilesRegistrationOptions {
						watchers: vec![FileSystemWatcher {
							glob_pattern: GlobPattern::String("**/.odoo_lsp{,.json}".to_string()),
							kind: Some(WatchKind::Create | WatchKind::Change),
						}],
					})
					.unwrap(),
				),
			});
		}

		if !registrations.is_empty() {
			_ = self.client.register_capability(registrations).await;
		}

		let _blocker = unsafe { self.root_setup.block_unchecked(loc!()) };
		self.ensure_nonoverlapping_roots();
		info!(workspaces = ?self.workspaces);

		for ws in self.workspaces.iter() {
			if let Err(err) = (self.index)
				.add_root(Path::new(ws.key()), Some(self.client.clone()))
				.await
			{
				error!("could not add root {}:\n{err}", ws.key().display());
			}
		}
	}
	#[instrument(skip_all, ret, fields(uri=params.text_document.uri.as_str()))]
	async fn did_open(&self, params: DidOpenTextDocumentParams) {
		self.root_setup.wait(loc!()).await;
		// NB: fixes a race condition where completions are requested even before
		// did_open had a chance to put in a blocker for the document, leading to
		// flaky tests and the first completion request yielding nothing (super minor issue)
		let _blocker = self.root_setup.block(loc!());

		let file_path = params.text_document.uri.to_file_path().unwrap();
		let file_path_str = file_path.to_str().unwrap();
		info!("{}", file_path_str);
		let language_id = params.text_document.language_id.as_str();
		let split_uri = file_path_str.rsplit_once('.');
		let language = match (language_id, split_uri) {
			("python", _) | (_, Some((_, "py"))) => Language::Python,
			("javascript", _) | (_, Some((_, "js"))) => Language::Javascript,
			("xml", _) | (_, Some((_, "xml"))) => Language::Xml,
			("csv", _) | (_, Some((_, "csv"))) => Language::Csv,
			_ => {
				debug!(
					"Could not determine language, or language not supported:\nlanguage_id={language_id} split_uri={split_uri:?}"
				);
				return;
			}
		};

		let mut progress = None;
		let token = ProgressToken::String(format!("odoo_lsp_open:{file_path_str}"));
		if self.capabilities.can_create_wdp.load(Relaxed)
			&& self
				.client
				.send_request::<WorkDoneProgressCreate>(WorkDoneProgressCreateParams { token: token.clone() })
				.await
				.is_ok()
		{
			progress = Some(
				self.client
					.progress(token, format!("Opening {file_path_str}"))
					.begin()
					.await,
			);
		}

		let rope = Rope::from_str(&params.text_document.text);
		self.document_map.insert(
			params.text_document.uri.path().as_str().to_string(),
			Document::new(rope.clone()),
		);

		self.index.add_root_for_file(&file_path).await;

		_ = self
			.on_change(backend::TextDocumentItem {
				uri: params.text_document.uri,
				text: Text::Full(params.text_document.text),
				version: params.text_document.version,
				language: Some(language),
				old_rope: None,
				open: true,
			})
			.await
			.inspect_err(|err| warn!("{err}"));

		if let Some(progress) = progress {
			tokio::spawn(progress.finish());
		}
	}
	#[instrument(skip_all, ret, fields(uri = params.text_document.uri.as_str()))]
	async fn did_change(&self, mut params: DidChangeTextDocumentParams) {
		self.root_setup.wait(loc!()).await;
		if let [single] = params.content_changes.as_mut_slice()
			&& single.range.is_none()
			&& single.range_length.is_none()
		{
			_ = self
				.on_change(backend::TextDocumentItem {
					uri: params.text_document.uri,
					text: Text::Full(core::mem::take(&mut single.text)),
					version: params.text_document.version,
					language: None,
					old_rope: None,
					open: false,
				})
				.await
				.inspect_err(|err| warn!("{err}"));
			return;
		}

		let old_rope;
		let path = params.text_document.uri.path().as_str();
		{
			await_did_open_document!(self, path);
			let mut document = self
				.document_map
				.get_mut(params.text_document.uri.path().as_str())
				.unwrap_or_else(|| panic!("{}", format_loc!("Did not build a document for {}", params.text_document.uri.as_str())));
			old_rope = document.rope.clone();
			// Update the rope
			// TODO: Refactor into method
			// Per the spec (https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_didChange),
			// deltas are applied SEQUENTIALLY so we don't have to do any extra processing.
			for change in &params.content_changes {
				if change.range.is_none() && change.range_length.is_none() {
					document.rope = ropey::Rope::from_str(&change.text);
				} else {
					let range = change.range.unwrap_or_else(|| panic!("{}", format_loc!("LSP change event must have a range")));
					let range: CharRange = rope_conv(range, document.rope.slice(..));
					let rope = &mut document.rope;
					rope.remove(range.erase());
					if !change.text.is_empty() {
						rope.insert(range.start.0, &change.text);
					}
				}
			}
		}
		_ = self
			.on_change(backend::TextDocumentItem {
				uri: params.text_document.uri,
				text: Text::Delta(params.content_changes),
				version: params.text_document.version,
				language: None,
				old_rope: Some(old_rope),
				open: false,
			})
			.await
			.inspect_err(|err| warn!("{err}"));
	}
	#[instrument(skip_all, ret, fields(uri = params.text_document.uri.as_str()))]
	async fn did_save(&self, params: DidSaveTextDocumentParams) {
		self.root_setup.wait(loc!()).await;
		_ = self.did_save_impl(params).await.inspect_err(|err| warn!("{err}"));
	}
	#[instrument(skip_all, ret, fields(uri = params.text_document_position_params.text_document.uri.as_str()))]
	async fn goto_definition(&self, params: GotoDefinitionParams) -> Result<Option<GotoDefinitionResponse>> {
		self.root_setup.wait(loc!()).await;

		let uri = &params.text_document_position_params.text_document.uri;
		let path = uri.path().as_str();
		let Some((_, ext)) = uri.path().as_str().rsplit_once('.') else {
			debug!("(goto_definition) unsupported: {}", uri.path().as_str());
			return Ok(None);
		};
		await_did_open_document!(self, path);

		let Some(document) = self.document_map.try_get(path).expect(format_loc!("deadlock")) else {
			panic!("Bug: did not build a document for {}", uri.path().as_str());
		};
		if document.setup.should_wait() {
			return Ok(None);
		}
		let rope = document.rope.slice(..);
		let location = match ext {
			"xml" => self.xml_jump_def(params, rope),
			"py" => self.python_jump_def(params, rope),
			"js" => self.js_jump_def(params, rope),
			"csv" => {
				return self.csv_goto_definition(params, rope)
					.map_err(|err| { error!("Error retrieving definition:\n{err}"); tower_lsp_server::jsonrpc::Error::internal_error() });
			}
			_ => {
				debug!("(goto_definition) unsupported: {}", uri.path().as_str());
				return Ok(None);
			}
		};

		let location = location
			.map_err(|err| error!("Error retrieving definition:\n{err}"))
			.ok()
			.flatten();
		Ok(location.map(GotoDefinitionResponse::Scalar))
	}
	#[instrument(skip_all, ret, fields(path))]
	async fn references(&self, params: ReferenceParams) -> Result<Option<Vec<Location>>> {
		self.root_setup.wait(loc!()).await;

		let uri = &params.text_document_position.text_document.uri;
		let path = uri.path().as_str();
		let Some((_, ext)) = uri.path().as_str().rsplit_once('.') else {
			return Ok(None); // hit a directory, super unlikely
		};
		let module_key = some!(self.index.find_module_of(&some!(uri.to_file_path())));
		self.index.load_modules_dependent_on(module_key).await;

		await_did_open_document!(self, path);

		let Some(document) = self.document_map.get(path) else {
			debug!("Bug: did not build a document for {}", uri.path().as_str());
			return Ok(None);
		};

		let rope = document.rope.slice(..);
		let refs = match ext {
			"py" => self.python_references(params, rope),
			"xml" => self.xml_references(params, rope),
			"js" => self.js_references(params, rope),
			_ => return Ok(None),
		};

		Ok(refs.inspect_err(|err| warn!("{err}")).ok().flatten())
	}
	#[instrument(skip_all, ret, fields(path))]
	async fn prepare_rename(
		&self,
		params: TextDocumentPositionParams,
	) -> Result<Option<PrepareRenameResponse>> {
		self.root_setup.wait(loc!()).await;

		let uri = &params.text_document.uri;
		let path = uri.path().as_str();
		let Some((_, ext)) = path.rsplit_once('.') else {
			return Ok(None);
		};

		await_did_open_document!(self, path);

		let Some(document) = self.document_map.get(path) else {
			debug!("Bug: did not build a document for {}", path);
			return Ok(None);
		};

		let rope = document.rope.slice(..);
		let result = match ext {
			"py" => self.python_prepare_rename(params.clone(), rope),
			"xml" => self.xml_prepare_rename(params.clone(), rope),
			"js" => self.js_prepare_rename(params.clone(), rope),
			_ => return Ok(None),
		};

		match result {
			Ok(Some((symbol, range))) => {
				let placeholder = match &symbol {
					backend::RenameableSymbol::XmlId { qualified_id, .. } => qualified_id.clone(),
					backend::RenameableSymbol::ModelName(name) => crate::index::_R(*name).to_string(),
					backend::RenameableSymbol::TemplateName(name) => name.clone(),
				};
				Ok(Some(PrepareRenameResponse::RangeWithPlaceholder { range, placeholder }))
			}
			Ok(None) => Ok(None),
			Err(err) => {
				warn!("prepare_rename error: {err}");
				Ok(None)
			}
		}
	}
	#[instrument(skip_all, ret, fields(path, new_name = params.new_name.as_str()))]
	async fn rename(&self, params: RenameParams) -> Result<Option<WorkspaceEdit>> {
		self.root_setup.wait(loc!()).await;

		let uri = &params.text_document_position.text_document.uri;
		let path = uri.path().as_str();
		let Some((_, ext)) = path.rsplit_once('.') else {
			return Ok(None);
		};

		let file_path = some!(uri.to_file_path());
		let module_key = some!(self.index.find_module_of(&file_path));
		self.index.load_modules_dependent_on(module_key).await;

		await_did_open_document!(self, path);

		let Some(document) = self.document_map.get(path) else {
			debug!("Bug: did not build a document for {}", path);
			return Ok(None);
		};

		let rope = document.rope.slice(..);

		// First, prepare the rename to get the symbol
		let position_params = params.text_document_position.clone();
		let prepare_result = match ext {
			"py" => self.python_prepare_rename(position_params.clone(), rope),
			"xml" => self.xml_prepare_rename(position_params.clone(), rope),
			"js" => self.js_prepare_rename(position_params.clone(), rope),
			_ => return Ok(None),
		};

		let (symbol, _range) = match prepare_result {
			Ok(Some(result)) => result,
			Ok(None) => return Ok(None),
			Err(err) => {
				warn!("rename: prepare failed: {err}");
				return Ok(None);
			}
		};

		// Validate the new name
		if let Err(err) = symbol.validate_new_name(&params.new_name) {
			return Err(tower_lsp_server::jsonrpc::Error::invalid_params(err));
		}

		// Check for conflicts with existing symbols
		if let Some(err) = self.check_rename_conflicts(&symbol, &params.new_name) {
			return Err(tower_lsp_server::jsonrpc::Error::invalid_params(err));
		}

		// Build the workspace edit
		let edit = self.build_rename_edit(&symbol, &params.new_name, &file_path);
		match edit {
			Ok(edit) => Ok(edit),
			Err(err) => {
				warn!("rename: build edit failed: {err}");
				Ok(None)
			}
		}
	}
	#[instrument(skip_all, ret, fields(path))]
	async fn prepare_call_hierarchy(
		&self,
		params: CallHierarchyPrepareParams,
	) -> Result<Option<Vec<CallHierarchyItem>>> {
		self.root_setup.wait(loc!()).await;

		let uri = &params.text_document_position_params.text_document.uri;
		let path = uri.path().as_str();
		let Some((_, ext)) = path.rsplit_once('.') else {
			return Ok(None);
		};

		// Only Python files support call hierarchy for now
		if ext != "py" {
			return Ok(None);
		}

		await_did_open_document!(self, path);

		let Some(document) = self.document_map.get(path) else {
			debug!("Bug: did not build a document for {}", path);
			return Ok(None);
		};

		let rope = document.rope.slice(..);
		let result = self.python_prepare_call_hierarchy(params, rope);

		match result {
			Ok(items) => Ok(items),
			Err(err) => {
				warn!("prepare_call_hierarchy error: {err}");
				Ok(None)
			}
		}
	}
	#[instrument(skip_all, ret)]
	async fn incoming_calls(
		&self,
		params: CallHierarchyIncomingCallsParams,
	) -> Result<Option<Vec<CallHierarchyIncomingCall>>> {
		self.root_setup.wait(loc!()).await;

		// Extract the callable ID from the item's data
		let Some(data) = params.item.data.as_ref() else {
			return Ok(None);
		};

		let call_data: backend::CallHierarchyData = match serde_json::from_value(data.clone()) {
			Ok(data) => data,
			Err(err) => {
				warn!("Failed to deserialize call hierarchy data: {err}");
				return Ok(None);
			}
		};

		// Ensure calls are indexed for relevant modules
		// For methods, we need to index dependent modules and trigger lazy evaluation
		if let crate::call_graph::CallableId::Method { model, method } = &call_data.callable {
			if let Some(model_key) = crate::index::_G(model) {
				if let Some(entry) = self.index.models.get(&model_key) {
					if let Some(base) = &entry.base {
						let file_path = base.0.path.to_path();
						if let Some(module_key) = self.index.find_module_of(&file_path) {
							self.index.load_modules_dependent_on(module_key).await;
						}
					}
				}
			}
			
			// Trigger lazy evaluation of ALL methods with this name across all models
			// This is necessary because any model could be calling this method
			self.ensure_all_methods_evaluated(method);
		} else if let crate::call_graph::CallableId::Function { path, name } = &call_data.callable {
			// Trigger lazy evaluation for the function
			self.ensure_function_evaluated(path, name);
		}

		// Get incoming calls from the call graph
		let call_sites = self.index.call_graph.get_incoming(&call_data.callable);

		if call_sites.is_empty() {
			return Ok(Some(vec![]));
		}

		// Group call sites by caller
		let mut calls_by_caller: std::collections::HashMap<
			Option<crate::call_graph::CallableId>,
			Vec<Range>,
		> = std::collections::HashMap::new();

		for site in call_sites {
			calls_by_caller
				.entry(site.caller.clone())
				.or_default()
				.push(site.location.range);
		}

		// Build CallHierarchyIncomingCall items
		let mut results = Vec::new();
		for (caller, ranges) in calls_by_caller {
			let Some(caller) = caller else {
				continue; // Skip module-level code without a function context
			};

			if let Some(item) = self.index.build_call_hierarchy_item(&caller) {
				results.push(CallHierarchyIncomingCall {
					from: item,
					from_ranges: ranges,
				});
			}
		}

		Ok(Some(results))
	}
	#[instrument(skip_all, ret)]
	async fn outgoing_calls(
		&self,
		params: CallHierarchyOutgoingCallsParams,
	) -> Result<Option<Vec<CallHierarchyOutgoingCall>>> {
		self.root_setup.wait(loc!()).await;

		// Extract the callable ID from the item's data
		let Some(data) = params.item.data.as_ref() else {
			return Ok(None);
		};

		let call_data: backend::CallHierarchyData = match serde_json::from_value(data.clone()) {
			Ok(data) => data,
			Err(err) => {
				warn!("Failed to deserialize call hierarchy data: {err}");
				return Ok(None);
			}
		};

		// Trigger lazy evaluation of the callable to ensure calls are collected
		match &call_data.callable {
			crate::call_graph::CallableId::Method { model, method } => {
				self.ensure_method_evaluated(model, method);
			}
			crate::call_graph::CallableId::Function { path, name } => {
				self.ensure_function_evaluated(path, name);
			}
		}

		// Get outgoing calls from the call graph
		let callees = self.index.call_graph.get_outgoing(&call_data.callable);

		if callees.is_empty() {
			return Ok(Some(vec![]));
		}

		// Group by callee
		let mut calls_by_callee: std::collections::HashMap<
			crate::call_graph::CallableId,
			Vec<Range>,
		> = std::collections::HashMap::new();

		for (callee, loc) in callees {
			calls_by_callee
				.entry(callee)
				.or_default()
				.push(loc.range);
		}

		// Build CallHierarchyOutgoingCall items
		let mut results = Vec::new();
		for (callee, ranges) in calls_by_callee {
			if let Some(item) = self.index.build_call_hierarchy_item(&callee) {
				results.push(CallHierarchyOutgoingCall {
					to: item,
					from_ranges: ranges,
				});
			}
		}

		Ok(Some(results))
	}
	#[instrument(skip_all, fields(uri))]
	async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
		self.root_setup.wait(loc!()).await;

		let uri = &params.text_document_position.text_document.uri;

		let Some((_, ext)) = uri.path().as_str().rsplit_once('.') else {
			return Ok(None); // hit a directory, super unlikely
		};

		let path = uri.path().as_str();
		await_did_open_document!(self, path);
		let module_key = some!(self.index.find_module_of(&some!(uri.to_file_path())));
		self.index.load_modules_dependent_on(module_key).await;
		let rope = {
			let Some(document) = self.document_map.get(uri.path().as_str()) else {
				debug!("Bug: did not build a document for {}", uri.path().as_str());
				return Ok(None);
			};
			document.rope.clone()
		};
		if ext == "xml" {
			let pos = params.text_document_position.position;
			let completions = self.xml_completions(&params, rope.slice(..));
			match completions {
				Ok(inner @ Some(_)) => Ok(inner),
				Ok(None) => {
					if self.xml_is_cursor_in_text(uri, pos, rope.slice(..)).unwrap_or(false) {
						Ok(Some(add_xml_snippets(None)))
					} else {
						Ok(None)
					}
				}
				Err(report) => {
					self.client
						.show_message(MessageType::ERROR, format!("error during xml completion:\n{report}"))
						.await;
					Ok(None)
				}
			}
		} else if ext == "py" {
			let ast = {
				let file_path = uri.to_file_path().unwrap();
				let Some(ast) = self.ast_map.get(file_path.to_str().unwrap()) else {
					debug!("Bug: did not build AST for {}", file_path.display());
					return Ok(None);
				};
				ast.value().clone()
			};
			let completions = self.python_completions(params, ast, rope.slice(..)).await;
			match completions {
				Ok(ret) => Ok(ret),
				Err(err) => {
					self.client
						.show_message(MessageType::ERROR, format!("error during python completion:\n{err}"))
						.await;
					Ok(None)
				}
			}
		} else if ext == "js" {
			let ast = {
				let file_path = uri.to_file_path().unwrap();
				let Some(ast) = self.ast_map.get(file_path.to_str().unwrap()) else {
					debug!("Bug: did not build AST for {}", file_path.display());
					return Ok(None);
				};
				ast.value().clone()
			};
			let completions = self.js_completions(params, ast, rope.slice(..)).await;
			match completions {
				Ok(ret) => Ok(ret),
				Err(err) => {
					self.client
						.show_message(MessageType::ERROR, format!("error during js completion:\n{err}"))
						.await;
					Ok(None)
				}
			}
		} else if ext == "csv" {
			let completions = self.csv_completions(params, rope.slice(..)).await;
			match completions {
				Ok(ret) => Ok(ret),
				Err(err) => {
					self.client
						.show_message(MessageType::ERROR, format!("error during csv completion:\n{err}"))
						.await;
					Ok(None)
				}
			}
		} else {
			debug!("(completion) unsupported {}", uri.path().as_str());
			Ok(None)
		}
	}
	#[instrument(skip_all)]
	async fn completion_resolve(&self, mut completion: CompletionItem) -> Result<CompletionItem> {
		if self.root_setup.should_wait() {
			return Ok(completion);
		}
		'resolve: {
			match &completion.kind {
				Some(CompletionItemKind::CLASS) => {
					let Some(model) = _G(&completion.label) else {
						break 'resolve;
					};
					let Some(mut entry) = self.index.models.get_mut(&model) else {
						break 'resolve;
					};
					if let Err(err) = entry.resolve_details() {
						error!("resolving details: {err}");
					}
					completion.documentation = Some(Documentation::MarkupContent(MarkupContent {
						kind: MarkupKind::Markdown,
						value: self.index.model_docstring(&entry, None, None),
					}))
				}
				Some(CompletionItemKind::FIELD) => {
					_ = self.index.completion_resolve_field(&mut completion);
				}
				Some(CompletionItemKind::METHOD) => {
					_ = self.index.completion_resolve_method(&mut completion);
				}
				_ => {}
			}
		}
		Ok(completion)
	}
	#[instrument(skip_all, fields(uri = params.text_document_position_params.text_document.uri.as_str()))]
	async fn signature_help(&self, params: SignatureHelpParams) -> Result<Option<SignatureHelp>> {
		match self.python_signature_help(params) {
			Ok(ret) => Ok(ret),
			Err(err) => {
				error!("{err}");
				Ok(None)
			}
		}
	}
	#[instrument(level = "trace", skip_all, ret, fields(uri = params.text_document_position_params.text_document.uri.as_str()))]
	async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
		self.root_setup.wait(loc!()).await;

		let uri = &params.text_document_position_params.text_document.uri;
		let path = uri.path().as_str();
		await_did_open_document!(self, path);

		let document = some!(self.document_map.get(uri.path().as_str()));
		let (_, ext) = some!(uri.path().as_str().rsplit_once('.'));
		let rope = document.rope.slice(..);
		let hover = match ext {
			"py" => self.python_hover(params, rope),
			"xml" => self.xml_hover(params, rope),
			"js" => self.js_hover(params, rope),
			"csv" => self.csv_hover(params, rope),
			_ => {
				debug!("(hover) unsupported {}", uri.path().as_str());
				Ok(None)
			}
		};
		match hover {
			Ok(ret) => Ok(ret),
			Err(err) => {
				error!("{err}");
				Ok(None)
			}
		}
	}
	#[instrument(skip_all)]
	async fn did_change_configuration(&self, _: DidChangeConfigurationParams) {
		self.root_setup.wait(loc!()).await;
		let items = self
			.workspaces
			.iter()
			.map(|entry| {
				let scope_uri = Uri::from_file_path(entry.key());
				ConfigurationItem {
					section: Some("odoo-lsp".into()),
					scope_uri,
				}
			})
			.collect();
		let configs = self.client.configuration(items).await.unwrap_or_default();
		let workspace_paths = self.workspaces.iter().map(|ws| ws.key().to_owned()).collect::<Vec<_>>();
		for (config, ws) in configs.into_iter().zip(workspace_paths) {
			match serde_json::from_value(config) {
				Ok(config) => self.on_change_config(config, Some(&ws)),
				Err(err) => warn!("Ignoring config update for {}:\n  {err}", ws.display()),
			}
		}
		self.ensure_nonoverlapping_roots();

		let workspaces = self
			.workspaces
			.iter()
			.map(|ws| ws.key().to_owned())
			.collect::<HashSet<_>>();
		let roots = self
			.index
			.roots
			.iter()
			.map(|root| root.key().to_owned())
			.collect::<HashSet<_>>();

		for removed in roots.difference(&workspaces) {
			self.index.remove_root(removed);
		}

		self.index.delete_marked_entries();

		for added in workspaces.difference(&roots) {
			if let Err(err) = self.index.add_root(added, None).await {
				error!("failed to add root {}:\n  {err}", added.display());
			}
		}
	}
	#[instrument(skip(self))]
	async fn did_change_workspace_folders(&self, params: DidChangeWorkspaceFoldersParams) {
		self.root_setup.wait(loc!()).await;
		for added in params.event.added {
			let Some(path) = added.uri.to_file_path() else { continue };
			self.index.add_root_for_file(&path).await;
		}
		for removed in params.event.removed {
			warn!("unimplemented removing workspace folder {}", removed.name);
		}
		// self.index.delete_marked_entries();
	}
	/// For VSCode and capable LSP clients, these events represent changes mostly to configuration files.
	#[instrument(skip(self))]
	async fn did_change_watched_files(&self, params: DidChangeWatchedFilesParams) {
		for FileEvent { uri, .. } in params.changes {
			let Some(file_path) = uri.to_file_path() else { continue };
			let Some(".odoo_lsp") = file_path.file_stem().and_then(|ostr| ostr.to_str()) else {
				continue;
			};
			if let Some(wspath) = self.workspaces.find_workspace_of(&file_path, |wspath, _| {
				if let Ok(suffix) = file_path.strip_prefix(wspath)
					&& suffix.file_stem().unwrap_or(suffix.as_os_str()).to_string_lossy() == ".odoo_lsp"
				{
					Some(wspath.to_owned())
				} else {
					None
				}
			}) {
				let Ok(file) = std::fs::read(&file_path) else {
					break;
				};
				let mut diagnostics = vec![];
				match serde_json::from_slice(&file) {
					Ok(config) => self.on_change_config(config, Some(&wspath)),
					Err(err) => {
						let point = Position {
							line: err.line() as u32 - 1,
							character: err.column() as u32 - 1,
						};
						diagnostics.push(Diagnostic {
							range: Range {
								start: point,
								end: point,
							},
							message: format!("{err}"),
							severity: Some(DiagnosticSeverity::ERROR),
							..Default::default()
						});
					}
				}
				if !diagnostics.is_empty() {
					let client = self.client.clone();
					tokio::spawn(async move { client.publish_diagnostics(uri, diagnostics, None).await });
				}
				break;
			}
		}
	}
	#[instrument(skip_all, fields(query = params.query))]
	async fn symbol(&self, params: WorkspaceSymbolParams) -> Result<Option<WorkspaceSymbolResponse>> {
		self.root_setup.wait(loc!()).await;

		let query = &params.query;
		let limit = self.project_config.symbols_limit.load(Relaxed);

		let models_by_prefix = some!(self.index.models.by_prefix.read().ok());
		let records_by_prefix = some!(self.index.records.by_prefix.read().ok());
		let models = models_by_prefix.iter_prefix(query.as_bytes()).flat_map(|(_, key)| {
			self.index.models.get(key).into_iter().flat_map(|entry| {
				#[allow(deprecated)]
				entry.base.as_ref().map(|loc| SymbolInformation {
					name: _R(*entry.key()).to_string(),
					kind: SymbolKind::CONSTANT,
					tags: None,
					deprecated: None,
					location: loc.0.clone().into(),
					container_name: None,
				})
			})
		});
		fn to_symbol_information(record: &crate::record::Record) -> SymbolInformation {
			SymbolInformation {
				name: record.qualified_id(),
				kind: SymbolKind::VARIABLE,
				tags: None,
				#[allow(deprecated)]
				deprecated: None,
				location: record.location.clone().into(),
				container_name: None,
			}
		}
		if let Some((module, xml_id_query)) = query.split_once('.') {
			let module = some!(_G(module)).into();
			let records = records_by_prefix
				.iter_prefix(xml_id_query.as_bytes())
				.flat_map(|(_, keys)| {
					keys.iter().flat_map(|key| {
						self.index
							.records
							.get(key)
							.and_then(|record| (record.module == module).then(|| to_symbol_information(&record)))
					})
				});
			Ok(Some(WorkspaceSymbolResponse::Flat(
				models.chain(records).take(limit).collect(),
			)))
		} else {
			let records = records_by_prefix.iter_prefix(query.as_bytes()).flat_map(|(_, keys)| {
				keys.iter()
					.flat_map(|key| self.index.records.get(key).map(|record| to_symbol_information(&record)))
			});
			Ok(Some(WorkspaceSymbolResponse::Flat(
				models.chain(records).take(limit).collect(),
			)))
		}
	}
	#[instrument(skip_all, fields(path))]
	async fn diagnostic(&self, params: DocumentDiagnosticParams) -> Result<DocumentDiagnosticReportResult> {
		self.root_setup.wait(loc!()).await;

		let path = params.text_document.uri.path().as_str();
		await_did_open_document!(self, path);

		let mut diagnostics = vec![];
		let split_path = path.rsplit_once('.');
		if let Some((_, "py")) = split_path
			&& let Some(mut document) = self.document_map.get_mut(path)
		{
			let damage_zone = document.damage_zone.take();
			let rope = document.rope.clone();
			let file_path = params.text_document.uri.to_file_path().unwrap();
			self.diagnose_python(
				file_path.to_str().unwrap(),
				rope.slice(..),
				damage_zone,
				&mut document.diagnostics_cache,
			);
			diagnostics.clone_from(&document.diagnostics_cache);
		} else if let Some((_, "csv")) = split_path
			&& let Some(document) = self.document_map.get(path)
		{
			let rope = document.rope.clone();
			let contents = std::borrow::Cow::from(rope.slice(..));
			let file_path = params.text_document.uri.to_file_path();
			let current_module = file_path.as_ref().and_then(|p| self.index.find_module_of(p));
			diagnostics = self.csv_diagnostics(&contents, current_module);
		} else if let Some((_, "xml")) = split_path
			&& let Some(document) = self.document_map.get(path)
		{
			let rope = document.rope.clone();
			let contents = std::borrow::Cow::from(rope.slice(..));
			let file_path = params.text_document.uri.to_file_path();
			let current_module = file_path.as_ref().and_then(|p| self.index.find_module_of(p));
			diagnostics = self.diagnose_xml(&contents, rope.slice(..), current_module);
		} else if let Some((_, "js")) = split_path
			&& let Some(document) = self.document_map.get(path)
		{
			let rope = document.rope.clone();
			diagnostics = self.js_diagnostics(rope.slice(..));
		}
		Ok(DocumentDiagnosticReportResult::Report(DocumentDiagnosticReport::Full(
			RelatedFullDocumentDiagnosticReport {
				related_documents: None,
				full_document_diagnostic_report: FullDocumentDiagnosticReport {
					result_id: None,
					items: diagnostics,
				},
			},
		)))
	}
	#[instrument(skip_all, ret, fields(uri = params.text_document.uri.as_str()))]
	async fn code_action(&self, params: CodeActionParams) -> Result<Option<CodeActionResponse>> {
		if self.root_setup.should_wait() {
			return Ok(None);
		}
		let Some((_, ext @ ("py" | "xml"))) = params.text_document.uri.path().as_str().rsplit_once('.') else {
			return Ok(None);
		};

		let document = some!(self.document_map.get(params.text_document.uri.path().as_str()));
		if document.setup.should_wait() {
			return Ok(None);
		}

		match ext {
			"xml" => Ok(self
				.xml_code_actions(params, document.rope.slice(..))
				.inspect_err(|err| {
					error!("(code_action xml) {err}");
				})
				.unwrap_or(None)),
			"py" => Ok(self
				.python_code_action(params, document.rope.slice(..))
				.inspect_err(|err| {
					error!("(code_action python) {err}");
				})
				.unwrap_or(None)),
			_ => unreachable!(),
		}
	}
	#[instrument(skip_all, ret)]
	async fn execute_command(&self, params: ExecuteCommandParams) -> Result<Option<Value>> {
		if self.root_setup.should_wait() {
			return Ok(None);
		}
		if let "goto_owl" = params.command.as_str()
			&& let [Value::String(_), Value::String(subcomponent)] = params.arguments.as_slice()
		{
			// FIXME: Subcomponents should not just depend on the component's name,
			// since users can readjust subcomponents' names at will.
			let component = some!(_G(subcomponent));
			let location = {
				let component = some!(self.index.components.get(&component));
				some!(component.location.clone())
			};
			_ = self
				.client
				.show_document(ShowDocumentParams {
					uri: Uri::from_file_path(location.path.to_path()).unwrap(),
					external: Some(false),
					take_focus: Some(true),
					selection: Some(location.range),
				})
				.await;
		} else if let "jump_view" = params.command.as_str() {
			let (model, module) = match &params.arguments[..] {
				[Value::String(model)] => (model.as_str(), None),
				[Value::String(model), Value::String(module)] => (model.as_str(), Some(module.as_str())),
				_ => return Ok(None),
			};
			let model = some!(_G(model)).into();
			let location = {
				let mut views = self.index.records.views_by_model(&model);
				let view = match module {
					Some(module) => views.find(|record| _R(record.module) == module),
					None => views.find(|record| record.inherit_id.is_none()),
				};
				some!(view.as_deref()).location.clone()
			};
			_ = self
				.client
				.show_document(ShowDocumentParams {
					uri: Uri::from_file_path(location.path.to_path()).unwrap(),
					external: Some(false),
					take_focus: Some(true),
					selection: Some(location.range),
				})
				.await;
		}

		Ok(None)
	}
	#[instrument(skip_all, ret, fields(uri = params.text_document.uri.as_str()))]
	async fn document_symbol(
		&self,
		params: DocumentSymbolParams,
	) -> Result<Option<DocumentSymbolResponse>> {
		self.root_setup.wait(loc!()).await;

		let uri = &params.text_document.uri;
		let path = uri.path().as_str();
		await_did_open_document!(self, path);

		let Some((_, ext)) = path.rsplit_once('.') else {
			return Ok(None);
		};

		let document = some!(self.document_map.get(path));
		let rope = document.rope.slice(..);

		let symbols = match ext {
			"py" => self.python_document_symbols(uri, rope),
			"xml" => self.xml_document_symbols(uri, rope),
			"js" => self.js_document_symbols(uri, rope),
			_ => {
				debug!("(document_symbol) unsupported {}", path);
				return Ok(None);
			}
		};

		match symbols {
			Ok(Some(syms)) => Ok(Some(DocumentSymbolResponse::Nested(syms))),
			Ok(None) => Ok(None),
			Err(err) => {
				error!("{err}");
				Ok(None)
			}
		}
	}
	#[instrument(skip_all, ret, fields(uri = params.text_document.uri.as_str()))]
	async fn inlay_hint(&self, params: InlayHintParams) -> Result<Option<Vec<InlayHint>>> {
		self.root_setup.wait(loc!()).await;

		let uri = &params.text_document.uri;
		let path = uri.path().as_str();
		await_did_open_document!(self, path);

		let Some((_, ext)) = path.rsplit_once('.') else {
			return Ok(None);
		};

		let document = some!(self.document_map.get(path));
		let rope = document.rope.slice(..);

		let hints = match ext {
			"py" => self.python_inlay_hints(uri, rope, params.range),
			"xml" => self.xml_inlay_hints(uri, rope, params.range),
			_ => {
				debug!("(inlay_hint) unsupported {}", path);
				return Ok(None);
			}
		};

		match hints {
			Ok(h) => Ok(h),
			Err(err) => {
				error!("{err}");
				Ok(None)
			}
		}
	}
	#[instrument(skip_all, ret, fields(uri = params.text_document.uri.as_str()))]
	async fn semantic_tokens_full(
		&self,
		params: SemanticTokensParams,
	) -> Result<Option<SemanticTokensResult>> {
		self.root_setup.wait(loc!()).await;

		let uri = &params.text_document.uri;
		let path = uri.path().as_str();
		await_did_open_document!(self, path);

		let Some((_, ext)) = path.rsplit_once('.') else {
			return Ok(None);
		};

		let document = some!(self.document_map.get(path));
		let rope = document.rope.slice(..);

		let tokens = match ext {
			"py" => self.python_semantic_tokens(uri, rope, None),
			_ => {
				debug!("(semantic_tokens_full) unsupported {}", path);
				return Ok(None);
			}
		};

		match tokens {
			Ok(Some(data)) => Ok(Some(SemanticTokensResult::Tokens(SemanticTokens {
				result_id: None,
				data,
			}))),
			Ok(None) => Ok(None),
			Err(err) => {
				error!("{err}");
				Ok(None)
			}
		}
	}
	#[instrument(skip_all, ret, fields(uri = params.text_document.uri.as_str()))]
	async fn semantic_tokens_range(
		&self,
		params: SemanticTokensRangeParams,
	) -> Result<Option<SemanticTokensRangeResult>> {
		self.root_setup.wait(loc!()).await;

		let uri = &params.text_document.uri;
		let path = uri.path().as_str();
		await_did_open_document!(self, path);

		let Some((_, ext)) = path.rsplit_once('.') else {
			return Ok(None);
		};

		let document = some!(self.document_map.get(path));
		let rope = document.rope.slice(..);

		let tokens = match ext {
			"py" => self.python_semantic_tokens(uri, rope, Some(params.range)),
			_ => {
				debug!("(semantic_tokens_range) unsupported {}", path);
				return Ok(None);
			}
		};

		match tokens {
			Ok(Some(data)) => Ok(Some(SemanticTokensRangeResult::Tokens(SemanticTokens {
				result_id: None,
				data,
			}))),
			Ok(None) => Ok(None),
			Err(err) => {
				error!("{err}");
				Ok(None)
			}
		}
	}
}
