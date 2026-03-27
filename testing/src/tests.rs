use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::OnceLock;
use std::time::Duration;

use async_lsp::LanguageServer;
use async_lsp::lsp_types::*;
use futures::{StreamExt, stream::FuturesUnordered};
use pretty_assertions::{Comparison, StrComparison};
use rstest::*;
use tree_sitter::Parser;
use tree_sitter::Query;
use tree_sitter::QueryCursor;
use tree_sitter::StreamingIterator;
use ts_macros::query;

use crate::server;

use std::sync::Once;

static TRACING_INIT: Once = Once::new();

fn init_tracing() {
	TRACING_INIT.call_once(|| {
		tracing_subscriber::fmt()
			.with_env_filter(tracing_subscriber::EnvFilter::builder().parse_lossy("warn,odoo_lsp=trace"))
			.init();
	});
}

#[rstest]
#[tokio::test(flavor = "multi_thread")]
#[timeout(Duration::from_secs(10))]
async fn fixture_test(#[files("fixtures/*")] root: PathBuf) -> ExitCode {
	std::env::set_current_dir(&root).unwrap();
	let mut server = server::setup_lsp_server(Some(2));
	init_tracing();

	_ = server
		.initialize(InitializeParams {
			workspace_folders: Some(vec![WorkspaceFolder {
				uri: Url::from_file_path(&root).unwrap(),
				name: root
					.file_name()
					.map(|ostr| ostr.to_string_lossy().into_owned())
					.unwrap_or("odoo-lsp".to_string()),
			}]),
			..Default::default()
		})
		.await
		.expect("initialization failed");

	_ = server.notify::<notification::Initialized>(InitializedParams {});

	// <!> collect expected samples
	let mut expected = gather_expected(&root, TestLanguages::Python);
	expected.extend(gather_expected(&root, TestLanguages::Xml));
	expected.extend(gather_expected(&root, TestLanguages::JavaScript));
	expected.retain(|_, expected| {
		!expected.complete.is_empty()
			|| !expected.diag.is_empty()
			|| !expected.r#type.is_empty()
			|| !expected.def.is_empty()
			|| !expected.related.is_empty()
			|| !expected.symbol.is_empty()
			|| !expected.hint.is_empty()
			|| !expected.token.is_empty()
	});

	// <!> compare and run
	let mut expected: FuturesUnordered<_> = expected
		.into_iter()
		.map(|(path, expected)| {

			let mut server = server.clone();
			let text = match std::fs::read_to_string(&path) {
				Ok(t) => t,
				Err(e) => panic!("Failed to read {}: {e}", path.display()),
			};
			async move {
				let mut diffs = vec![];

				let language_id = match path.extension().unwrap().to_string_lossy().as_ref() {
					"py" => "python",
					"xml" => "xml",
					"js" => "javascript",
					unk => panic!("unknown file extension {unk}"),
				}
				.to_string();

				let uri = Url::from_file_path(&path).unwrap();

				_ = server.did_open(DidOpenTextDocumentParams {
					text_document: TextDocumentItem {
						uri: uri.clone(),
						language_id,
						version: 1,
						text,
					},
				});

				let diags = server
					.document_diagnostic(DocumentDiagnosticParams {
						text_document: TextDocumentIdentifier { uri: uri.clone() },
						identifier: None,
						previous_result_id: None,
						work_done_progress_params: Default::default(),
						partial_result_params: Default::default(),
					})
					.await;
				if let Ok(DocumentDiagnosticReportResult::Report(DocumentDiagnosticReport::Full(report))) = diags {
					let actual = report
						.full_document_diagnostic_report
						.items
						.iter()
						.map(|diag| (diag.range.start, diag.message.clone()))
						.collect::<Vec<_>>();
					if expected.diag[..] != actual[..] {
						diffs.push(format!(
							"[diag] {}\n{}",
							path.display(),
							Comparison::new(&expected.diag[..], &actual[..]),
						));
					}

					if !expected.related.is_empty() {
						// Group consecutive ^related assertions by their position
						// They should all point to the same line/character where an assertion exists
						let mut related_groups: Vec<(Position, Vec<String>)> = Vec::new();

						for (pos, msg) in &expected.related {
							if let Some(last_group) = related_groups.last_mut()
								&& last_group.0 == *pos {
									last_group.1.push(msg.clone());
									continue;
								}
							// Start a new group
							related_groups.push((*pos, vec![msg.clone()]));
						}

						for (pos, expected_msgs) in related_groups {
							// The position of ^related assertions points to the line above them
							// First check if there's a diagnostic at this position
							let diag_at_pos = report.full_document_diagnostic_report.items.iter()
								.find(|d| d.range.start.line == pos.line && d.range.start.character == pos.character);

							if let Some(diag) = diag_at_pos {
								let actual_related = diag.related_information.as_ref()
									.map(|r| r.iter().map(|info| info.message.clone()).collect::<Vec<_>>())
									.unwrap_or_default();

								if expected_msgs.len() != actual_related.len() {
									diffs.push(format!(
										"[related] {}:{}:{}\n\nExpected {} related entries but found {}:\n\nExpected:\n{}\n\nActual:\n{}",
										path.display(),
										pos.line + 1,
										pos.character + 1,
										expected_msgs.len(),
										actual_related.len(),
										expected_msgs.iter().map(|m| format!("  - {}", m)).collect::<Vec<_>>().join("\n"),
										if actual_related.is_empty() {
											"  (no related information)".to_string()
										} else {
											actual_related.iter().map(|m| format!("  - {}", m)).collect::<Vec<_>>().join("\n")
										}
									));
								} else {
									for expected_msg in &expected_msgs {
										if !actual_related.iter().any(|actual| actual.contains(expected_msg)) {
											diffs.push(format!(
												"[related] {}:{}:{}\n\nRelated entry mismatch:\n\nExpected to find:\n  \"{}\"\n\nBut actual related entries were:\n{}",
												path.display(),
												pos.line + 1,
												pos.character + 1,
												expected_msg,
												actual_related.iter().map(|m| format!("  - {}", m)).collect::<Vec<_>>().join("\n")
											));
										}
									}
								}
							} else {
								diffs.push(format!(
									"[related] {}:{}:{}\n\nNo diagnostic found at this position to attach related information to",
									path.display(),
									pos.line + 1,
									pos.character + 1
								));
							}
						}
					}
				} else {
					diffs.push(format!("[diag] failed to get diag: {diags:?}\n\tat {}", path.display()))
				}

				let completions: FuturesUnordered<_> = expected
					.complete
					.iter()
					.map(|(position, expected)| {
						let mut server = server.clone();
						let uri = uri.clone();
						let path = path.display();
						async move {
							let completions = server
								.completion(CompletionParams {
									text_document_position: TextDocumentPositionParams {
										text_document: TextDocumentIdentifier { uri },
										position: *position,
									},
									work_done_progress_params: Default::default(),
									partial_result_params: Default::default(),
									context: None,
								})
								.await;
							if let Ok(Some(CompletionResponse::List(list))) = completions {
								let mut actual =
									list.items.iter().map(|comp| comp.label.to_string()).collect::<Vec<_>>();
								let mut expected_sorted = expected.clone();
								actual.sort();
								expected_sorted.sort();
								if expected_sorted != actual {
									format!(
										"[complete] in {path}:{}:{}\n{}",
										position.line + 1,
										position.character + 1,
										Comparison::new(&expected[..], &actual[..]),
									)
								} else {
									String::new()
								}
							} else {
								format!(
									"[complete] failed to get completions: {completions:?}\n\tat {path}:{}:{}",
									position.line + 1,
									position.character + 1
								)
							}
						}
					})
					.collect();

				let types: FuturesUnordered<_> = expected
					.r#type
					.iter()
					.map(|(position, expected)| {
						let server = server.clone();
						let uri = uri.clone();
						let path = path.display();
						async move {
							let r#type = server
								.request::<InspectType>(TextDocumentPositionParams {
									text_document: TextDocumentIdentifier { uri: uri.clone() },
									position: *position,
								})
								.await;
							if let Ok(actual) = r#type {
								let actual = actual.as_deref().unwrap_or("None");
								if expected != actual {
									format!(
										"[type] in {path}:{}:{}  {}",
										position.line + 1,
										position.character + 1,
										StrComparison::new(expected, actual),
									)
								} else {
									String::new()
								}
							} else {
								format!(
									"[type] failed to get type: {type:?}\n\tat {path}:{}:{}",
									position.line + 1,
									position.character + 1
								)
							}
						}
					})
					.collect();

				let definitions: FuturesUnordered<_> = expected
					.def
					.iter()
					.map(|position| {
						let mut server = server.clone();
						let uri = uri.clone();
						let path = path.display();
						async move {
							let definition = server
								.definition(GotoDefinitionParams {
									text_document_position_params: TextDocumentPositionParams {
										text_document: TextDocumentIdentifier { uri },
										position: *position,
									},
									work_done_progress_params: Default::default(),
									partial_result_params: Default::default(),
								})
								.await;
							match definition {
								Ok(Some(GotoDefinitionResponse::Scalar(location))) => {
									// Check if the definition points to a valid file
									if location.uri.to_file_path().is_ok() {
										String::new()
									} else {
										format!(
											"[def] invalid file path in {path}:{}:{}",
											position.line + 1,
											position.character + 1
										)
									}
								}
								Ok(Some(GotoDefinitionResponse::Array(locations))) => {
									if !locations.is_empty() && locations[0].uri.to_file_path().is_ok() {
										String::new()
									} else {
										format!(
											"[def] no valid definitions in {path}:{}:{}",
											position.line + 1,
											position.character + 1
										)
									}
								}
								Ok(Some(GotoDefinitionResponse::Link(_))) => {
									// For now, just accept any link response as valid
									String::new()
								}
								Ok(None) => {
									format!(
										"[def] no definition found in {path}:{}:{}",
										position.line + 1,
										position.character + 1
									)
								}
								Err(e) => {
									format!(
										"[def] failed to get definition: {e:?}\\n\\tat {path}:{}:{}",
										position.line + 1,
										position.character + 1
									)
								}
							}
						}
					})
					.collect();

				// Document symbols test
				if !expected.symbol.is_empty() {
					let symbols_result = server
						.document_symbol(DocumentSymbolParams {
							text_document: TextDocumentIdentifier { uri: uri.clone() },
							work_done_progress_params: Default::default(),
							partial_result_params: Default::default(),
						})
						.await;

					match symbols_result {
						Ok(Some(DocumentSymbolResponse::Nested(symbols))) => {
							let actual_names = collect_symbol_names(&symbols);
							for expected_name in &expected.symbol {
								if !actual_names.iter().any(|n| n.contains(expected_name)) {
									diffs.push(format!(
										"[symbol] in {}\n\nExpected symbol '{}' not found.\nActual symbols: {:?}",
										path.display(),
										expected_name,
										actual_names
									));
								}
							}
						}
						Ok(Some(DocumentSymbolResponse::Flat(symbols))) => {
							let actual_names: Vec<_> = symbols.iter().map(|s| s.name.clone()).collect();
							for expected_name in &expected.symbol {
								if !actual_names.iter().any(|n| n.contains(expected_name)) {
									diffs.push(format!(
										"[symbol] in {}\n\nExpected symbol '{}' not found.\nActual symbols: {:?}",
										path.display(),
										expected_name,
										actual_names
									));
								}
							}
						}
						Ok(None) => {
							diffs.push(format!(
								"[symbol] in {}\n\nNo symbols returned, expected: {:?}",
								path.display(),
								expected.symbol
							));
						}
						Err(e) => {
							diffs.push(format!(
								"[symbol] in {}\n\nFailed to get symbols: {:?}",
								path.display(),
								e
							));
						}
					}
				}

				// Inlay hints test
				let hints: FuturesUnordered<_> = expected
					.hint
					.iter()
					.map(|(position, expected_hint)| {
						let mut server = server.clone();
						let uri = uri.clone();
						let path = path.display();
						let position = *position;
						let expected_hint = expected_hint.clone();
						async move {
							// Request hints for a small range around the position
							let range = Range {
								start: Position {
									line: position.line.saturating_sub(1),
									character: 0,
								},
								end: Position {
									line: position.line + 2,
									character: 0,
								},
							};
							let hints_result = server
								.inlay_hint(InlayHintParams {
									text_document: TextDocumentIdentifier { uri },
									range,
									work_done_progress_params: Default::default(),
								})
								.await;

							match hints_result {
								Ok(Some(hints)) => {
									// Find a hint at or near the expected position
									let matching_hint = hints.iter().find(|hint| {
										hint.position.line == position.line
											&& hint.position.character >= position.character
									});

									if let Some(hint) = matching_hint {
										let label = match &hint.label {
											InlayHintLabel::String(s) => s.clone(),
											InlayHintLabel::LabelParts(parts) => {
												parts.iter().map(|p| p.value.as_str()).collect()
											}
										};
										// Check if the hint contains the expected text
										if !label.contains(&expected_hint) {
											format!(
												"[hint] in {path}:{}:{}\n\nExpected hint containing '{}', got '{}'",
												position.line + 1,
												position.character + 1,
												expected_hint,
												label
											)
										} else {
											String::new()
										}
									} else {
										format!(
											"[hint] in {path}:{}:{}\n\nNo hint found at position, expected '{}'.\nAvailable hints: {:?}",
											position.line + 1,
											position.character + 1,
											expected_hint,
											hints.iter().map(|h| format!("{}:{} - {:?}", h.position.line, h.position.character, h.label)).collect::<Vec<_>>()
										)
									}
								}
								Ok(None) => {
									format!(
										"[hint] in {path}:{}:{}\n\nNo hints returned, expected '{}'",
										position.line + 1,
										position.character + 1,
										expected_hint
									)
								}
								Err(e) => {
									format!(
										"[hint] in {path}:{}:{}\n\nFailed to get hints: {:?}",
										position.line + 1,
										position.character + 1,
										e
									)
								}
							}
						}
					})
					.collect();

				// Semantic tokens test
				if !expected.token.is_empty() {
					let _rope = odoo_lsp::prelude::Rope::from_str(&std::fs::read_to_string(&path).unwrap_or_default());
					let tokens_result = server
						.semantic_tokens_full(SemanticTokensParams {
							text_document: TextDocumentIdentifier { uri: uri.clone() },
							work_done_progress_params: Default::default(),
							partial_result_params: Default::default(),
						})
						.await;

					match tokens_result {
						Ok(Some(SemanticTokensResult::Tokens(tokens))) => {
							// Decode delta-encoded tokens to absolute positions
							let decoded = decode_semantic_tokens(&tokens.data);

							for (position, expected_type) in &expected.token {
								let matching_token = decoded.iter().find(|t| {
									t.line == position.line
										&& t.start_char <= position.character
										&& position.character < t.start_char + t.length
								});

								if let Some(token) = matching_token {
									let actual_type = token_type_name(token.token_type);
									if actual_type != expected_type {
										diffs.push(format!(
											"[token] in {}:{}:{}\n\nExpected token type '{}', got '{}'",
											path.display(),
											position.line + 1,
											position.character + 1,
											expected_type,
											actual_type
										));
									}
								} else {
									diffs.push(format!(
										"[token] in {}:{}:{}\n\nNo token found at position, expected '{}'.\nAvailable tokens: {:?}",
										path.display(),
										position.line + 1,
										position.character + 1,
										expected_type,
										decoded.iter().map(|t| format!("{}:{}-{} {}", t.line, t.start_char, t.start_char + t.length, token_type_name(t.token_type))).collect::<Vec<_>>()
									));
								}
							}
						}
						Ok(Some(SemanticTokensResult::Partial(_))) => {
							diffs.push(format!(
								"[token] in {}\n\nGot partial result, expected full tokens",
								path.display()
							));
						}
						Ok(None) => {
							diffs.push(format!(
								"[token] in {}\n\nNo tokens returned, expected: {:?}",
								path.display(),
								expected.token
							));
						}
						Err(e) => {
							diffs.push(format!(
								"[token] in {}\n\nFailed to get tokens: {:?}",
								path.display(),
								e
							));
						}
					}
				}

				let mut items = completions.chain(types).chain(definitions).chain(hints);
				while let Some(diff) = items.next().await {
					diffs.push(diff);
				}

				diffs
			}
		})
		.collect();

	let mut messages = vec![];
	while let Some(diffs) = expected.next().await {
		messages.extend(diffs);
	}

	_ = server.shutdown(()).await;
	_ = server.exit(());

	let message = messages.join("\n");
	let message = message.trim_ascii();
	if !message.is_empty() {
		eprintln!("tests failed:\n{message}");
		ExitCode::FAILURE
	} else {
		ExitCode::SUCCESS
	}
}

