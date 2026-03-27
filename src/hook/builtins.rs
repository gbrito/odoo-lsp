//! Built-in hook and service definitions for Odoo 18+
//!
//! These definitions provide completions, hover info, and validation
//! even before the full codebase is indexed.

use crate::ImStr;

use super::{HookCategory, HookDefinition, ServiceDefinition};

/// Returns all built-in hook definitions
pub fn builtin_hooks() -> Vec<HookDefinition> {
	let mut hooks = Vec::with_capacity(80);

	// === OWL Core Hooks ===
	hooks.extend(owl_core_hooks());

	// === OWL Lifecycle Hooks ===
	hooks.extend(owl_lifecycle_hooks());

	// === Odoo Core Hooks ===
	hooks.extend(odoo_core_hooks());

	// === Odoo UI Hooks ===
	hooks.extend(odoo_ui_hooks());

	// === Odoo Input Hooks ===
	hooks.extend(odoo_input_hooks());

	// === Odoo View/Model Hooks ===
	hooks.extend(odoo_view_hooks());

	hooks
}

fn owl_core_hooks() -> Vec<HookDefinition> {
	vec![
		HookDefinition {
			name: ImStr::from("useState"),
			signature: ImStr::from("<T>(initialValue: T) => T"),
			description: Some(ImStr::from(
				"Creates reactive state that triggers re-render on change. \
				The returned object is a reactive proxy.",
			)),
			source_module: ImStr::from("@odoo/owl"),
			category: HookCategory::OwlCore,
		},
		HookDefinition {
			name: ImStr::from("useRef"),
			signature: ImStr::from("(refName: string) => { el: HTMLElement | null }"),
			description: Some(ImStr::from(
				"Creates a reference to a DOM element marked with t-ref in the template.",
			)),
			source_module: ImStr::from("@odoo/owl"),
			category: HookCategory::OwlCore,
		},
		HookDefinition {
			name: ImStr::from("useEffect"),
			signature: ImStr::from("(effect: () => void | (() => void), deps?: () => any[]) => void"),
			description: Some(ImStr::from(
				"Runs a side effect after render. The effect can return a cleanup function. \
				Dependencies array determines when the effect re-runs.",
			)),
			source_module: ImStr::from("@odoo/owl"),
			category: HookCategory::OwlCore,
		},
		HookDefinition {
			name: ImStr::from("useEnv"),
			signature: ImStr::from("() => Environment"),
			description: Some(ImStr::from(
				"Returns the component's environment object containing services and configuration.",
			)),
			source_module: ImStr::from("@odoo/owl"),
			category: HookCategory::OwlCore,
		},
		HookDefinition {
			name: ImStr::from("useComponent"),
			signature: ImStr::from("() => Component"),
			description: Some(ImStr::from(
				"Returns the current component instance. Useful in hooks to access component methods.",
			)),
			source_module: ImStr::from("@odoo/owl"),
			category: HookCategory::OwlCore,
		},
		HookDefinition {
			name: ImStr::from("useSubEnv"),
			signature: ImStr::from("(envExtension: object) => void"),
			description: Some(ImStr::from(
				"Extends the environment for the current component and its descendants.",
			)),
			source_module: ImStr::from("@odoo/owl"),
			category: HookCategory::OwlCore,
		},
		HookDefinition {
			name: ImStr::from("useChildSubEnv"),
			signature: ImStr::from("(envExtension: object) => void"),
			description: Some(ImStr::from(
				"Extends the environment for child components only (not the current component).",
			)),
			source_module: ImStr::from("@odoo/owl"),
			category: HookCategory::OwlCore,
		},
		HookDefinition {
			name: ImStr::from("useExternalListener"),
			signature: ImStr::from(
				"(target: EventTarget, eventName: string, handler: EventListener, options?: AddEventListenerOptions) => void",
			),
			description: Some(ImStr::from(
				"Adds an event listener to an external target (like window or document) \
				that is automatically cleaned up when the component is destroyed.",
			)),
			source_module: ImStr::from("@odoo/owl"),
			category: HookCategory::OwlCore,
		},
	]
}

