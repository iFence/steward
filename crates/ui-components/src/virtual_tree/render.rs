//! gpui materializer for a validated plugin UI tree.
//!
//! Validation ([`super::spec`]) is the security boundary: by the time we get
//! here every kind, style method, color and length is known-good, so the
//! renderer never has to reject anything. It still applies defaults before the
//! node's own styles, so a plugin can override every visual decision the host
//! would otherwise make.
//!
//! Every node is materialized through a `Div` wrapper: containers add their
//! flex/scroll defaults, leaves add their content, and the shared style surface
//! applies uniformly. Component leaves (button/input) are drawn by the host's
//! own components so the visual language stays consistent with the launcher.

use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    div, img, prelude::*, px, relative, rgb, AbsoluteLength, AnyElement, App, DefiniteLength,
    ElementId, FontWeight, Hsla, Image, ImageFormat, ImageSource, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, Styled, Window,
};
use gpui_component::ActiveTheme;

use super::input::InputStore;
use super::spec::{Axis, Color, EventKind, Kind, Length, Node, StyleValue};

/// Dispatch one element event: the node's callback id (if any), the node id,
/// the event kind and an optional value (input text).
pub type EventSink = Rc<dyn Fn(Option<&str>, &str, EventKind, Option<String>)>;

/// Everything the materializer needs from its host.
pub struct RenderEnv {
    /// Where element events are delivered.
    pub sink: EventSink,
    /// Host-owned input state for the current tree.
    pub inputs: InputStore,
}

/// Materialize a validated tree into a gpui element.
pub fn render(node: &Node, env: &RenderEnv, window: &mut Window, cx: &mut App) -> AnyElement {
    render_node(node, env, window, cx)
}

fn render_node(node: &Node, env: &RenderEnv, window: &mut Window, cx: &mut App) -> AnyElement {
    match node.kind {
        Kind::Div | Kind::Row | Kind::Col | Kind::Grid | Kind::Scroll => {
            render_container(node, env, window, cx)
        }
        Kind::Text => {
            let el = apply_styles(div(), node, cx);
            el.child(text_of(node)).into_any_element()
        }
        Kind::Button => render_button(node, env, cx),
        Kind::Link => render_link(node, env, cx),
        Kind::Badge => {
            let mut el = div()
                .px_2()
                .rounded_full()
                .bg(cx.theme().muted)
                .text_color(cx.theme().muted_foreground)
                .text_xs();
            el = apply_styles(el, node, cx);
            el.child(text_of(node)).into_any_element()
        }
        Kind::Separator => {
            let mut el = div().h(px(1.0)).w_full().bg(cx.theme().border);
            el = apply_styles(el, node, cx);
            el.into_any_element()
        }
        Kind::Spacer => {
            let el = apply_styles(div().flex_1(), node, cx);
            el.into_any_element()
        }
        Kind::Progress => {
            let fraction = node
                .progress
                .map(|p| p.value)
                .unwrap_or(0.0)
                .clamp(0.0, 1.0);
            let mut el = div()
                .h(px(6.0))
                .w_full()
                .rounded_full()
                .bg(cx.theme().muted);
            el = apply_styles(el, node, cx);
            el.child(
                div()
                    .h_full()
                    .rounded_full()
                    .bg(cx.theme().primary)
                    .w(relative(fraction)),
            )
            .into_any_element()
        }
        Kind::Icon => render_icon(node, cx),
        Kind::Image => render_image(node, cx),
        Kind::Input => {
            let el = apply_styles(div().w_full(), node, cx);
            match env.inputs.get(&node.id) {
                Some(input) => el.child(input.element()).into_any_element(),
                None => el.into_any_element(),
            }
        }
    }
}

fn text_of(node: &Node) -> String {
    node.text.clone().unwrap_or_default()
}

