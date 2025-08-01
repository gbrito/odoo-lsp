use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::Ordering::Relaxed;

use ropey::Rope;
use serde_json::Value;
use tower_lsp_server::LanguageServer;
use tower_lsp_server::jsonrpc::Result;
use tower_lsp_server::lsp_types::notification::{DidChangeConfiguration, Notification};
use tower_lsp_server::{UriExt, lsp_types::*};
use tracing::{debug, error, info, instrument, warn};

use crate::{GITVER, NAME, VERSION, await_did_open_document};

use crate::backend::{Backend, Document, Language, Text};
use crate::index::{_G, _R};
use crate::{backend, some, utils::*};

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
					commands: vec!["goto_owl".to_string()],
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
				workspace: Some(WorkspaceServerCapabilities {
					workspace_folders: Some(WorkspaceFoldersServerCapabilities {
						supported: Some(true),
						change_notifications: Some(OneOf::Left(true)),
					}),
					file_operations: None,
				}),
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
		self.ast_map.remove(path);

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

		let _blocker = self.root_setup.block();
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
	#[instrument(skip_all, ret, fields(uri=params.text_document.uri.path().as_str()))]
	async fn did_open(&self, params: DidOpenTextDocumentParams) {
		self.root_setup.wait().await;
		// NB: fixes a race condition where completions are requested even before
		// did_open had a chance to put in a blocker for the document, leading to
		// flaky tests and the first completion request yielding nothing (super minor issue)
		let _blocker = self.root_setup.block();

		info!("{}", params.text_document.uri.path().as_str());
		let language_id = params.text_document.language_id.as_str();
		let split_uri = params.text_document.uri.path().as_str().rsplit_once('.');
		let language = match (language_id, split_uri) {
			("python", _) | (_, Some((_, "py"))) => Language::Python,
			("javascript", _) | (_, Some((_, "js"))) => Language::Javascript,
			("xml", _) | (_, Some((_, "xml"))) => Language::Xml,
			_ => {
				debug!(
					"Could not determine language, or language not supported:\nlanguage_id={language_id} split_uri={split_uri:?}"
				);
				return;
			}
		};

		let rope = Rope::from_str(&params.text_document.text);
		self.document_map.insert(
			params.text_document.uri.path().as_str().to_string(),
			Document::new(rope.clone()),
		);

		let path = params.text_document.uri.to_file_path().unwrap();
		if self.index.find_module_of(&path).is_none() {
			// outside of root?
			debug!("oob: {}", params.text_document.uri.path().as_str());
			let path = params.text_document.uri.to_file_path();
			let mut path = path.as_deref();
			while let Some(path_) = path {
				if tokio::fs::try_exists(path_.with_file_name("__manifest__.py"))
					.await
					.unwrap_or(false)
					&& let Some(file_path) = path_.parent().and_then(|p| p.parent())
				{
					_ = self
						.index
						.add_root(file_path, Some(self.client.clone()))
						.await
						.inspect_err(|err| warn!("failed to add root {}:\n{err}", file_path.display()));
					break;
				}
				path = path_.parent();
			}
		}

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
	}
	#[instrument(skip_all, ret, fields(uri = params.text_document.uri.as_str()))]
	async fn did_change(&self, mut params: DidChangeTextDocumentParams) {
		self.root_setup.wait().await;
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
				.expect("Did not build a document");
			old_rope = document.rope.clone();
			// Update the rope
			// TODO: Refactor into method
			// Per the spec (https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_didChange),
			// deltas are applied SEQUENTIALLY so we don't have to do any extra processing.
			for change in &params.content_changes {
				if change.range.is_none() && change.range_length.is_none() {
					document.rope = ropey::Rope::from_str(&change.text);
				} else {
					let range = change.range.expect("LSP change event must have a range");
					let range: CharRange =
						rope_conv(range, document.rope.slice(..)).expect("did_change applying delta: no range");
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
		self.root_setup.wait().await;
		_ = self.did_save_impl(params).await.inspect_err(|err| warn!("{err}"));
	}
	#[instrument(skip_all, ret, fields(uri = params.text_document_position_params.text_document.uri.as_str()))]
	async fn goto_definition(&self, params: GotoDefinitionParams) -> Result<Option<GotoDefinitionResponse>> {
		self.root_setup.wait().await;

		let uri = &params.text_document_position_params.text_document.uri;
		let path = uri.path().as_str();
		let Some((_, ext)) = uri.path().as_str().rsplit_once('.') else {
			debug!("(goto_definition) unsupported: {}", uri.path().as_str());
			return Ok(None);
		};
		await_did_open_document!(self, path);

		let Some(document) = self.document_map.get(path) else {
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
			_ => {
				debug!("(goto_definition) unsupported: {}", uri.path().as_str());
				return Ok(None);
			}
		};

		let location = location
			.map_err(|err| error!("Error retrieving references:\n{err}"))
			.ok()
			.flatten();
		Ok(location.map(GotoDefinitionResponse::Scalar))
	}
	#[instrument(skip_all, ret, fields(path))]
	async fn references(&self, params: ReferenceParams) -> Result<Option<Vec<Location>>> {
		self.root_setup.wait().await;

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
	#[instrument(skip_all, fields(uri))]
	async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
		self.root_setup.wait().await;

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
			let completions = self.xml_completions(params, rope.slice(..));
			match completions {
				Ok(ret) => Ok(ret),
				Err(report) => {
					self.client
						.show_message(MessageType::ERROR, format!("error during xml completion:\n{report}"))
						.await;
					Ok(None)
				}
			}
		} else if ext == "py" {
			let ast = {
				let Some(ast) = self.ast_map.get(uri.path().as_str()) else {
					debug!("Bug: did not build AST for {}", uri.path().as_str());
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
				let Some(ast) = self.ast_map.get(uri.path().as_str()) else {
					debug!("Bug: did not build AST for {}", uri.path().as_str());
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
					let Some(mut entry) = self.index.models.get_mut(&model.into()) else {
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
	#[instrument(skip_all, ret, fields(uri = params.text_document_position_params.text_document.uri.as_str()))]
	async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
		self.root_setup.wait().await;

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
		self.root_setup.wait().await;
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
				Err(err) => error!("Could not parse updated configuration for {}:\n{err}", ws.display()),
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
				error!("failed to add root {}:\n{err}", added.display());
			}
		}
	}
	#[instrument(skip(self))]
	async fn did_change_workspace_folders(&self, params: DidChangeWorkspaceFoldersParams) {
		self.root_setup.wait().await;
		for added in params.event.added {
			let Some(file_path) = added.uri.to_file_path() else {
				error!("not a file path: {}", added.uri.as_str());
				continue;
			};
			_ = self
				.index
				.add_root(&file_path, None)
				.await
				.inspect_err(|err| warn!("failed to add root {}:\n{err}", file_path.display()));
		}
		for removed in params.event.removed {
			let Some(file_path) = removed.uri.to_file_path() else {
				error!("not a file path: {}", removed.uri.as_str());
				continue;
			};
			self.index.remove_root(&file_path);
		}
		self.index.delete_marked_entries();
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
				self.client.publish_diagnostics(uri, diagnostics, None).await;
				break;
			}
		}
	}
	#[instrument(skip_all, fields(query = params.query))]
	async fn symbol(
		&self,
		params: WorkspaceSymbolParams,
	) -> Result<Option<OneOf<Vec<SymbolInformation>, Vec<WorkspaceSymbol>>>> {
		self.root_setup.wait().await;

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
			Ok(Some(OneOf::Left(models.chain(records).take(limit).collect())))
		} else {
			let records = records_by_prefix.iter_prefix(query.as_bytes()).flat_map(|(_, keys)| {
				keys.iter()
					.flat_map(|key| self.index.records.get(key).map(|record| to_symbol_information(&record)))
			});
			Ok(Some(OneOf::Left(models.chain(records).take(limit).collect())))
		}
	}
	#[instrument(skip_all, fields(path))]
	async fn diagnostic(&self, params: DocumentDiagnosticParams) -> Result<DocumentDiagnosticReportResult> {
		self.root_setup.wait().await;

		let path = params.text_document.uri.path().as_str();
		await_did_open_document!(self, path);

		let mut diagnostics = vec![];
		if let Some((_, "py")) = path.rsplit_once('.')
			&& let Some(mut document) = self.document_map.get_mut(path)
		{
			let damage_zone = document.damage_zone.take();
			let rope = document.rope.clone();
			self.diagnose_python(
				params.text_document.uri.path().as_str(),
				rope.slice(..),
				damage_zone,
				&mut document.diagnostics_cache,
			);
			diagnostics.clone_from(&document.diagnostics_cache);
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
		let Some((_, "xml")) = params.text_document.uri.path().as_str().rsplit_once('.') else {
			return Ok(None);
		};

		let document = some!(self.document_map.get(params.text_document.uri.path().as_str()));
		if document.setup.should_wait() {
			return Ok(None);
		}

		Ok(self
			.xml_code_actions(params, document.rope.slice(..))
			.inspect_err(|err| {
				error!("(code_lens) {err}");
			})
			.unwrap_or(None))
	}
	#[instrument(skip_all, ret)]
	async fn execute_command(&self, params: ExecuteCommandParams) -> Result<Option<Value>> {
		if self.root_setup.should_wait() {
			return Ok(None);
		}
		if let ("goto_owl", [Value::String(_), Value::String(subcomponent)]) =
			(params.command.as_str(), params.arguments.as_slice())
		{
			// FIXME: Subcomponents should not just depend on the component's name,
			// since users can readjust subcomponents' names at will.
			let component = some!(_G(subcomponent));
			let location = {
				let component = some!(self.index.components.get(&component.into()));
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
		}

		Ok(None)
	}
}
