//! Call graph tracking for Call Hierarchy LSP support.
//!
//! This module provides data structures and methods for tracking method/function
//! call relationships, enabling "incoming calls" and "outgoing calls" navigation.

use std::collections::HashSet;
use std::sync::RwLock;

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use smart_default::SmartDefault;

use crate::index::PathSymbol;
use crate::utils::MinLoc;

/// Identifies a callable entity (method or function).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CallableId {
	/// Model method identified by model name and method name.
	Method {
		model: String,
		method: String,
	},
	/// Module-level function identified by file path and function name.
	Function {
		path: String,
		name: String,
	},
}

impl CallableId {
	/// Create a new method callable ID.
	pub fn method(model: impl Into<String>, method: impl Into<String>) -> Self {
		Self::Method {
			model: model.into(),
			method: method.into(),
		}
	}

	/// Create a new function callable ID.
	pub fn function(path: impl Into<String>, name: impl Into<String>) -> Self {
		Self::Function {
			path: path.into(),
			name: name.into(),
		}
	}

	/// Get a display name for the callable.
	pub fn display_name(&self) -> String {
		match self {
			Self::Method { model, method } => format!("{}.{}", model, method),
			Self::Function { name, .. } => name.clone(),
		}
	}

	/// Get a detail string for the callable (model name or file path).
	pub fn detail(&self) -> &str {
		match self {
			Self::Method { model, .. } => model,
			Self::Function { path, .. } => path,
		}
	}
}

/// Type of call relationship.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallType {
	/// Regular method/function call (e.g., `self.method()` or `func()`).
	Direct,
	/// Super call in inheritance chain (e.g., `super().method()`).
	Super,
}

/// A call site where a callable is invoked.
#[derive(Debug, Clone)]
pub struct CallSite {
	/// Location of the call expression.
	pub location: MinLoc,
	/// The callable making this call (None for module-level code outside functions).
	pub caller: Option<CallableId>,
	/// Type of call relationship.
	pub call_type: CallType,
}

/// Index of call relationships for fast lookup.
///
/// Supports both incoming calls (who calls X?) and outgoing calls (what does X call?).
#[derive(SmartDefault)]
pub struct CallGraph {
	/// Callee -> list of call sites that invoke it (for incoming calls).
	#[default(_code = "DashMap::with_shard_amount(4)")]
	incoming: DashMap<CallableId, Vec<CallSite>>,

	/// Caller -> list of (callee, call_location) pairs (for outgoing calls).
	#[default(_code = "DashMap::with_shard_amount(4)")]
	outgoing: DashMap<CallableId, Vec<(CallableId, MinLoc)>>,

	/// Track which files have been indexed for call collection.
	#[default(_code = "RwLock::new(HashSet::new())")]
	indexed_files: RwLock<HashSet<PathSymbol>>,
}

impl CallGraph {
	/// Add a call relationship to the graph.
	///
	/// # Arguments
	/// * `caller` - The callable making the call (None for module-level code)
	/// * `callee` - The callable being invoked
	/// * `location` - Location of the call expression
	/// * `call_type` - Type of call (Direct or Super)
	pub fn add_call(
		&self,
		caller: Option<CallableId>,
		callee: CallableId,
		location: MinLoc,
		call_type: CallType,
	) {
		// Add to incoming calls (callee -> callers)
		self.incoming
			.entry(callee.clone())
			.or_default()
			.push(CallSite {
				location: location.clone(),
				caller: caller.clone(),
				call_type,
			});

		// Add to outgoing calls (caller -> callees) if caller is known
		if let Some(caller) = caller {
			self.outgoing
				.entry(caller)
				.or_default()
				.push((callee, location));
		}
	}

	/// Get all call sites that invoke a callable (incoming calls).
	pub fn get_incoming(&self, callee: &CallableId) -> Vec<CallSite> {
		self.incoming
			.get(callee)
			.map(|v| v.value().clone())
			.unwrap_or_default()
	}

	/// Get all callees from a callable (outgoing calls).
	pub fn get_outgoing(&self, caller: &CallableId) -> Vec<(CallableId, MinLoc)> {
		self.outgoing
			.get(caller)
			.map(|v| v.value().clone())
			.unwrap_or_default()
	}

	/// Clear all call data associated with a specific file.
	///
	/// This is used for incremental updates when a file changes.
	pub fn clear_file(&self, path: PathSymbol) {
		let path_str = path.to_string();

		// Remove incoming calls from this file
		self.incoming.retain(|_, sites| {
			sites.retain(|site| site.location.path.to_string() != path_str);
			!sites.is_empty()
		});

		// Remove outgoing calls from callables in this file
		self.outgoing.retain(|caller, _| {
			match caller {
				CallableId::Function { path: p, .. } => p != &path_str,
				// Methods are keyed by model, not file, so we keep them
				// (they'll be re-populated when the model is re-indexed)
				CallableId::Method { .. } => true,
			}
		});

		// Also clean outgoing call entries that point to locations in this file
		for mut entry in self.outgoing.iter_mut() {
			entry.value_mut().retain(|(_, loc)| loc.path.to_string() != path_str);
		}

		// Mark file as not indexed
		if let Ok(mut indexed) = self.indexed_files.write() {
			indexed.remove(&path);
		}
	}

