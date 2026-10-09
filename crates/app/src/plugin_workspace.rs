//! Plugin workspace: a dockable host for plugin views.
//!
//! The workspace window owns a gpui-component [`DockArea`]; each plugin view
//! becomes a [`PluginDockPanel`] that can be tabbed, dragged, split and closed.
//! Element events still travel to the plugin as `view.invoke` exactly as they
//! do for the launcher's inline rendering, so a docked view is the same view,
//! just hosted in a window the user can rearrange.
//!
//! Because the fixed-height views (calendar/list/detail/form/grid/search) need
//! a pixel height to virtualize and the dock decides that height at layout
//! time, the dock panel measures its own content area through
//! `on_children_prepainted` and re-renders the body once the measurement is
//! known. Layout is persisted through the storage settings table and rebuilt on
//! the next launch through the dock's `PanelRegistry`.

use std::{
    cell::{Cell, RefCell},
    collections::HashSet,
    rc::Rc,
};

use gpui::{
    div, prelude::*, px, size, AnyElement, AnyWindowHandle, App, Bounds, Context, Entity,
    EventEmitter, FocusHandle, Focusable, IntoElement, Render, SharedString, Subscription,
    TitlebarOptions, Window, WindowBackgroundAppearance, WindowBounds, WindowKind, WindowOptions,
};
use gpui_component::dock::{
    panel_handle, register_panel, BasePanelView, DockArea, DockEvent, DockPlacement, DockSkin,
    Panel as ComponentPanel, PanelBuildContext, PanelEvent, PanelHandle, PanelInfo, PanelState,
};
use gpui_component::Root;
use serde_json::Value;
use steward_ui_components::virtual_tree::{validate_view, VirtualTreeView};

use crate::i18n::Localization;
use crate::launcher::LauncherState;
use crate::platform;
use crate::plugin_panel_window::{parse_panel_view, PluginPanelWindow};

/// Storage key holding the serialized `DockAreaState`.
const WORKSPACE_LAYOUT_KEY: &str = "plugin_workspace_layout";
/// `panel_name` under which every plugin panel is registered.
const PANEL_NAME: &str = "steward.plugin";
/// A panel rebuilt from the persisted layout, keyed by its `(plugin_id,
/// command)`: the key is what tells one command's panel apart from another's.
type RestoredPanel = ((String, String), Entity<PluginDockPanel>);
/// Dock layout schema version; bump when the persisted shape changes.
const WORKSPACE_VERSION: usize = 1;

/// Build the event sink a docked `ui` tree uses: element events are forwarded
/// to the plugin's isolate as a `view.invoke`.
fn workspace_event_sink(
    state: Rc<RefCell<LauncherState>>,
    plugin_id: String,
    command: String,
) -> steward_ui_components::virtual_tree::EventSink {
    Rc::new(move |callback_id, node_id, event, value| {
        let Some(callback_id) = callback_id else {
            return;
        };
        let mut payload = serde_json::json!({
            "type": event.as_str(),
            "node_id": node_id,
        });
        if let Some(value) = value {
            payload["value"] = Value::String(value);
        }
        let host = state.borrow().plugin_host.clone();
        if host
            .borrow_mut()
            .invoke_view(&plugin_id, &command, callback_id, &payload)
            .is_none()
        {
            eprintln!("[steward] plugin {plugin_id} not ready for view.invoke");
        }
    })
}

/// The rendered body of a docked panel: a Virtual UI Tree entity, or the
/// existing fixed-view renderer (`calendar`/`list`/`detail`/`form`/`grid`/
/// `search`) running without its own window chrome.
enum DockBody {
    Ui(Entity<VirtualTreeView>),
    Panel(Entity<PluginPanelWindow>),
}

/// One plugin view hosted in the workspace dock.
pub struct PluginDockPanel {
    plugin_id: String,
    command: String,
    title: SharedString,
    /// The raw view, persisted so a panel can be rebuilt after a restart.
    view: Value,
    i18n: Rc<Localization>,
    state: Rc<RefCell<LauncherState>>,
    body: DockBody,
    /// Measured content height; `0.0` until the first prepaint.
    height: Cell<f32>,
    focus: FocusHandle,
}