fn owl_lifecycle_hooks() -> Vec<HookDefinition> {
	vec![
		HookDefinition {
			name: ImStr::from("onWillStart"),
			signature: ImStr::from("(callback: () => void | Promise<void>) => void"),
			description: Some(ImStr::from(
				"Called before the component is mounted for the first time. \
				Can be async to delay initial rendering.",
			)),
			source_module: ImStr::from("@odoo/owl"),
			category: HookCategory::OwlLifecycle,
		},
		HookDefinition {
			name: ImStr::from("onMounted"),
			signature: ImStr::from("(callback: () => void) => void"),
			description: Some(ImStr::from(
				"Called after the component is mounted to the DOM. \
				DOM elements are available at this point.",
			)),
			source_module: ImStr::from("@odoo/owl"),
			category: HookCategory::OwlLifecycle,
		},
		HookDefinition {
			name: ImStr::from("onWillUpdateProps"),
			signature: ImStr::from("(callback: (nextProps: Props) => void | Promise<void>) => void"),
			description: Some(ImStr::from(
				"Called before the component receives new props. \
				Can be async to delay the update.",
			)),
			source_module: ImStr::from("@odoo/owl"),
			category: HookCategory::OwlLifecycle,
		},
		HookDefinition {
			name: ImStr::from("onWillRender"),
			signature: ImStr::from("(callback: () => void) => void"),
			description: Some(ImStr::from(
				"Called before each render (initial and updates). \
				Useful for preparing data before rendering.",
			)),
			source_module: ImStr::from("@odoo/owl"),
			category: HookCategory::OwlLifecycle,
		},
		HookDefinition {
			name: ImStr::from("onRendered"),
			signature: ImStr::from("(callback: () => void) => void"),
			description: Some(ImStr::from(
				"Called after each render, before DOM patching.",
			)),
			source_module: ImStr::from("@odoo/owl"),
			category: HookCategory::OwlLifecycle,
		},
		HookDefinition {
			name: ImStr::from("onPatched"),
			signature: ImStr::from("(callback: () => void) => void"),
			description: Some(ImStr::from(
				"Called after the component's DOM has been patched (updated). \
				Not called on initial mount.",
			)),
			source_module: ImStr::from("@odoo/owl"),
			category: HookCategory::OwlLifecycle,
		},
		HookDefinition {
			name: ImStr::from("onWillUnmount"),
			signature: ImStr::from("(callback: () => void) => void"),
			description: Some(ImStr::from(
				"Called just before the component is removed from the DOM. \
				Use for cleanup (removing event listeners, etc.).",
			)),
			source_module: ImStr::from("@odoo/owl"),
			category: HookCategory::OwlLifecycle,
		},
		HookDefinition {
			name: ImStr::from("onWillDestroy"),
			signature: ImStr::from("(callback: () => void) => void"),
			description: Some(ImStr::from(
				"Called when the component is about to be destroyed. \
				Final cleanup opportunity.",
			)),
			source_module: ImStr::from("@odoo/owl"),
			category: HookCategory::OwlLifecycle,
		},
		HookDefinition {
			name: ImStr::from("onError"),
			signature: ImStr::from("(callback: (error: Error) => void) => void"),
			description: Some(ImStr::from(
				"Called when an error occurs in the component or its descendants. \
				Can be used to implement error boundaries.",
			)),
			source_module: ImStr::from("@odoo/owl"),
			category: HookCategory::OwlLifecycle,
		},
	]
}

