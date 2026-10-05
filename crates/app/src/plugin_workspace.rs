//! Plugin workspace: a dockable host for plugin views.
//!
//! The workspace window owns a gpui-component [`DockArea`]; each plugin view
//! becomes a [`PluginDockPanel`] that can be tabbed, dragged, split and closed.
//! Element events still travel to the plugin as `view.invoke` exactly as they
//! do for the launcher's inline rendering, so a docked view is the same tree,
//! just hosted in a window the user can rearrange.
//!
//! Layout is persisted through the storage settings table, and panels are
//! rebuilt on the next launch through the dock's `PanelRegistry`.

use std::{cell::RefCell, rc::Rc};

use gpui::{
    div, prelude::*, px, size, AnyWindowHandle, App, Bounds, Context, Entity, EventEmitter,
    FocusHandle, Focusable, IntoElement, Render, SharedString, Subscription, TitlebarOptions,
    Window, WindowBackgroundAppearance, WindowBounds, WindowKind, WindowOptions,
};
use gpui_component::dock::{
    register_panel, BasePanelView, DockArea, DockEvent, DockPlacement, DockSkin,
    Panel as ComponentPanel, PanelBuildContext, PanelEvent, PanelHandle, PanelInfo, PanelState,
};
use gpui_component::Root;
use serde_json::Value;
use steward_ui_components::virtual_tree::{validate_view, VirtualTreeView};

use crate::i18n::Localization;
use crate::launcher::LauncherState;
use crate::platform;

/// Storage key holding the serialized `DockAreaState`.
const WORKSPACE_LAYOUT_KEY: &str = "plugin_workspace_layout";
/// `panel_name` under which every plugin panel is registered.
const PANEL_NAME: &str = "steward.plugin";
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

/// One plugin view hosted in the workspace dock.
pub struct PluginDockPanel {
    plugin_id: String,
    command: String,
    title: SharedString,
    /// The raw view, persisted so a panel can be rebuilt after a restart.
    view: Value,
    tree: Entity<VirtualTreeView>,
    focus: FocusHandle,
}

impl PluginDockPanel {
    pub(crate) fn new(
        plugin_id: String,
        command: String,
        title: SharedString,
        view: Value,
        state: Rc<RefCell<LauncherState>>,
        cx: &mut Context<Self>,
    ) -> Self {
        let sink = workspace_event_sink(state.clone(), plugin_id.clone(), command.clone());
        // A tree that fails validation still yields a panel; it just renders an
        // empty body. The launcher validates before opening, so this is a
        // belt-and-braces path for a restored layout.
        let node = validate_view(&view).ok();
        let tree = cx.new(|cx| match node {
            Some(node) => VirtualTreeView::new(node, sink, cx),
            None => {
                let empty =
                    steward_ui_components::virtual_tree::validate_view(&serde_json::json!({
                        "type": "ui",
                        "root": { "kind": "col" }
                    }))
                    .expect("the empty view is valid");
                VirtualTreeView::new(empty, sink, cx)
            }
        });
        Self {
            plugin_id,
            command,
            title,
            view,
            tree,
            focus: cx.focus_handle(),
        }
    }

    /// Replace the displayed tree (an element handler returned a new one).
    pub(crate) fn set_view(&mut self, view: Value, cx: &mut Context<Self>) {
        let Ok(node) = validate_view(&view) else {
            return;
        };
        self.view = view;
        let tree = self.tree.clone();
        tree.update(cx, |tree, cx| tree.set_tree(node, cx));
    }
}

impl EventEmitter<PanelEvent> for PluginDockPanel {}

impl Focusable for PluginDockPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for PluginDockPanel {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().p(px(8.0)).child(self.tree.clone())
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
    pub(crate) fn new(
        state: Rc<RefCell<LauncherState>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (dock_area, skin) = DockSkin::dock_area(
            "steward.plugin-workspace",
            Some(WORKSPACE_VERSION),
            window,
            cx,
        );

        // Restore the previous layout, if the stored schema still parses.
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

        let subscription = cx.subscribe(&dock_area, |this, _area, _event: &DockEvent, cx| {
            this.persist(cx);
        });

        Self {
            dock_area,
            _skin: skin,
            state,
            _subscription: subscription,
        }
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
    _window: &mut Window,
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
    let state = workspace_state(cx);
    let panel = cx.new(|cx| PluginDockPanel::new(plugin_id, command, title, view, state, cx));
    std::sync::Arc::new(PanelHandle::new(panel))
}

/// Where the workspace keeps its handle on the shared launcher state. The
/// registry builder is registered once and cannot capture a per-app `Rc`, so it
/// reads this global instead.
struct WorkspaceContext(Rc<RefCell<LauncherState>>);
impl gpui::Global for WorkspaceContext {}

fn workspace_state(cx: &App) -> Rc<RefCell<LauncherState>> {
    cx.global::<WorkspaceContext>().0.clone()
}

/// Register the plugin panel type. Call once, during application init.
pub(crate) fn init(cx: &mut App, state: &Rc<RefCell<LauncherState>>) {
    cx.set_global(WorkspaceContext(state.clone()));
    // Re-registering would replace the factory, which is harmless, but the
    // registry is per-app and init runs once.
    register_panel(cx, PANEL_NAME, build_restored_panel);
}

/// Open (or focus) the workspace window and return its handle.
pub(crate) fn ensure_workspace_window(
    state: &Rc<RefCell<LauncherState>>,
    i18n: Rc<Localization>,
    cx: &mut App,
) -> AnyWindowHandle {
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
                window.set_window_title(&i18n.translate("app-name"));
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
        .dock_area
        .clone();
    let state = state.borrow_mut();
    state.workspace_window.replace(Some(handle));
    state.workspace_dock.replace(Some(dock));
    handle
}

/// Add a plugin panel to the workspace (creating the window if needed). When
/// the same `(plugin_id, command)` is already docked, its tree is replaced and
/// its window is brought forward.
pub(crate) fn open_ui_panel(
    state: &Rc<RefCell<LauncherState>>,
    i18n: Rc<Localization>,
    plugin_id: String,
    command: String,
    title: SharedString,
    view: Value,
    cx: &mut App,
) -> Option<AnyWindowHandle> {
    let handle = ensure_workspace_window(state, i18n, cx);

    let existing = state
        .borrow()
        .workspace_panels
        .borrow()
        .get(&(plugin_id.clone(), command.clone()))
        .cloned();
    if let Some(panel) = existing {
        panel.update(cx, |panel, cx| panel.set_view(view, cx));
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
                state_for_panel.clone(),
                cx,
            )
        });
        dock.update(cx, |area, cx| {
            area.add_panel(panel.clone(), DockPlacement::Center, None, window, cx);
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

/// Remove a docked plugin panel (dock-back / close).
pub(crate) fn close_ui_panel(
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