impl PluginDockPanel {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        plugin_id: String,
        command: String,
        title: SharedString,
        view: Value,
        i18n: Rc<Localization>,
        state: Rc<RefCell<LauncherState>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let body = build_body(&plugin_id, &command, &view, &i18n, &state, window, cx);
        Self {
            plugin_id,
            command,
            title,
            view,
            i18n,
            state,
            body,
            height: Cell::new(0.0),
            focus: cx.focus_handle(),
        }
    }

    /// Replace the displayed view (an element handler or `select` returned a
    /// new one). A `ui` tree replacing another `ui` tree updates in place so
    /// host-owned input state survives; anything else rebuilds the body.
    pub(crate) fn set_view(&mut self, view: Value, window: &mut Window, cx: &mut Context<Self>) {
        let node = validate_view(&view).ok();
        if let (DockBody::Ui(tree), Some(node)) = (&self.body, node) {
            self.view = view;
            let tree = tree.clone();
            tree.update(cx, |tree, cx| tree.set_tree(node, cx));
            return;
        }
        self.view = view;
        self.body = build_body(
            &self.plugin_id,
            &self.command,
            &self.view,
            &self.i18n,
            &self.state,
            window,
            cx,
        );
        cx.notify();
    }
}

/// Build the rendered body for a raw plugin view.
fn build_body(
    plugin_id: &str,
    command: &str,
    view: &Value,
    i18n: &Rc<Localization>,
    state: &Rc<RefCell<LauncherState>>,
    window: &mut Window,
    cx: &mut Context<PluginDockPanel>,
) -> DockBody {
    if let Ok(node) = validate_view(view) {
        let sink = workspace_event_sink(state.clone(), plugin_id.to_string(), command.to_string());
        return DockBody::Ui(cx.new(|cx| VirtualTreeView::new(node, sink, cx)));
    }
    if let Some((kind, actions)) = parse_panel_view(view, plugin_id, command, true) {
        let panel = cx.new(|cx| {
            PluginPanelWindow::new_docked(
                state.clone(),
                i18n.clone(),
                plugin_id.to_string(),
                command.to_string(),
                kind,
                actions,
                window,
                cx,
            )
        });
        return DockBody::Panel(panel);
    }
    // A view this build cannot host renders as an empty, valid tree.
    let empty = validate_view(&serde_json::json!({
        "type": "ui",
        "root": { "kind": "col" }
    }))
    .expect("the empty view is valid");
    let sink = workspace_event_sink(state.clone(), plugin_id.to_string(), command.to_string());
    DockBody::Ui(cx.new(|cx| VirtualTreeView::new(empty, sink, cx)))
}

impl EventEmitter<PanelEvent> for PluginDockPanel {}

impl Focusable for PluginDockPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for PluginDockPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let height = self.height.get();
        let content: AnyElement = if height > 0.0 {
            match &self.body {
                DockBody::Ui(tree) => tree.clone().into_any_element(),
                DockBody::Panel(panel) => {
                    panel.update(cx, |panel, cx| panel.render_body(Some(height), window, cx))
                }
            }
        } else {
            div().into_any_element()
        };
        let weak = cx.weak_entity();
        div()
            .relative()
            .size_full()
            .on_children_prepainted(move |bounds, _window, cx| {
                let Some(bounds) = bounds.first() else {
                    return;
                };
                let measured = bounds.size.height.as_f32();
                if let Some(entity) = weak.upgrade() {
                    cx.defer(move |cx| {
                        entity.update(cx, |this, cx| {
                            if (this.height.get() - measured).abs() > 0.5 {
                                this.height.set(measured);
                                cx.notify();
                            }
                        });
                    });
                }
            })
            .child(div().absolute().inset_0())
            .child(div().size_full().child(content))
    }
}

impl gpui_component::dock::BasePanel for PluginDockPanel {
    fn panel_name(&self) -> &'static str {
        PANEL_NAME
    }

    fn dump(&self, _cx: &App) -> PanelState {
        let mut state = PanelState::new(PANEL_NAME);
        state.info = PanelInfo::panel(serde_json::json!({
            "plugin_id": self.plugin_id,
            "command": self.command,
            "title": self.title,
            "view": self.view,
        }));
        state
    }
}