fn odoo_core_hooks() -> Vec<HookDefinition> {
	vec![
		HookDefinition {
			name: ImStr::from("useService"),
			signature: ImStr::from("(serviceName: string) => Service"),
			description: Some(ImStr::from(
				"Imports an Odoo service into the component. \
				Services are singletons providing shared functionality.",
			)),
			source_module: ImStr::from("@web/core/utils/hooks"),
			category: HookCategory::OdooCore,
		},
		HookDefinition {
			name: ImStr::from("useBus"),
			signature: ImStr::from(
				"(bus: EventBus, eventName: string, callback: EventListener) => void",
			),
			description: Some(ImStr::from(
				"Attaches an event listener to a bus that is automatically \
				removed when the component is destroyed.",
			)),
			source_module: ImStr::from("@web/core/utils/hooks"),
			category: HookCategory::OdooCore,
		},
		HookDefinition {
			name: ImStr::from("useAutofocus"),
			signature: ImStr::from("(options?: { refName?: string, selectAll?: boolean, mobile?: boolean }) => Ref"),
			description: Some(ImStr::from(
				"Auto-focuses an element when the component is mounted. \
				By default targets t-ref=\"autofocus\".",
			)),
			source_module: ImStr::from("@web/core/utils/hooks"),
			category: HookCategory::OdooCore,
		},
		HookDefinition {
			name: ImStr::from("useSpellCheck"),
			signature: ImStr::from("(options?: { refName?: string }) => void"),
			description: Some(ImStr::from(
				"Manages the spellcheck attribute on input/textarea elements \
				based on user preferences.",
			)),
			source_module: ImStr::from("@web/core/utils/hooks"),
			category: HookCategory::OdooCore,
		},
		HookDefinition {
			name: ImStr::from("useChildRef"),
			signature: ImStr::from("() => ForwardRef"),
			description: Some(ImStr::from(
				"Creates a ref that can be forwarded by a child component \
				using useForwardRefToParent.",
			)),
			source_module: ImStr::from("@web/core/utils/hooks"),
			category: HookCategory::OdooCore,
		},
		HookDefinition {
			name: ImStr::from("useForwardRefToParent"),
			signature: ImStr::from("(refName: string) => Ref"),
			description: Some(ImStr::from(
				"Forwards a ref to a parent component that created it with useChildRef.",
			)),
			source_module: ImStr::from("@web/core/utils/hooks"),
			category: HookCategory::OdooCore,
		},
		HookDefinition {
			name: ImStr::from("useOwnedDialogs"),
			signature: ImStr::from("() => (Component, props, options?) => () => void"),
			description: Some(ImStr::from(
				"Returns a function to open dialogs that are automatically \
				closed when the owner component is destroyed.",
			)),
			source_module: ImStr::from("@web/core/utils/hooks"),
			category: HookCategory::OdooCore,
		},
		HookDefinition {
			name: ImStr::from("useRefListener"),
			signature: ImStr::from(
				"(ref: Ref, eventName: string, listener: EventListener, options?: AddEventListenerOptions) => void",
			),
			description: Some(ImStr::from(
				"Manages event listeners on a ref's element, handling \
				add/remove automatically with component lifecycle.",
			)),
			source_module: ImStr::from("@web/core/utils/hooks"),
			category: HookCategory::OdooCore,
		},
	]
}

fn odoo_ui_hooks() -> Vec<HookDefinition> {
	vec![
		HookDefinition {
			name: ImStr::from("usePopover"),
			signature: ImStr::from(
				"(Component: ComponentClass, options?: PopoverOptions) => { open, close, isOpen }",
			),
			description: Some(ImStr::from(
				"Creates a popover that can be programmatically opened and closed.",
			)),
			source_module: ImStr::from("@web/core/popover/popover_hook"),
			category: HookCategory::OdooUI,
		},
		HookDefinition {
			name: ImStr::from("useTooltip"),
			signature: ImStr::from("(refName: string, params: TooltipParams) => void"),
			description: Some(ImStr::from(
				"Attaches a tooltip to an element referenced by t-ref.",
			)),
			source_module: ImStr::from("@web/core/tooltip/tooltip_hook"),
			category: HookCategory::OdooUI,
		},
		HookDefinition {
			name: ImStr::from("useDropdownState"),
			signature: ImStr::from("(options?: { onOpen?, onClose? }) => { open, close, isOpen }"),
			description: Some(ImStr::from(
				"Creates state for managing a dropdown's open/close state.",
			)),
			source_module: ImStr::from("@web/core/dropdown/dropdown_hooks"),
			category: HookCategory::OdooUI,
		},
		HookDefinition {
			name: ImStr::from("useDropdownCloser"),
			signature: ImStr::from("() => { close, closeChildren, closeAll }"),
			description: Some(ImStr::from(
				"Returns functions to close dropdowns in the current hierarchy.",
			)),
			source_module: ImStr::from("@web/core/dropdown/dropdown_hooks"),
			category: HookCategory::OdooUI,
		},
		HookDefinition {
			name: ImStr::from("useDropdownNesting"),
			signature: ImStr::from("(state: DropdownState) => void"),
			description: Some(ImStr::from(
				"Integrates a dropdown with the nesting system for proper close behavior.",
			)),
			source_module: ImStr::from("@web/core/dropdown/dropdown_hooks"),
			category: HookCategory::OdooUI,
		},
		HookDefinition {
			name: ImStr::from("useDropdownGroup"),
			signature: ImStr::from("() => DropdownGroup"),
			description: Some(ImStr::from(
				"Creates a dropdown group for managing multiple related dropdowns.",
			)),
			source_module: ImStr::from("@web/core/dropdown/dropdown_hooks"),
			category: HookCategory::OdooUI,
		},
		HookDefinition {
			name: ImStr::from("usePosition"),
			signature: ImStr::from(
				"(refName: string, getTarget: () => Element, options?: PositionOptions) => { lock, unlock }",
			),
			description: Some(ImStr::from(
				"Positions an element relative to a target element with automatic updates.",
			)),
			source_module: ImStr::from("@web/core/position/position_hook"),
			category: HookCategory::OdooUI,
		},
		HookDefinition {
			name: ImStr::from("useTransition"),
			signature: ImStr::from(
				"(options: { name, initialVisibility?, immediate?, leaveDuration?, onLeave? }) => { shouldMount, className, stage }",
			),
			description: Some(ImStr::from(
				"Manages CSS transition states for mount/unmount animations.",
			)),
			source_module: ImStr::from("@web/core/transition"),
			category: HookCategory::OdooUI,
		},
		HookDefinition {
			name: ImStr::from("useActiveElement"),
			signature: ImStr::from("(refName: string) => void"),
			description: Some(ImStr::from(
				"Tracks and manages the active element for accessibility.",
			)),
			source_module: ImStr::from("@web/core/ui/ui_service"),
			category: HookCategory::OdooUI,
		},
	]
}