fn render_container(node: &Node, env: &RenderEnv, window: &mut Window, cx: &mut App) -> AnyElement {
    // Every container carries an id: scroll behaviour needs a stateful element
    // and a stable id also gives the scroll offset somewhere to live.
    let mut el = div().id(ElementId::from(node.id.clone()));
    el = match node.kind {
        Kind::Row => el.flex().flex_row(),
        Kind::Col => el.flex().flex_col(),
        Kind::Grid => el.flex().flex_row().flex_wrap().items_start(),
        Kind::Scroll => match node.scroll_axis {
            Axis::X => el.overflow_x_scroll(),
            Axis::Y => el.overflow_y_scroll(),
            Axis::Both => el.overflow_scroll(),
        },
        _ => el,
    };
    el = apply_styles(el, node, cx);
    for child in &node.children {
        let child_el = render_node(child, env, window, cx);
        el = match node.grid_columns {
            Some(columns) => el.child(
                div()
                    .flex_basis(relative(1.0 / columns as f32))
                    .min_w(px(0.0))
                    .child(child_el),
            ),
            None => el.child(child_el),
        };
    }
    el.into_any_element()
}

fn render_button(node: &Node, env: &RenderEnv, cx: &mut App) -> AnyElement {
    let mut el = div()
        .id(ElementId::from(node.id.clone()))
        .flex()
        .items_center()
        .justify_center()
        .px_3()
        .py_1()
        .rounded_md()
        .bg(cx.theme().primary)
        .text_color(cx.theme().primary_foreground)
        .text_sm();
    el = apply_styles(el, node, cx);
    el = attach_click(el, node, env);
    el.child(text_of(node)).into_any_element()
}

fn render_link(node: &Node, env: &RenderEnv, cx: &mut App) -> AnyElement {
    let mut el = div()
        .id(ElementId::from(node.id.clone()))
        .text_color(cx.theme().link)
        .underline();
    el = apply_styles(el, node, cx);
    el = attach_click(el, node, env);
    el.child(text_of(node)).into_any_element()
}

fn attach_click<T: Styled + StatefulInteractiveElement + IntoElement>(
    el: T,
    node: &Node,
    env: &RenderEnv,
) -> T {
    let Some(callback) = node.callback(EventKind::Click).map(str::to_string) else {
        return el;
    };
    let sink = env.sink.clone();
    let node_id = node.id.clone();
    el.cursor_pointer().on_click(move |_, _, _| {
        sink(Some(&callback), &node_id, EventKind::Click, None);
    })
}

fn render_icon(node: &Node, cx: &mut App) -> AnyElement {
    let Some(svg) = node.icon_svg.as_ref() else {
        return div().into_any_element();
    };
    let image = Arc::new(Image::from_bytes(
        ImageFormat::Svg,
        svg.clone().into_bytes(),
    ));
    let mut el = div().size(px(16.0)).flex_shrink_0();
    el = apply_styles(el, node, cx);
    el.child(img(ImageSource::Image(image))).into_any_element()
}

fn render_image(node: &Node, cx: &mut App) -> AnyElement {
    let mut el = div();
    el = apply_styles(el, node, cx);
    let Some(data) = node.image_data.as_ref() else {
        return el.into_any_element();
    };
    match decode_svg_data_uri(data) {
        Some(bytes) => {
            let image = Arc::new(Image::from_bytes(ImageFormat::Svg, bytes));
            el.child(img(ImageSource::Image(image))).into_any_element()
        }
        None => el.into_any_element(),
    }
}

/// Decode a `data:image/svg+xml` URI (base64 or percent/plain). Other image
/// formats are not materialized in v1.
fn decode_svg_data_uri(data: &str) -> Option<Vec<u8>> {
    let rest = data.strip_prefix("data:")?;
    let (meta, payload) = rest.split_once(',')?;
    if !meta.starts_with("image/svg+xml") {
        return None;
    }
    if meta.ends_with(";base64") {
        base64_decode(payload)
    } else {
        Some(payload.as_bytes().to_vec())
    }
}