impl ComponentPanel for PluginDockPanel {
    /// The panel fills its host: no title row of its own.
    ///
    /// The tab group draws a title bar above a panel that is alone in its
    /// group unless the panel declines one, and a plugin view (the calendar
    /// above all) brings its own header - a second "Calendar" row above it is
    /// pure noise. The tabs come back on their own as soon as a second panel
    /// shares the group, which is exactly when switching is needed.
    fn title_bar(&self, _cx: &App) -> bool {
        false
    }

    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        Some(self.title.clone())
    }

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.title.clone()
    }
}

/// The workspace window's root view: a dock area with the plugin panel type
/// registered and its layout persisted.
pub struct PluginWorkspace {
    dock_area: Entity<DockArea>,
    /// Kept so the skin outlives the renderer it installed (the dock holder also
    /// keeps it alive; this handle is for future settings).
    _skin: Rc<DockSkin>,
    state: Rc<RefCell<LauncherState>>,
    _subscription: Subscription,
}

impl PluginWorkspace {
    fn new(state: Rc<RefCell<LauncherState>>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (dock_area, skin) = DockSkin::dock_area(
            "steward.plugin-workspace",
            Some(WORKSPACE_VERSION),
            window,
            cx,
        );

        // Panels rebuilt from the layout announce themselves in this log, so a
        // command the saved layout docked twice can be collapsed below.
        let restored = {
            let context = cx.global::<WorkspaceContext>();
            context.restored.borrow_mut().clear();
            context.restored.clone()
        };
        let stored = state
            .borrow()
            .storage
            .borrow()
            .get_setting(WORKSPACE_LAYOUT_KEY);
        if let Some(json) = stored {
            if let Ok(layout) = serde_json::from_str::<gpui_component::dock::DockAreaState>(&json) {
                dock_area.update(cx, |area, cx| {
                    let _ = area.load(layout, window, cx);
                });
            }
        }
        // A layout written while the restore forgot to register its panels can
        // hold one tab per visit (two "Calendar" tabs for one calendar). Keep
        // the first panel of each command and drop the rest, then let the
        // persisted layout be rewritten without them.
        let mut seen = HashSet::new();
        let duplicates: Vec<Entity<PluginDockPanel>> = restored
            .borrow_mut()
            .drain(..)
            .filter(|(key, _)| !seen.insert(key.clone()))
            .map(|(_, panel)| panel)
            .collect();
        let removed_duplicates = !duplicates.is_empty();
        for panel in duplicates {
            dock_area.update(cx, |area, cx| {
                area.remove_panel(panel.clone(), window, cx);
            });
        }

        let subscription = cx.subscribe(&dock_area, |this, _area, _event: &DockEvent, cx| {
            this.persist(cx);
        });

        let workspace = Self {
            dock_area,
            _skin: skin,
            state,
            _subscription: subscription,
        };
        if removed_duplicates {
            workspace.persist(cx);
        }
        workspace
    }

    fn persist(&self, cx: &mut Context<Self>) {
        let layout = self.dock_area.read(cx).dump(cx);
        if let Ok(json) = serde_json::to_string(&layout) {
            let _ = self
                .state
                .borrow()
                .storage
                .borrow()
                .set_setting(WORKSPACE_LAYOUT_KEY, &json);
        }
    }

    fn dock_area(&self) -> Entity<DockArea> {
        self.dock_area.clone()
    }
}

impl Render for PluginWorkspace {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.dock_area.clone())
    }
}