fn odoo_input_hooks() -> Vec<HookDefinition> {
	vec![
		HookDefinition {
			name: ImStr::from("useHotkey"),
			signature: ImStr::from(
				"(hotkey: string, callback: () => void, options?: HotkeyOptions) => void",
			),
			description: Some(ImStr::from(
				"Registers a keyboard shortcut that triggers the callback. \
				Automatically unregistered on component destroy.",
			)),
			source_module: ImStr::from("@web/core/hotkeys/hotkey_hook"),
			category: HookCategory::OdooInput,
		},
		HookDefinition {
			name: ImStr::from("useCommand"),
			signature: ImStr::from(
				"(name: string, action: () => void, options?: CommandOptions) => void",
			),
			description: Some(ImStr::from(
				"Registers a command in the command palette.",
			)),
			source_module: ImStr::from("@web/core/commands/command_hook"),
			category: HookCategory::OdooInput,
		},
		HookDefinition {
			name: ImStr::from("useSortable"),
			signature: ImStr::from("(params: SortableParams) => Sortable"),
			description: Some(ImStr::from(
				"Enables drag-and-drop sorting functionality for a list of elements.",
			)),
			source_module: ImStr::from("@web/core/utils/sortable_owl"),
			category: HookCategory::OdooInput,
		},
		HookDefinition {
			name: ImStr::from("useAutoresize"),
			signature: ImStr::from("(ref: Ref, options?: AutoresizeOptions) => void"),
			description: Some(ImStr::from(
				"Automatically resizes a textarea to fit its content.",
			)),
			source_module: ImStr::from("@web/core/utils/autoresize"),
			category: HookCategory::OdooInput,
		},
		HookDefinition {
			name: ImStr::from("useDebounced"),
			signature: ImStr::from(
				"<T>(callback: T, delay: number, options?: { execBeforeUnmount?, immediate?, trailing? }) => T & { cancel }",
			),
			description: Some(ImStr::from(
				"Creates a debounced version of a function that delays execution.",
			)),
			source_module: ImStr::from("@web/core/utils/timing"),
			category: HookCategory::OdooInput,
		},
		HookDefinition {
			name: ImStr::from("useThrottleForAnimation"),
			signature: ImStr::from("<T>(func: T) => T & { cancel }"),
			description: Some(ImStr::from(
				"Throttles a function to run at most once per animation frame.",
			)),
			source_module: ImStr::from("@web/core/utils/timing"),
			category: HookCategory::OdooInput,
		},
		HookDefinition {
			name: ImStr::from("useDateTimePicker"),
			signature: ImStr::from("(params: DateTimePickerParams) => { state, open }"),
			description: Some(ImStr::from(
				"Creates a date/time picker that can be opened programmatically.",
			)),
			source_module: ImStr::from("@web/core/datetime/datetime_hook"),
			category: HookCategory::OdooInput,
		},
		HookDefinition {
			name: ImStr::from("useDropzone"),
			signature: ImStr::from(
				"(targetRef: Ref, onDrop: (files: FileList) => void, extraClass?: string, isEnabled?: () => boolean) => void",
			),
			description: Some(ImStr::from(
				"Enables file drop functionality on an element.",
			)),
			source_module: ImStr::from("@web/core/dropzone/dropzone_hook"),
			category: HookCategory::OdooInput,
		},
		HookDefinition {
			name: ImStr::from("useCustomDropzone"),
			signature: ImStr::from(
				"(targetRef: Ref, dropzoneComponent: Component, props: object, isEnabled?: () => boolean) => void",
			),
			description: Some(ImStr::from(
				"Enables custom dropzone with a custom overlay component.",
			)),
			source_module: ImStr::from("@web/core/dropzone/dropzone_hook"),
			category: HookCategory::OdooInput,
		},
		HookDefinition {
			name: ImStr::from("useNavigation"),
			signature: ImStr::from("(containerRef: Ref, options?: NavigationOptions) => { enable, disable }"),
			description: Some(ImStr::from(
				"Enables keyboard navigation within a container.",
			)),
			source_module: ImStr::from("@web/core/navigation/navigation"),
			category: HookCategory::OdooInput,
		},
		HookDefinition {
			name: ImStr::from("useTagNavigation"),
			signature: ImStr::from("(refName: string, deleteTag: (index: number) => void) => void"),
			description: Some(ImStr::from(
				"Enables keyboard navigation for tag/chip components.",
			)),
			source_module: ImStr::from("@web/core/record_selectors/tag_navigation_hook"),
			category: HookCategory::OdooInput,
		},
		HookDefinition {
			name: ImStr::from("useEmojiPicker"),
			signature: ImStr::from("(ref: Ref, props: object, options?: object) => { add, toggle, ... }"),
			description: Some(ImStr::from(
				"Attaches an emoji picker to an input element.",
			)),
			source_module: ImStr::from("@web/core/emoji_picker/emoji_picker"),
			category: HookCategory::OdooInput,
		},
		HookDefinition {
			name: ImStr::from("useFileViewer"),
			signature: ImStr::from("() => { open, close }"),
			description: Some(ImStr::from(
				"Returns functions to open/close the file viewer.",
			)),
			source_module: ImStr::from("@web/core/file_viewer/file_viewer_hook"),
			category: HookCategory::OdooInput,
		},
	]
}