query! {
	PyExpected();

	((comment) @diag
	(#match? @diag "\\^diag "))

	((comment) @complete
	(#match? @complete "\\^complete "))

	((comment) @type
	(#match? @type "\\^type "))

	((comment) @def
	(#match? @def "\\^def"))

	((comment) @related
	(#match? @related "\\^related "))

	((comment) @symbol
	(#match? @symbol "\\^symbol "))

	((comment) @hint
	(#match? @hint "\\^hint "))

	((comment) @token
	(#match? @token "\\^token "))
}

fn xml_query() -> &'static Query {
	static QUERY: OnceLock<Query> = OnceLock::new();
	const XML_QUERY: &str = r#"
		((Comment) @diag
		(#match? @diag "\\^diag "))

		((Comment) @complete
		(#match? @complete "\\^complete "))

		((Comment) @hint
		(#match? @hint "\\^hint "))

		((Comment) @symbol
		(#match? @symbol "\\^symbol "))

		((Comment) @type
		(#match? @type "\\^type "))
	"#;
	QUERY.get_or_init(|| Query::new(&tree_sitter_xml::LANGUAGE_XML.into(), XML_QUERY).unwrap())
}

fn js_query() -> &'static Query {
	static QUERY: OnceLock<Query> = OnceLock::new();
	const JS_QUERY: &str = r#"
		((comment) @diag
		(#match? @diag "\\^diag "))

		((comment) @complete
		(#match? @complete "\\^complete "))

		((comment) @def
		(#match? @def "\\^def"))
	"#;
	QUERY.get_or_init(|| Query::new(&tree_sitter_javascript::LANGUAGE.into(), JS_QUERY).unwrap())
}

#[derive(Default)]
struct Expected {
	diag: Vec<(Position, String)>,
	complete: Vec<(Position, Vec<String>)>,
	r#type: Vec<(Position, String)>,
	def: Vec<Position>,
	related: Vec<(Position, String)>,
	symbol: Vec<String>,
	hint: Vec<(Position, String)>,
	token: Vec<(Position, String)>,
}

enum TestLanguages {
	Python,
	Xml,
	JavaScript,
}

impl TestLanguages {
	/// Returns the comment prefix for this language
	fn comment_prefix(&self) -> &'static str {
		match self {
			TestLanguages::Xml => "<!--",
			TestLanguages::JavaScript => "//",
			TestLanguages::Python => "#",
		}
	}

	/// Returns the comment suffix for this language (if any)
	fn comment_suffix(&self) -> Option<&'static str> {
		match self {
			TestLanguages::Xml => Some("-->"),
			_ => None,
		}
	}

	/// Extracts the content from a comment node
	fn extract_comment_content<'a>(&self, node_text: &'a str) -> &'a str {
		let text = node_text.strip_prefix(self.comment_prefix()).unwrap_or(node_text);
		if let Some(suffix) = self.comment_suffix() {
			text.strip_suffix(suffix).unwrap_or(text).trim()
		} else {
			text.trim()
		}
	}
}

enum InspectType {}
impl request::Request for InspectType {
	const METHOD: &'static str = "odoo-lsp/inspect-type";
	type Params = TextDocumentPositionParams;
	type Result = Option<String>;
}

/// Represents a parsed assertion from test files
#[derive(Debug, Clone)]
struct ParsedAssertion {
	line: u32,
	character: u32,
	kind: String,
	value: String,
}

impl ParsedAssertion {
	/// Creates a Position pointing to the line above this assertion
	fn target_position(&self) -> Position {
		Position {
			line: self.line.saturating_sub(1),
			character: self.character,
		}
	}

	/// Checks if this assertion is on the line immediately after another
	fn is_consecutive_to(&self, other: &ParsedAssertion) -> bool {
		self.line == other.line + 1
	}
}

fn gather_expected(root: &Path, lang: TestLanguages) -> HashMap<PathBuf, Expected> {
	let (glob, query, language) = match lang {
		TestLanguages::Python => ("**/*.py", PyExpected::query as fn() -> _, tree_sitter_python::LANGUAGE),
		TestLanguages::Xml => ("**/*.xml", xml_query as _, tree_sitter_xml::LANGUAGE_XML),
		TestLanguages::JavaScript => ("**/*.js", js_query as _, tree_sitter_javascript::LANGUAGE),
	};

	let path = root.join(glob).to_string_lossy().into_owned();
	let mut expected = HashMap::<_, Expected>::new();

	for file in globwalk::glob(&path).unwrap() {
		let Ok(file) = file else { continue };
		let contents = std::fs::read_to_string(file.path()).unwrap();
		let expected = expected.entry(file.into_path()).or_default();
		let mut parser = Parser::new();
		parser.set_language(&language.into()).unwrap();
		let ast = parser.parse(contents.as_bytes(), None).unwrap();
		let mut cursor = QueryCursor::new();

		// First pass: collect all assertions
		let mut assertions = Vec::new();
		let mut captures = cursor.captures(query(), ast.root_node(), contents.as_bytes());
		while let Some((match_, _)) = captures.next() {
			for capture in match_.captures {
				if let Some(assertion) = parse_assertion_from_capture(capture, &contents, &lang) {
					assertions.push(assertion);
				}
			}
		}

		// Sort assertions by line number to process them in order
		assertions.sort_by_key(|a| a.line);

		// Second pass: process assertions and handle consecutive related
		process_assertions(expected, assertions);
	}

	expected
}

/// Parse a single assertion from a tree-sitter capture
fn parse_assertion_from_capture(
	capture: &tree_sitter::QueryCapture,
	contents: &str,
	lang: &TestLanguages,
) -> Option<ParsedAssertion> {
	let node_text = capture.node.utf8_text(contents.as_bytes()).ok()?;

	// Find the caret position in the original node text
	let caret_pos_in_node = node_text.find('^')?;

	// Extract comment content for parsing the assertion
	let comment_content = lang.extract_comment_content(node_text);

	// Parse the assertion - find caret in the comment content
	let caret_pos_in_content = comment_content.find('^')?;
	let assertion_text = &comment_content[caret_pos_in_content..];
	let assertion_content = assertion_text.strip_prefix("^")?.trim();

	let (kind, value) = if let Some((k, v)) = assertion_content.split_once(' ') {
		(k.to_string(), v.to_string())
	} else if assertion_content == "def" {
		("def".to_string(), String::new())
	} else {
		return None; // Skip invalid assertions
	};

	// Calculate position
	let range = capture.node.range();
	let line = range.start_point.row as _;
	// The character position is where the caret appears in the original line
	let character = (range.start_point.column + caret_pos_in_node) as u32;

	Some(ParsedAssertion {
		line,
		character,
		kind,
		value,
	})
}

/// Process parsed assertions and populate the Expected struct
fn process_assertions(expected: &mut Expected, assertions: Vec<ParsedAssertion>) {
	let mut i = 0;
	let mut last_non_related_position: Option<Position> = None;

	while i < assertions.len() {
		let assertion = &assertions[i];
		let position = assertion.target_position();

		match assertion.kind.as_str() {
			"complete" => {
				let completions = assertion.value.split(' ').map(String::from).collect();
				expected.complete.push((position, completions));
				last_non_related_position = Some(position);
				i += 1;
			}
			"diag" => {
				expected.diag.push((position, assertion.value.clone()));
				last_non_related_position = Some(position);
				i += 1;
			}
			"type" => {
				expected.r#type.push((position, assertion.value.clone()));
				last_non_related_position = Some(position);
				i += 1;
			}
			"def" => {
				expected.def.push(position);
				last_non_related_position = Some(position);
				i += 1;
			}
			"symbol" => {
				// Symbol assertions are file-level, listing expected symbol names
				expected.symbol.push(assertion.value.clone());
				i += 1;
			}
			"hint" => {
				// Hint assertions check for inlay hint at specific position
				expected.hint.push((position, assertion.value.clone()));
				last_non_related_position = Some(position);
				i += 1;
			}
			"token" => {
				// Token assertions check for semantic token at specific position
				expected.token.push((position, assertion.value.clone()));
				last_non_related_position = Some(position);
				i += 1;
			}
			"related" => {
				// For consecutive related assertions, they should all point to the same position
				// which is the position of the last non-related assertion
				if let Some(target_pos) = last_non_related_position {
					// Collect all consecutive related assertions
					let mut related_group = vec![assertion.value.clone()];
					let mut j = i + 1;

					while j < assertions.len() && assertions[j].kind == "related" {
						if assertions[j].is_consecutive_to(&assertions[j - 1]) {
							related_group.push(assertions[j].value.clone());
							j += 1;
						} else {
							break;
						}
					}

					// Add all related assertions with the same target position
					for msg in related_group {
						expected.related.push((target_pos, msg));
					}

					i = j;
				} else {
					// Standalone related without a preceding assertion
					eprintln!(
						"Warning: ^related assertion at line {} without preceding assertion",
						assertion.line
					);
					expected.related.push((position, assertion.value.clone()));
					i += 1;
				}
			}
			_ => {
				// Unknown assertion type
				eprintln!(
					"Warning: Unknown assertion type '{}' at line {}",
					assertion.kind, assertion.line
				);
				i += 1;
			}
		}
	}
}

/// Recursively collect all symbol names from a nested DocumentSymbol tree
fn collect_symbol_names(symbols: &[DocumentSymbol]) -> Vec<String> {
	let mut names = Vec::new();
	for symbol in symbols {
		names.push(symbol.name.clone());
		if let Some(children) = &symbol.children {
			names.extend(collect_symbol_names(children));
		}
	}
	names
}

/// Decoded semantic token with absolute positions
#[derive(Debug)]
struct DecodedToken {
	line: u32,
	start_char: u32,
	length: u32,
	token_type: u32,
	#[allow(dead_code)]
	modifiers: u32,
}

/// Decode delta-encoded semantic tokens to absolute positions
fn decode_semantic_tokens(tokens: &[SemanticToken]) -> Vec<DecodedToken> {
	let mut decoded = Vec::with_capacity(tokens.len());
	let mut prev_line = 0u32;
	let mut prev_start = 0u32;

	for token in tokens {
		let line = prev_line + token.delta_line;
		let start_char = if token.delta_line == 0 {
			prev_start + token.delta_start
		} else {
			token.delta_start
		};

		decoded.push(DecodedToken {
			line,
			start_char,
			length: token.length,
			token_type: token.token_type,
			modifiers: token.token_modifiers_bitset,
		});

		prev_line = line;
		prev_start = start_char;
	}

	decoded
}

/// Map token type index to name (must match server.rs legend order)
fn token_type_name(index: u32) -> &'static str {
	match index {
		0 => "CLASS",
		1 => "PROPERTY",
		2 => "METHOD",
		3 => "DECORATOR",
		4 => "TYPE",
		5 => "STRING",
		_ => "UNKNOWN",
	}
}