fn base64_decode(input: &str) -> Option<Vec<u8>> {
    const TABLE: [i8; 256] = {
        let mut table = [-1i8; 256];
        let mut i = 0u8;
        while i < 64 {
            table[b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"[i as usize]
                as usize] = i as i8;
            i += 1;
        }
        table
    };
    let mut out = Vec::with_capacity(input.len() / 4 * 3);
    let mut buffer = 0u32;
    let mut bits = 0u32;
    for byte in input.bytes() {
        if byte == b'=' {
            break;
        }
        let value = TABLE[byte as usize];
        if value < 0 {
            continue;
        }
        buffer = (buffer << 6) | value as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// Style surface
// ---------------------------------------------------------------------------

fn apply_styles<T: Styled>(mut el: T, node: &Node, cx: &App) -> T {
    for (name, value) in &node.style {
        el = apply_style(el, name, value, cx);
    }
    el
}

/// Apply one style method. Unknown names are a no-op here — validation already
/// rejected them, and the `style_table_matches_renderer_arms` test fails if the
/// table and the arms below drift apart.
fn apply_style<T: Styled>(mut el: T, name: &str, value: &StyleValue, cx: &App) -> T {
    let color = || resolve_color(value, cx);
    el = match name {
        // Flex and alignment flags.
        "flex" => el.flex(),
        "flex_1" => el.flex_1(),
        "flex_auto" => el.flex_auto(),
        "flex_none" => el.flex_none(),
        "flex_col" => el.flex_col(),
        "flex_row" => el.flex_row(),
        "flex_col_reverse" => el.flex_col_reverse(),
        "flex_row_reverse" => el.flex_row_reverse(),
        "flex_wrap" => el.flex_wrap(),
        "flex_wrap_reverse" => el.flex_wrap_reverse(),
        "flex_nowrap" => el.flex_nowrap(),
        "flex_grow_0" => el.flex_grow_0(),
        "flex_grow_1" => el.flex_grow_1(),
        "flex_shrink_0" => el.flex_shrink_0(),
        "flex_shrink_1" => el.flex_shrink_1(),
        "items_start" => el.items_start(),
        "items_center" => el.items_center(),
        "items_end" => el.items_end(),
        "items_baseline" => el.items_baseline(),
        "items_stretch" => el.items_stretch(),
        "justify_start" => el.justify_start(),
        "justify_center" => el.justify_center(),
        "justify_end" => el.justify_end(),
        "justify_between" => el.justify_between(),
        "justify_around" => el.justify_around(),
        "justify_evenly" => el.justify_evenly(),
        "self_start" => el.self_start(),
        "self_center" => el.self_center(),
        "self_end" => el.self_end(),
        "self_stretch" => el.self_stretch(),
        "self_baseline" => el.self_baseline(),
        // Position, visibility, overflow, sizing.
        "relative" => el.relative(),
        "absolute" => el.absolute(),
        "hidden" => el.hidden(),
        "block" => el.block(),
        "visible" => el.visible(),
        "invisible" => el.invisible(),
        "overflow_hidden" => el.overflow_hidden(),
        "overflow_x_hidden" => el.overflow_x_hidden(),
        "overflow_y_hidden" => el.overflow_y_hidden(),
        "size_full" => el.size_full(),
        "w_full" => el.w_full(),
        "h_full" => el.h_full(),
        "w_auto" => el.w_auto(),
        "h_auto" => el.h_auto(),
        "size_auto" => el.size_auto(),
        "aspect_square" => el.aspect_square(),
        // Rounded / border flag variants.
        "rounded_none" => el.rounded_none(),
        "rounded_xs" => el.rounded_xs(),
        "rounded_sm" => el.rounded_sm(),
        "rounded_md" => el.rounded_md(),
        "rounded_lg" => el.rounded_lg(),
        "rounded_xl" => el.rounded_xl(),
        "rounded_full" => el.rounded_full(),
        // Text.
        "text_xs" => el.text_xs(),
        "text_sm" => el.text_sm(),
        "text_base" => el.text_base(),
        "text_lg" => el.text_lg(),
        "text_xl" => el.text_xl(),
        "text_2xl" => el.text_2xl(),
        "text_3xl" => el.text_3xl(),
        "text_left" => el.text_left(),
        "text_center" => el.text_center(),
        "text_right" => el.text_right(),
        "truncate" => el.truncate(),
        "text_ellipsis" => el.text_ellipsis(),
        "italic" => el.italic(),
        "not_italic" => el.not_italic(),
        "underline" => el.underline(),
        "line_through" => el.line_through(),
        "whitespace_nowrap" => el.whitespace_nowrap(),
        "whitespace_normal" => el.whitespace_normal(),
        "cursor_pointer" => el.cursor_pointer(),
        "cursor_text" => el.cursor_text(),
        "cursor_default" => el.cursor_default(),
        // Length-valued methods.
        "w" => el.w(definite(value)),
        "h" => el.h(definite(value)),
        "size" => el.size(definite(value)),
        "min_w" => el.min_w(definite(value)),
        "min_h" => el.min_h(definite(value)),
        "min_size" => el.min_size(definite(value)),
        "max_w" => el.max_w(definite(value)),
        "max_h" => el.max_h(definite(value)),
        "max_size" => el.max_size(definite(value)),
        "m" => el.m(definite(value)),
        "mx" => el.mx(definite(value)),
        "my" => el.my(definite(value)),
        "mt" => el.mt(definite(value)),
        "mb" => el.mb(definite(value)),
        "ml" => el.ml(definite(value)),
        "mr" => el.mr(definite(value)),
        "p" => el.p(definite(value)),
        "px" => el.px(definite(value)),
        "py" => el.py(definite(value)),
        "pt" => el.pt(definite(value)),
        "pb" => el.pb(definite(value)),
        "pl" => el.pl(definite(value)),
        "pr" => el.pr(definite(value)),
        "inset" => el.inset(definite(value)),
        "top" => el.top(definite(value)),
        "bottom" => el.bottom(definite(value)),
        "left" => el.left(definite(value)),
        "right" => el.right(definite(value)),
        "gap" => el.gap(definite(value)),
        "gap_x" => el.gap_x(definite(value)),
        "gap_y" => el.gap_y(definite(value)),
        "flex_basis" => el.flex_basis(definite(value)),
        "line_height" => el.line_height(definite(value)),
        // Absolute-length methods.
        "border" => el.border(absolute(value)),
        "border_t" => el.border_t(absolute(value)),
        "border_b" => el.border_b(absolute(value)),
        "border_l" => el.border_l(absolute(value)),
        "border_r" => el.border_r(absolute(value)),
        "border_x" => el.border_x(absolute(value)),
        "border_y" => el.border_y(absolute(value)),
        "rounded" => el.rounded(absolute(value)),
        "rounded_t" => el.rounded_t(absolute(value)),
        "rounded_b" => el.rounded_b(absolute(value)),
        "rounded_l" => el.rounded_l(absolute(value)),
        "rounded_r" => el.rounded_r(absolute(value)),
        "rounded_tl" => el.rounded_tl(absolute(value)),
        "rounded_tr" => el.rounded_tr(absolute(value)),
        "rounded_bl" => el.rounded_bl(absolute(value)),
        "rounded_br" => el.rounded_br(absolute(value)),
        "text_size" => el.text_size(absolute(value)),
        // Numbers.
        "opacity" => el.opacity(number(value)),
        "flex_grow" => el.flex_grow(number(value)),
        "flex_shrink" => el.flex_shrink(number(value)),
        "aspect_ratio" => el.aspect_ratio(number(value)),
        "line_clamp" => el.line_clamp(number(value).max(0.0) as usize),
        // Colors and strings.
        "bg" => el.bg(color()),
        "text_color" => el.text_color(color()),
        "border_color" => el.border_color(color()),
        "text_bg" => el.text_bg(color()),
        "font_family" => el.font_family(SharedString::from(text(value))),
        "font_weight" => el.font_weight(weight(&text(value))),
        _ => el,
    };
    el
}

fn definite(value: &StyleValue) -> DefiniteLength {
    match value {
        StyleValue::Length(Length::Px(value)) => px(*value).into(),
        StyleValue::Length(Length::Percent(value)) => relative(*value / 100.0),
        _ => px(0.0).into(),
    }
}

fn absolute(value: &StyleValue) -> AbsoluteLength {
    match value {
        StyleValue::Length(Length::Px(value)) => px(*value).into(),
        _ => px(0.0).into(),
    }
}

fn number(value: &StyleValue) -> f32 {
    match value {
        StyleValue::Number(value) => *value,
        _ => 0.0,
    }
}

fn text(value: &StyleValue) -> String {
    match value {
        StyleValue::Text(value) | StyleValue::Weight(value) => value.clone(),
        _ => String::new(),
    }
}

fn resolve_color(value: &StyleValue, cx: &App) -> Hsla {
    let color = match value {
        StyleValue::Color(color) => color,
        _ => return cx.theme().foreground,
    };
    match color {
        Color::Hex(hex) => rgb(*hex).into(),
        Color::Token(name) => {
            let colors = &cx.theme().colors;
            match name.as_str() {
                "background" => colors.background,
                "foreground" => colors.foreground,
                "surface" => colors.popover,
                "surface_foreground" => colors.popover_foreground,
                "primary" => colors.primary,
                "primary_foreground" => colors.primary_foreground,
                "primary_hover" => colors.primary_hover,
                "primary_active" => colors.primary_active,
                "secondary" => colors.secondary,
                "secondary_foreground" => colors.secondary_foreground,
                "muted" => colors.muted,
                "muted_foreground" => colors.muted_foreground,
                "accent" => colors.accent,
                "accent_foreground" => colors.accent_foreground,
                "danger" => colors.danger,
                "danger_foreground" => colors.danger_foreground,
                "success" => colors.success,
                "success_foreground" => colors.success_foreground,
                "info" => colors.info,
                "info_foreground" => colors.info_foreground,
                "border" => colors.border,
                "input" => colors.input,
                "ring" => colors.ring,
                "popover" => colors.popover,
                "popover_foreground" => colors.popover_foreground,
                "list" => colors.list,
                "list_hover" => colors.list_hover,
                "list_active" => colors.list_active,
                "link" => colors.link,
                "selection" => colors.selection,
                "caret" => colors.caret,
                "scrollbar" => colors.scrollbar,
                "scrollbar_thumb" => colors.scrollbar_thumb,
                "skeleton" => colors.skeleton,
                "title_bar" => colors.title_bar,
                "tab" => colors.tab,
                "tab_active" => colors.tab_active,
                "tab_foreground" => colors.tab_foreground,
                "table" => colors.table,
                "table_head" => colors.table_head,
                "table_head_foreground" => colors.table_head_foreground,
                "table_row_border" => colors.table_row_border,
                _ => colors.foreground,
            }
        }
    }
}

fn weight(name: &str) -> FontWeight {
    match name {
        "thin" => FontWeight::THIN,
        "extra_light" => FontWeight::EXTRA_LIGHT,
        "light" => FontWeight::LIGHT,
        "medium" => FontWeight::MEDIUM,
        "semibold" => FontWeight::SEMIBOLD,
        "bold" => FontWeight::BOLD,
        "extra_bold" => FontWeight::EXTRA_BOLD,
        "black" => FontWeight::BLACK,
        _ => FontWeight::NORMAL,
    }
}

#[cfg(test)]
mod tests {
    use crate::virtual_tree::spec::style_methods;

    /// Every style method the table declares must have a renderer arm, or the
    /// table and the materializer have drifted. `apply_style` cannot be called
    /// without an `App` (colors read the theme), so this reads the source the
    /// same way gpui-component's dock re-export test does: a name in the table
    /// with no `"name" =>` arm in `render.rs` fails the build.
    #[test]
    fn style_table_matches_renderer_arms() {
        let source = include_str!("render.rs");
        let missing: Vec<&str> = style_methods()
            .iter()
            .filter(|method| !source.contains(&format!("\"{}\" =>", method.name)))
            .map(|method| method.name.as_str())
            .collect();
        assert!(
            missing.is_empty(),
            "renderer is missing arms for {missing:?}"
        );
    }
}