fn odoo_view_hooks() -> Vec<HookDefinition> {
	vec![
		HookDefinition {
			name: ImStr::from("useModel"),
			signature: ImStr::from(
				"(ModelClass: ModelConstructor, params: ModelParams, options?: ModelOptions) => Model",
			),
			description: Some(ImStr::from(
				"Creates and manages a model instance for data loading and manipulation.",
			)),
			source_module: ImStr::from("@web/model/model"),
			category: HookCategory::OdooView,
		},
		HookDefinition {
			name: ImStr::from("useModelWithSampleData"),
			signature: ImStr::from(
				"(ModelClass: ModelConstructor, params: ModelParams, options?: ModelOptions) => Model",
			),
			description: Some(ImStr::from(
				"Like useModel but provides sample data when the model is empty.",
			)),
			source_module: ImStr::from("@web/model/model"),
			category: HookCategory::OdooView,
		},
		HookDefinition {
			name: ImStr::from("useRecordObserver"),
			signature: ImStr::from("(callback: (record: Record) => void) => void"),
			description: Some(ImStr::from(
				"Observes changes to the current record and triggers callback on updates.",
			)),
			source_module: ImStr::from("@web/model/relational_model/utils"),
			category: HookCategory::OdooView,
		},
		HookDefinition {
			name: ImStr::from("useInputField"),
			signature: ImStr::from("(params: InputFieldParams) => InputFieldResult"),
			description: Some(ImStr::from(
				"Provides common input field functionality like validation and formatting.",
			)),
			source_module: ImStr::from("@web/views/fields/input_field_hook"),
			category: HookCategory::OdooView,
		},
		HookDefinition {
			name: ImStr::from("useViewButtons"),
			signature: ImStr::from("(ref: Ref, options?: ViewButtonOptions) => void"),
			description: Some(ImStr::from(
				"Manages button states and actions in views.",
			)),
			source_module: ImStr::from("@web/views/view_button/view_button_hook"),
			category: HookCategory::OdooView,
		},
		HookDefinition {
			name: ImStr::from("usePager"),
			signature: ImStr::from("(getProps: () => PagerProps) => PagerState"),
			description: Some(ImStr::from(
				"Creates pager state for navigating through records.",
			)),
			source_module: ImStr::from("@web/search/pager_hook"),
			category: HookCategory::OdooView,
		},
		HookDefinition {
			name: ImStr::from("useSetupAction"),
			signature: ImStr::from("(params?: SetupActionParams) => void"),
			description: Some(ImStr::from(
				"Sets up action-related functionality for view components.",
			)),
			source_module: ImStr::from("@web/search/action_hook"),
			category: HookCategory::OdooView,
		},
		HookDefinition {
			name: ImStr::from("useViewArch"),
			signature: ImStr::from("(arch: string, params?: ViewArchParams) => void"),
			description: Some(ImStr::from(
				"Processes and validates a view architecture.",
			)),
			source_module: ImStr::from("@web/views/view_hook"),
			category: HookCategory::OdooView,
		},
		HookDefinition {
			name: ImStr::from("useActionLinks"),
			signature: ImStr::from("(options: { resModel: string, reload: () => void }) => void"),
			description: Some(ImStr::from(
				"Handles action links in views (e.g., mailto:, tel:).",
			)),
			source_module: ImStr::from("@web/views/view_hook"),
			category: HookCategory::OdooView,
		},
		HookDefinition {
			name: ImStr::from("useBounceButton"),
			signature: ImStr::from("(containerRef: Ref, shouldBounce: () => boolean) => void"),
			description: Some(ImStr::from(
				"Adds bounce animation to a button when a condition is met.",
			)),
			source_module: ImStr::from("@web/views/view_hook"),
			category: HookCategory::OdooView,
		},
		HookDefinition {
			name: ImStr::from("useSelectCreate"),
			signature: ImStr::from(
				"(options: { resModel, activeActions, onSelected, onCreateEdit, onUnselect }) => selectCreate",
			),
			description: Some(ImStr::from(
				"Provides select/create functionality for relational fields.",
			)),
			source_module: ImStr::from("@web/views/fields/relational_utils"),
			category: HookCategory::OdooView,
		},
		HookDefinition {
			name: ImStr::from("useX2ManyCrud"),
			signature: ImStr::from("(getList: () => List, isMany2Many: boolean) => CrudMethods"),
			description: Some(ImStr::from(
				"Provides CRUD operations for x2many fields.",
			)),
			source_module: ImStr::from("@web/views/fields/relational_utils"),
			category: HookCategory::OdooView,
		},
		HookDefinition {
			name: ImStr::from("useSpecialData"),
			signature: ImStr::from("<T>(loadFn: (orm, props) => Promise<T>) => { data: Record<string, T> }"),
			description: Some(ImStr::from(
				"Loads special data for fields (e.g., selection options).",
			)),
			source_module: ImStr::from("@web/views/fields/relational_utils"),
			category: HookCategory::OdooView,
		},
		HookDefinition {
			name: ImStr::from("useNumpadDecimal"),
			signature: ImStr::from("() => void"),
			description: Some(ImStr::from(
				"Handles numpad decimal key based on locale settings.",
			)),
			source_module: ImStr::from("@web/views/fields/relational_utils"),
			category: HookCategory::OdooView,
		},
		HookDefinition {
			name: ImStr::from("useDynamicPlaceholder"),
			signature: ImStr::from("(elementRef: Ref) => void"),
			description: Some(ImStr::from(
				"Enables dynamic placeholder functionality for input fields.",
			)),
			source_module: ImStr::from("@web/views/fields/relational_utils"),
			category: HookCategory::OdooView,
		},
		HookDefinition {
			name: ImStr::from("useCalendarPopover"),
			signature: ImStr::from("(Component: ComponentClass) => Popover"),
			description: Some(ImStr::from(
				"Creates a popover for calendar events.",
			)),
			source_module: ImStr::from("@web/views/calendar/hooks"),
			category: HookCategory::OdooView,
		},
		HookDefinition {
			name: ImStr::from("useFullCalendar"),
			signature: ImStr::from("(refName: string, params: FullCalendarParams) => void"),
			description: Some(ImStr::from(
				"Initializes and manages a FullCalendar instance.",
			)),
			source_module: ImStr::from("@web/views/calendar/hooks"),
			category: HookCategory::OdooView,
		},
		HookDefinition {
			name: ImStr::from("useProgressBar"),
			signature: ImStr::from(
				"(progressAttributes, model, aggregateFields, activeBars) => ProgressBar",
			),
			description: Some(ImStr::from(
				"Creates progress bar state for kanban views.",
			)),
			source_module: ImStr::from("@web/views/kanban/progress_bar_hook"),
			category: HookCategory::OdooView,
		},
		HookDefinition {
			name: ImStr::from("useMagicColumnWidths"),
			signature: ImStr::from("(tableRef: Ref, getState: () => State) => void"),
			description: Some(ImStr::from(
				"Automatically adjusts column widths in list views.",
			)),
			source_module: ImStr::from("@web/views/list/column_width_hook"),
			category: HookCategory::OdooView,
		},
		HookDefinition {
			name: ImStr::from("useVirtualGrid"),
			signature: ImStr::from(
				"(options: { scrollableRef, initialScroll?, onChange?, bufferCoef? }) => VirtualGridState",
			),
			description: Some(ImStr::from(
				"Provides virtual scrolling for large grids/tables.",
			)),
			source_module: ImStr::from("@web/core/virtual_grid_hook"),
			category: HookCategory::OdooView,
		},
		HookDefinition {
			name: ImStr::from("useRegistry"),
			signature: ImStr::from("(registry: Registry) => { entries: [string, any][] }"),
			description: Some(ImStr::from(
				"Provides reactive access to a registry's entries.",
			)),
			source_module: ImStr::from("@web/core/registry_hook"),
			category: HookCategory::OdooView,
		},
		HookDefinition {
			name: ImStr::from("useOwnDebugContext"),
			signature: ImStr::from("(options?: { categories?: string[] }) => void"),
			description: Some(ImStr::from(
				"Sets up debug context for the component.",
			)),
			source_module: ImStr::from("@web/core/debug/debug_context"),
			category: HookCategory::OdooView,
		},
		HookDefinition {
			name: ImStr::from("useEnvDebugContext"),
			signature: ImStr::from("() => DebugContext"),
			description: Some(ImStr::from(
				"Returns the debug context from the environment.",
			)),
			source_module: ImStr::from("@web/core/debug/debug_context"),
			category: HookCategory::OdooView,
		},
	]
}