/// The `PanelRegistry` builder for [`PluginDockPanel`]: rebuild a panel from its
/// persisted `PanelInfo`.
fn build_restored_panel(
    context: PanelBuildContext,
    window: &mut Window,
    cx: &mut App,
) -> std::sync::Arc<dyn BasePanelView> {
    let info = match &context.state().info {
        PanelInfo::Panel(value) => value.clone(),
        _ => Value::Null,
    };
    let plugin_id = info["plugin_id"].as_str().unwrap_or_default().to_string();
    let command = info["command"].as_str().unwrap_or_default().to_string();
    let title: SharedString = info["title"]
        .as_str()
        .unwrap_or("Plugin")
        .to_string()
        .into();
    let view = info.get("view").cloned().unwrap_or(Value::Null);
    let (state, i18n) = workspace_context(cx);
    let panel = cx.new(|cx| {
        PluginDockPanel::new(
            plugin_id.clone(),
            command.clone(),
            title,
            view,
            i18n,
            state.clone(),
            window,
            cx,
        )
    });
    // Register the restored panel exactly like one this process opened:
    // `open_panel` looks the command up in this map, and a panel it cannot see
    // is a panel it docks a second copy of - one persisted tab plus one fresh
    // tab, two "Calendar" tabs in one group.
    state
        .borrow_mut()
        .workspace_panels
        .borrow_mut()
        .insert((plugin_id.clone(), command.clone()), panel.clone());
    // And log it for the workspace builder, which drops the extra copies a
    // layout written before this bookkeeping existed may still hold.
    cx.global::<WorkspaceContext>()
        .restored
        .borrow_mut()
        .push(((plugin_id, command), panel.clone()));
    std::sync::Arc::new(PanelHandle::new(panel))
}

/// Where the workspace keeps its handle on the shared launcher state and i18n.
/// The registry builder is registered once and cannot capture a per-app `Rc`,
/// so it reads this global instead.
struct WorkspaceContext {
    state: Rc<RefCell<LauncherState>>,
    i18n: Rc<Localization>,
    /// Panels the current window rebuilds from the persisted layout, in load
    /// order. Cleared when that window is created and drained right after the
    /// load, so the workspace can drop a command the layout docked twice.
    restored: RefCell<Vec<RestoredPanel>>,
}
impl gpui::Global for WorkspaceContext {}

fn workspace_context(cx: &App) -> (Rc<RefCell<LauncherState>>, Rc<Localization>) {
    let context = cx.global::<WorkspaceContext>();
    (context.state.clone(), context.i18n.clone())
}

/// Register the plugin panel type. Call once, during application init.
pub(crate) fn init(cx: &mut App, state: &Rc<RefCell<LauncherState>>, i18n: Rc<Localization>) {
    cx.set_global(WorkspaceContext {
        state: state.clone(),
        i18n,
        restored: RefCell::new(Vec::new()),
    });
    register_panel(cx, PANEL_NAME, build_restored_panel);
}

/// Open (or focus) the workspace window and return its handle.
fn ensure_workspace_window(state: &Rc<RefCell<LauncherState>>, cx: &mut App) -> AnyWindowHandle {
    if let Some(handle) = *state.borrow().workspace_window.borrow() {
        return handle;
    }
    let bounds = Bounds::centered(None, size(px(960.0), px(600.0)), cx);
    let state_for_window = state.clone();
    let mut workspace = None;
    let handle: AnyWindowHandle = cx
        .open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(480.0), px(320.0))),
                titlebar: Some(TitlebarOptions {
                    appears_transparent: false,
                    ..Default::default()
                }),
                show: true,
                focus: true,
                kind: WindowKind::Normal,
                is_resizable: true,
                is_minimizable: true,
                window_background: WindowBackgroundAppearance::Opaque,
                ..Default::default()
            },
            |window, cx| {
                // The product name, not a localized string: the launcher and
                // the standalone panel windows title themselves the same way.
                window.set_window_title("Steward");
                platform::force_dark_titlebar(window);
                window
                    .observe_window_appearance(|window, _cx| {
                        platform::force_dark_titlebar(window);
                    })
                    .detach();
                let entity = cx.new(|cx| PluginWorkspace::new(state_for_window, window, cx));
                workspace = Some(entity.clone());
                cx.new(|cx| Root::new(entity, window, cx))
            },
        )
        .expect("failed to open the plugin workspace window")
        .into();
    let dock = workspace
        .expect("the workspace window built its view")
        .read(cx)
        .dock_area();
    let state = state.borrow_mut();
    state.workspace_window.replace(Some(handle));
    state.workspace_dock.replace(Some(dock));
    handle
}

