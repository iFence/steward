//! A reusable entity that renders one plugin `ui` tree.
//!
//! Both the launcher's drop-down and a detached plugin panel hold one of these
//! and call [`VirtualTreeView::set_tree`] when a new tree arrives. Keeping the
//! tree and the host-owned input state together means a replacement tree reuses
//! the same inputs (typed text survives a redraw) and only the nodes that
//! actually changed pay for a re-render.

use gpui::{Context, IntoElement, Render, Window};

use super::input::InputStore;
use super::render::{render, EventSink, RenderEnv};
use super::spec::Node;

/// A rendered, validated plugin UI tree.
pub struct VirtualTreeView {
    tree: Node,
    env: RenderEnv,
}

impl VirtualTreeView {
    /// Build the view and materialize its host-owned inputs.
    pub fn new(tree: Node, sink: EventSink, cx: &mut Context<Self>) -> Self {
        let inputs = InputStore::default();
        inputs.sync(&tree, &sink, cx);
        Self {
            tree,
            env: RenderEnv { sink, inputs },
        }
    }

    /// Replace the tree (e.g. after a `view.invoke` returned a new one),
    /// preserving the text of inputs whose element id is unchanged.
    pub fn set_tree(&mut self, tree: Node, cx: &mut Context<Self>) {
        self.env.inputs.sync(&tree, &self.env.sink, cx);
        self.tree = tree;
        cx.notify();
    }

    /// The currently displayed tree.
    pub fn tree(&self) -> &Node {
        &self.tree
    }
}

impl Render for VirtualTreeView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        render(&self.tree, &self.env, window, cx)
    }
}