/// Returns all built-in service definitions
pub fn builtin_services() -> Vec<ServiceDefinition> {
	vec![
		// Core services with async methods
		ServiceDefinition::builtin_with_async(
			"orm",
			&[
				"call",
				"create",
				"nameGet",
				"read",
				"readGroup",
				"search",
				"searchRead",
				"unlink",
				"webSearchRead",
				"write",
			],
		),
		ServiceDefinition::builtin_with_async(
			"field",
			&["loadFields", "loadPath", "loadPropertyDefinitions"],
		),
		ServiceDefinition::builtin_with_async("name", &["loadDisplayNames"]),
		ServiceDefinition::builtin_with_async("view", &["loadViews"]),
		// Core services without async methods
		ServiceDefinition::builtin("action"),
		ServiceDefinition::builtin("command"),
		ServiceDefinition::builtin("company"),
		ServiceDefinition::builtin("currency"),
		ServiceDefinition::builtin("dialog"),
		ServiceDefinition::builtin("effect"),
		ServiceDefinition::builtin("error"),
		ServiceDefinition::builtin("file_upload"),
		ServiceDefinition::builtin("hotkey"),
		ServiceDefinition::builtin("http"),
		ServiceDefinition::builtin("localization"),
		ServiceDefinition::builtin("menu"),
		ServiceDefinition::builtin("notification"),
		ServiceDefinition::builtin("overlay"),
		ServiceDefinition::builtin("popover"),
		ServiceDefinition::builtin("rpc"),
		ServiceDefinition::builtin("router"),
		ServiceDefinition::builtin("sortable"),
		ServiceDefinition::builtin("title"),
		ServiceDefinition::builtin("tooltip"),
		ServiceDefinition::builtin("ui"),
		ServiceDefinition::builtin("user"),
		ServiceDefinition::builtin("datetime_picker"),
	]
}
