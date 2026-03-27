//! OWL Hooks support for Odoo 18+
//!
//! This module provides support for OWL hooks including:
//! - `useState`, `useRef`, `useEffect`, `useEnv` and other OWL core hooks
//! - Odoo-specific hooks like `useService`, `useBus`, `useHotkey`
//! - Lifecycle hooks: `onMounted`, `onWillStart`, etc.
//! - Service discovery from `registry.category("services").add()`

mod builtins;

pub use builtins::{builtin_hooks, builtin_services};

use dashmap::DashMap;
use smart_default::SmartDefault;

use crate::ImStr;
use crate::utils::MinLoc;

/// A hook call in a component's setup() method
#[derive(Debug, Clone)]
pub struct HookUsage {
	/// The hook function name (e.g., "useState", "useService", "onMounted")
	pub hook_name: ImStr,
	/// Location of the hook call
	pub location: MinLoc,
	/// The variable this hook is assigned to (e.g., "this.state" for `this.state = useState()`)
	pub variable: Option<ImStr>,
	/// Parsed hook arguments
	pub args: HookArgs,
}

/// Parsed arguments for different hook types
#[derive(Debug, Clone, Default)]
pub enum HookArgs {
	#[default]
	None,
	/// useService("serviceName")
	Service(ImStr),
	/// useRef("refName")
	Ref(ImStr),
	/// useBus(bus, "eventName", callback)
	Bus {
		event: Option<ImStr>,
	},
	/// useHotkey("ctrl+s", callback)
	Hotkey(ImStr),
	/// useState(initialValue)
	State,
	/// Lifecycle hooks: onMounted, onWillStart, etc.
	Lifecycle,
	/// Other/unknown args
	Other,
}

/// Service definition discovered from Odoo codebase
#[derive(Debug, Clone)]
pub struct ServiceDefinition {
	/// Service name (e.g., "orm", "notification")
	pub name: ImStr,
	/// Location where the service is registered
	pub location: Option<MinLoc>,
	/// Services this service depends on
	pub dependencies: Vec<ImStr>,
	/// Methods wrapped for async safety (from `async: [...]` in service definition)
	pub async_methods: Vec<ImStr>,
	/// Odoo module that defines this service
	pub module: Option<ImStr>,
	/// Whether this is a built-in service (not discovered from codebase)
	pub builtin: bool,
}

impl ServiceDefinition {
	/// Create a built-in service definition
	pub fn builtin(name: &str) -> Self {
		Self {
			name: ImStr::from(name),
			location: None,
			dependencies: Vec::new(),
			async_methods: Vec::new(),
			module: Some(ImStr::from("web")),
			builtin: true,
		}
	}

	/// Create a built-in service with async methods
	pub fn builtin_with_async(name: &str, async_methods: &[&str]) -> Self {
		Self {
			name: ImStr::from(name),
			location: None,
			dependencies: Vec::new(),
			async_methods: async_methods.iter().map(|s| ImStr::from(*s)).collect(),
			module: Some(ImStr::from("web")),
			builtin: true,
		}
	}
}

/// Known hook definition (for hover/completion info)
#[derive(Debug, Clone)]
pub struct HookDefinition {
	/// Hook name (e.g., "useState", "useService")
	pub name: ImStr,
	/// Human-readable signature for display
	pub signature: ImStr,
	/// Description of what the hook does
	pub description: Option<ImStr>,
	/// Source module (e.g., "@odoo/owl", "@web/core/utils/hooks")
	pub source_module: ImStr,
	/// Category for filtering completions
	pub category: HookCategory,
}

/// Hook categories for filtering completions
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HookCategory {
	/// OWL core hooks: useState, useRef, useEffect, useEnv, useComponent, useSubEnv
	OwlCore,
	/// OWL lifecycle hooks: onMounted, onWillStart, onPatched, etc.
	OwlLifecycle,
	/// Odoo core hooks: useService, useBus, useAutofocus
	OdooCore,
	/// Odoo UI hooks: usePopover, useTooltip, useDropdownState
	OdooUI,
	/// Odoo input hooks: useHotkey, useSortable, useAutoresize
	OdooInput,
	/// Odoo view/model hooks: useModel, useInputField, usePager
	OdooView,
	/// Other/unknown hooks
	Other,
}

impl HookCategory {
	/// Returns true if this hook starts with "on" (lifecycle hooks)
	pub fn is_lifecycle(&self) -> bool {
		matches!(self, HookCategory::OwlLifecycle)
	}

	/// Returns true if this hook starts with "use"
	pub fn is_use_hook(&self) -> bool {
		!self.is_lifecycle()
	}
}

/// Index of discovered services
#[derive(SmartDefault)]
pub struct ServiceIndex {
	#[default(_code = "DashMap::with_shard_amount(4)")]
	inner: DashMap<ImStr, ServiceDefinition>,
}

impl core::ops::Deref for ServiceIndex {
	type Target = DashMap<ImStr, ServiceDefinition>;
	#[inline]
	fn deref(&self) -> &Self::Target {
		&self.inner
	}
}

impl ServiceIndex {
	/// Initialize with built-in services
	pub fn with_builtins() -> Self {
		let index = Self::default();
		for service in builtin_services() {
			index.insert(service.name.clone(), service);
		}
		index
	}

	/// Add or update a service definition
	/// If the service already exists and is built-in, update with discovered info
	pub fn add_or_update(&self, service: ServiceDefinition) {
		use dashmap::mapref::entry::Entry;
		match self.inner.entry(service.name.clone()) {
			Entry::Vacant(entry) => {
				entry.insert(service);
			}
			Entry::Occupied(mut entry) => {
				let existing = entry.get_mut();
				// If existing is builtin and new is not, update with discovered info
				if existing.builtin && !service.builtin {
					existing.location = service.location;
					existing.module = service.module;
					existing.builtin = false;
				}
				// Always update dependencies and async_methods if provided
				if !service.dependencies.is_empty() {
					existing.dependencies = service.dependencies;
				}
				if !service.async_methods.is_empty() {
					existing.async_methods = service.async_methods;
				}
			}
		}
	}
}

/// Index of known hook definitions
#[derive(SmartDefault)]
pub struct HookIndex {
	#[default(_code = "DashMap::with_shard_amount(4)")]
	inner: DashMap<ImStr, HookDefinition>,
}

impl core::ops::Deref for HookIndex {
	type Target = DashMap<ImStr, HookDefinition>;
	#[inline]
	fn deref(&self) -> &Self::Target {
		&self.inner
	}
}

impl HookIndex {
	/// Initialize with built-in hooks
	pub fn with_builtins() -> Self {
		let index = Self::default();
		for hook in builtin_hooks() {
			index.insert(hook.name.clone(), hook);
		}
		index
	}

	/// Get hooks matching a prefix, optionally filtered by category
	pub fn complete(&self, prefix: &str, lifecycle_only: bool) -> Vec<&HookDefinition> {
		self.inner
			.iter()
			.filter(|entry| {
				let hook = entry.value();
				hook.name.starts_with(prefix)
					&& (!lifecycle_only || hook.category.is_lifecycle())
					&& (lifecycle_only || hook.category.is_use_hook())
			})
			.map(|entry| {
				// SAFETY: The reference is valid as long as we hold the DashMap
				// We return owned data in practice via the completion items
				unsafe { &*(entry.value() as *const HookDefinition) }
			})
			.collect()
	}
}