	/// Clear all call data for a model's methods.
	///
	/// This is used when a model is re-indexed.
	pub fn clear_model(&self, model: &str) {
		// Remove incoming calls to this model's methods
		self.incoming.retain(|callee, _| {
			!matches!(callee, CallableId::Method { model: m, .. } if m == model)
		});

		// Remove outgoing calls from this model's methods
		self.outgoing.retain(|caller, _| {
			!matches!(caller, CallableId::Method { model: m, .. } if m == model)
		});
	}

	/// Check if a file has been indexed for call collection.
	pub fn is_file_indexed(&self, path: &PathSymbol) -> bool {
		self.indexed_files
			.read()
			.map(|indexed| indexed.contains(path))
			.unwrap_or(false)
	}

	/// Mark a file as indexed for call collection.
	pub fn mark_file_indexed(&self, path: PathSymbol) {
		if let Ok(mut indexed) = self.indexed_files.write() {
			indexed.insert(path);
		}
	}

	/// Get the number of tracked call relationships (for debugging).
	pub fn stats(&self) -> (usize, usize) {
		let incoming_count: usize = self.incoming.iter().map(|e| e.value().len()).sum();
		let outgoing_count: usize = self.outgoing.iter().map(|e| e.value().len()).sum();
		(incoming_count, outgoing_count)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use tower_lsp_server::ls_types::{Position, Range};

	fn make_loc(path: PathSymbol, line: u32) -> MinLoc {
		MinLoc {
			path,
			range: Range {
				start: Position { line, character: 0 },
				end: Position { line, character: 10 },
			},
		}
	}
	
	fn make_path(root: &str, subpath: &str) -> PathSymbol {
		use crate::index::symbol::_I;
		PathSymbol::strip_root(_I(root), std::path::Path::new(&format!("{}/{}", root, subpath)))
	}

	#[test]
	fn test_add_and_get_calls() {
		let graph = CallGraph::default();
		
		let caller = CallableId::method("sale.order", "action_confirm");
		let callee = CallableId::method("sale.order", "_check_order_state");
		let path = make_path("/test", "file.py");
		let location = make_loc(path, 10);

		graph.add_call(Some(caller.clone()), callee.clone(), location.clone(), CallType::Direct);

		// Check incoming calls
		let incoming = graph.get_incoming(&callee);
		assert_eq!(incoming.len(), 1);
		assert_eq!(incoming[0].caller, Some(caller.clone()));
		assert_eq!(incoming[0].call_type, CallType::Direct);

		// Check outgoing calls
		let outgoing = graph.get_outgoing(&caller);
		assert_eq!(outgoing.len(), 1);
		assert_eq!(outgoing[0].0, callee);
	}

	#[test]
	fn test_super_calls() {
		let graph = CallGraph::default();
		
		let caller = CallableId::method("sale.order.custom", "action_confirm");
		let callee = CallableId::method("sale.order", "action_confirm");
		let path = make_path("/test", "custom.py");
		let location = make_loc(path, 15);

		graph.add_call(Some(caller.clone()), callee.clone(), location, CallType::Super);

		let incoming = graph.get_incoming(&callee);
		assert_eq!(incoming.len(), 1);
		assert_eq!(incoming[0].call_type, CallType::Super);
	}

	#[test]
	fn test_clear_file() {
		let graph = CallGraph::default();
		
		let caller1 = CallableId::function("/test/a.py", "func_a");
		let caller2 = CallableId::function("/test/b.py", "func_b");
		let callee = CallableId::function("/test/c.py", "func_c");

		let path_a = make_path("/test", "a.py");
		let path_b = make_path("/test", "b.py");
		
		let loc_a = make_loc(path_a, 10);
		let loc_b = make_loc(path_b, 20);

		graph.add_call(Some(caller1.clone()), callee.clone(), loc_a, CallType::Direct);
		graph.add_call(Some(caller2.clone()), callee.clone(), loc_b, CallType::Direct);

		assert_eq!(graph.get_incoming(&callee).len(), 2);

		// Clear file a
		graph.clear_file(make_path("/test", "a.py"));

		// Should only have one incoming call now (from file b)
		let incoming = graph.get_incoming(&callee);
		assert_eq!(incoming.len(), 1);
		assert_eq!(incoming[0].caller, Some(caller2));
	}
}