/// Add a plugin panel to the workspace (creating the window if needed). When
/// the same `(plugin_id, command)` is already docked, its view is replaced and
/// the workspace is brought forward.
#[allow(clippy::too_many_arguments)]
pub(crate) fn open_panel(
    state: &Rc<RefCell<LauncherState>>,
    plugin_id: String,
    command: String,
    title: SharedString,
    view: Value,
    cx: &mut App,
) -> Option<AnyWindowHandle> {
    let handle = ensure_workspace_window(state, cx);
    let i18n = workspace_context(cx).1;

    let existing = state
        .borrow()
        .workspace_panels
        .borrow()
        .get(&(plugin_id.clone(), command.clone()))
        .cloned();
    if let Some(panel) = existing {
        let view = view.clone();
        let _ = handle.update(cx, |_root, window, cx| {
            panel.update(cx, |panel, cx| panel.set_view(view.clone(), window, cx));
        });
        let _ = handle.update(cx, |_, window, cx| {
            cx.activate(true);
            window.refresh();
        });
        return Some(handle);
    }

    let dock = state.borrow().workspace_dock.borrow().clone()?;
    let state_for_panel = state.clone();
    let mut created = None;
    let _ = handle.update(cx, |_root, window, cx| {
        let panel = cx.new(|cx| {
            PluginDockPanel::new(
                plugin_id.clone(),
                command.clone(),
                title.clone(),
                view.clone(),
                i18n.clone(),
                state_for_panel.clone(),
                window,
                cx,
            )
        });
        dock.update(cx, |area, cx| {
            // Hand base this crate's presentation handle rather than the bare
            // entity: with a plain `add_panel` the skin cannot recover the
            // panel's presentation and draws its `panel_name`
            // ("steward.plugin") where the tab label and title belong.
            // `panel_handle` keeps the tab name and title the panel reports.
            area.add_panel_view(
                panel_handle(panel.clone()),
                DockPlacement::Center,
                None,
                window,
                cx,
            );
        });
        created = Some(panel);
        cx.activate(true);
        window.refresh();
    });
    if let Some(panel) = created {
        state
            .borrow_mut()
            .workspace_panels
            .borrow_mut()
            .insert((plugin_id, command), panel);
    }
    Some(handle)
}

/// Replace a docked panel's view (a `select` or element handler returned one).
pub(crate) fn replace_panel_view(
    state: &Rc<RefCell<LauncherState>>,
    plugin_id: &str,
    command: &str,
    view: &Value,
    cx: &mut App,
) {
    let panel = state
        .borrow()
        .workspace_panels
        .borrow()
        .get(&(plugin_id.to_string(), command.to_string()))
        .cloned();
    let Some(panel) = panel else {
        return;
    };
    let Some(handle) = *state.borrow().workspace_window.borrow() else {
        return;
    };
    let view = view.clone();
    let _ = handle.update(cx, |_root, window, cx| {
        panel.update(cx, |panel, cx| panel.set_view(view.clone(), window, cx));
    });
}

/// Remove a docked plugin panel (dock-back / close).
pub(crate) fn close_panel(
    state: &Rc<RefCell<LauncherState>>,
    plugin_id: &str,
    command: &str,
    cx: &mut App,
) {
    let panel = state
        .borrow_mut()
        .workspace_panels
        .borrow_mut()
        .remove(&(plugin_id.to_string(), command.to_string()));
    let Some(panel) = panel else {
        return;
    };
    let dock = state.borrow().workspace_dock.borrow().clone();
    let Some(handle) = *state.borrow().workspace_window.borrow() else {
        return;
    };
    let _ = handle.update(cx, |_root, window, cx| {
        if let Some(dock) = dock {
            dock.update(cx, |area, cx| {
                area.remove_panel(panel.clone(), window, cx);
            });
        }
    });
}

/// Whether a panel is currently docked in the workspace.
pub(crate) fn is_panel_open(state: &LauncherState, plugin_id: &str, command: &str) -> bool {
    state
        .workspace_panels
        .borrow()
        .contains_key(&(plugin_id.to_string(), command.to_string()))
}

/// Clear workspace bookkeeping when its window closes.
pub(crate) fn window_closed(state: &Rc<RefCell<LauncherState>>, window_id: gpui::WindowId) {
    let is_workspace = state
        .borrow()
        .workspace_window
        .borrow()
        .as_ref()
        .is_some_and(|handle| handle.window_id() == window_id);
    if is_workspace {
        let state = state.borrow_mut();
        state.workspace_window.replace(None);
        state.workspace_dock.replace(None);
        state.workspace_panels.borrow_mut().clear();
    }
}
