//! Declarative plugin UI tree: the wire shape and its validator.
//!
//! A plugin returns `{ "type": "ui", "root": <node> }`; the host validates the
//! whole tree before rendering it, because the payload is untrusted. Everything
//! here is pure data: no gpui dependency, so it is unit-testable on its own and
//! usable from tooling (the CLI's future `check`) without a window.
//!
//! The style method vocabulary lives in `style_table.json`, which is the single
//! source shared with the TypeScript SDK (`@steward/extension-api`). Adding a
//! method means adding it there and handling it in [`super::render`].

use std::{collections::HashMap, sync::OnceLock};

use serde::Deserialize;
use serde_json::Value;

/// Maximum nesting depth of a rendered tree.
pub const MAX_DEPTH: usize = 32;
/// Maximum number of nodes in one tree.
pub const MAX_NODES: usize = 2000;
/// Maximum number of direct children of one container.
pub const MAX_CHILDREN: usize = 256;
/// Maximum bytes of one text/label string.
pub const MAX_TEXT_BYTES: usize = 8 * 1024;
/// Maximum bytes of the serialized tree.
pub const MAX_TREE_BYTES: usize = 1024 * 1024;
/// Maximum number of style entries on one node.
pub const MAX_STYLE_ENTRIES: usize = 64;
/// Maximum length of an element or callback id.
pub const MAX_ID_LEN: usize = 128;
/// Maximum length of a free-form string prop (font family, image data, ...).
pub const MAX_STRING_BYTES: usize = 256 * 1024;
/// Largest length value accepted for a style method, in pixels.
pub const MAX_LENGTH_PX: f32 = 4096.0;

static COLOR_TOKENS: OnceLock<Vec<String>> = OnceLock::new();

/// Theme color tokens a plugin may name. Kept in sync with the renderer's
/// resolver and mirrored in the SDK's declared union; the canonical list is
/// `color_tokens.json`, shared with the TypeScript generator.
pub fn color_tokens() -> &'static [String] {
    COLOR_TOKENS.get_or_init(|| {
        serde_json::from_str(include_str!("color_tokens.json"))
            .expect("color_tokens.json is valid and is checked in")
    })
}

/// A UI tree validation failure, with a human-readable message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiError(pub String);

impl std::fmt::Display for UiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for UiError {}

fn err(message: impl Into<String>) -> UiError {
    UiError(message.into())
}

/// The kind of element a node describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Div,
    Row,
    Col,
    Grid,
    Scroll,
    Text,
    Icon,
    Image,
    Button,
    Input,
    Link,
    Badge,
    Separator,
    Progress,
    Spacer,
}

impl Kind {
    fn from_wire(value: &str) -> Option<Self> {
        Some(match value {
            "div" => Self::Div,
            "row" => Self::Row,
            "col" => Self::Col,
            "grid" => Self::Grid,
            "scroll" => Self::Scroll,
            "text" => Self::Text,
            "icon" => Self::Icon,
            "image" => Self::Image,
            "button" => Self::Button,
            "input" => Self::Input,
            "link" => Self::Link,
            "badge" => Self::Badge,
            "separator" => Self::Separator,
            "progress" => Self::Progress,
            "spacer" => Self::Spacer,
            _ => return None,
        })
    }

    /// Whether the kind accepts `children`.
    pub fn is_container(self) -> bool {
        matches!(
            self,
            Self::Div | Self::Row | Self::Col | Self::Grid | Self::Scroll
        )
    }

    /// Whether the kind accepts `text`.
    pub fn accepts_text(self) -> bool {
        matches!(self, Self::Text | Self::Button | Self::Link | Self::Badge)
    }
}

/// How a style method's argument is encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgKind {
    /// No argument; presence means `true`.
    None,
    /// A definite length: pixels or a percentage (`"50%"`).
    Length,
    /// An absolute length: pixels only.
    Absolute,
    /// A bare number.
    Number,
    /// A theme token name or `#rrggbb`.
    Color,
    /// A short free-form string.
    String,
    /// A font weight name.
    Weight,
}

/// One entry of the canonical style vocabulary.
#[derive(Debug, Clone, Deserialize)]
pub struct StyleMethod {
    pub name: String,
    pub arg: ArgKind,
}

impl<'de> Deserialize<'de> for ArgKind {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        match value.as_str() {
            "none" => Ok(Self::None),
            "length" => Ok(Self::Length),
            "absolute" => Ok(Self::Absolute),
            "number" => Ok(Self::Number),
            "color" => Ok(Self::Color),
            "string" => Ok(Self::String),
            "weight" => Ok(Self::Weight),
            other => Err(serde::de::Error::custom(format!(
                "unknown style arg kind '{other}'"
            ))),
        }
    }
}

static STYLE_METHODS: OnceLock<Vec<StyleMethod>> = OnceLock::new();
static STYLE_INDEX: OnceLock<HashMap<&'static str, ArgKind>> = OnceLock::new();

/// The canonical style vocabulary, parsed once from `style_table.json`.
pub fn style_methods() -> &'static [StyleMethod] {
    STYLE_METHODS.get_or_init(|| {
        serde_json::from_str(include_str!("style_table.json"))
            .expect("style_table.json is valid and is checked in")
    })
}

/// Look up a style method's argument kind.
pub fn style_arg_kind(name: &str) -> Option<ArgKind> {
    STYLE_INDEX
        .get_or_init(|| {
            style_methods()
                .iter()
                .map(|method| {
                    // The table is stored in a `static`, so borrowing its
                    // strings for the index is sound and keeps lookups cheap.
                    let name: &'static str = Box::leak(method.name.clone().into_boxed_str());
                    (name, method.arg)
                })
                .collect()
        })
        .get(name)
        .copied()
}

/// A definite or absolute length.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Length {
    Px(f32),
    Percent(f32),
}

impl Length {
    /// The pixel value, if this is an absolute length.
    pub fn px(self) -> Option<f32> {
        match self {
            Self::Px(value) => Some(value),
            Self::Percent(_) => None,
        }
    }

    /// The fraction (0.0..=1.0) for a percentage, if this is a percentage.
    pub fn fraction(self) -> Option<f32> {
        match self {
            Self::Percent(value) => Some(value / 100.0),
            Self::Px(_) => None,
        }
    }
}

/// A resolved color: a theme token name or a literal `#rrggbb`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Color {
    Token(String),
    Hex(u32),
}

/// A validated style value.
#[derive(Debug, Clone, PartialEq)]
pub enum StyleValue {
    Flag,
    Number(f32),
    Length(Length),
    Color(Color),
    Text(String),
    Weight(String),
}

/// A supported element event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    Click,
    Change,
    Submit,
}

impl EventKind {
    /// The wire name (also used in the `view.invoke` event payload).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Click => "click",
            Self::Change => "change",
            Self::Submit => "submit",
        }
    }
}

/// The scroll axis of a `scroll` container.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Axis {
    X,
    #[default]
    Y,
    Both,
}

/// Host-owned input props.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InputProps {
    pub placeholder: String,
    pub value: String,
    pub multiline: bool,
    pub password: bool,
}

/// The props of a `progress` leaf.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProgressProps {
    pub value: f32,
    pub indeterminate: bool,
}

/// One validated element.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub kind: Kind,
    /// Explicit id, or the path-derived id assigned during validation.
    pub id: String,
    pub style: Vec<(String, StyleValue)>,
    pub text: Option<String>,
    pub children: Vec<Node>,
    pub events: Vec<(EventKind, String)>,
    pub grid_columns: Option<usize>,
    pub scroll_axis: Axis,
    pub input: Option<InputProps>,
    pub icon_svg: Option<String>,
    pub image_data: Option<String>,
    pub progress: Option<ProgressProps>,
}

impl Node {
    /// The callback id registered for `event`, if the node declares one.
    pub fn callback(&self, event: EventKind) -> Option<&str> {
        self.events
            .iter()
            .find(|(kind, _)| *kind == event)
            .map(|(_, id)| id.as_str())
    }
}

/// Validate a plugin view payload into an owned tree.
///
/// Accepts the runtime's `{ "view": ... }` envelope as well as a bare view.
pub fn validate_view(view: &Value) -> Result<Node, UiError> {
    let view = view.get("view").unwrap_or(view);
    let kind = view.get("type").and_then(Value::as_str).unwrap_or("");
    if kind != "ui" {
        return Err(err(format!(
            "expected a 'ui' view, found '{}'",
            if kind.is_empty() { "<none>" } else { kind }
        )));
    }
    let root = view
        .get("root")
        .ok_or_else(|| err("'ui' view is missing its 'root' element"))?;

    let bytes = serde_json::to_vec(view)
        .map_err(|error| err(format!("cannot measure the view: {error}")))?
        .len();
    if bytes > MAX_TREE_BYTES {
        return Err(err(format!(
            "view is {bytes} bytes; the limit is {MAX_TREE_BYTES}"
        )));
    }

    let mut state = ValidateState { nodes: 0 };
    let node = state.node(root, "r", 0)?;
    Ok(node)
}

struct ValidateState {
    nodes: usize,
}

/// The kind-specific props collected while validating one element.
#[derive(Default)]
struct KindProps {
    grid_columns: Option<usize>,
    scroll_axis: Axis,
    input: Option<InputProps>,
    icon_svg: Option<String>,
    image_data: Option<String>,
    progress: Option<ProgressProps>,
}

impl ValidateState {
    fn node(&mut self, value: &Value, path: &str, depth: usize) -> Result<Node, UiError> {
        if depth > MAX_DEPTH {
            return Err(err(format!(
                "element depth exceeds the limit of {MAX_DEPTH}"
            )));
        }
        self.nodes += 1;
        if self.nodes > MAX_NODES {
            return Err(err(format!(
                "element count exceeds the limit of {MAX_NODES}"
            )));
        }
        let object = value
            .as_object()
            .ok_or_else(|| err(format!("element at {path} must be an object")))?;

        let kind_name = object
            .get("kind")
            .and_then(Value::as_str)
            .ok_or_else(|| err(format!("element at {path} is missing its 'kind'")))?;
        let kind = Kind::from_wire(kind_name)
            .ok_or_else(|| err(format!("unknown element kind '{kind_name}' at {path}")))?;

        let id = match object.get("id") {
            None | Some(Value::Null) => path.to_string(),
            Some(value) => {
                let id = value
                    .as_str()
                    .ok_or_else(|| err(format!("element at {path}: 'id' must be a string")))?;
                validate_id(id, path)?;
                id.to_string()
            }
        };

        let style = self.style(object.get("style"), path)?;

        let text = match object.get("text") {
            None | Some(Value::Null) => None,
            Some(value) => {
                if !kind.accepts_text() {
                    return Err(err(format!(
                        "element at {path} ('{kind_name}') does not accept 'text'"
                    )));
                }
                let text = value
                    .as_str()
                    .ok_or_else(|| err(format!("element at {path}: 'text' must be a string")))?;
                if text.len() > MAX_TEXT_BYTES {
                    return Err(err(format!(
                        "element at {path}: text is {} bytes; the limit is {MAX_TEXT_BYTES}",
                        text.len()
                    )));
                }
                Some(text.to_string())
            }
        };

        let events = self.events(object.get("on"), path)?;

        let children = match object.get("children") {
            None | Some(Value::Null) => Vec::new(),
            Some(value) => {
                if !kind.is_container() {
                    return Err(err(format!(
                        "element at {path} ('{kind_name}') does not accept 'children'"
                    )));
                }
                let array = value.as_array().ok_or_else(|| {
                    err(format!("element at {path}: 'children' must be an array"))
                })?;
                if array.len() > MAX_CHILDREN {
                    return Err(err(format!(
                        "element at {path} has {} children; the limit is {MAX_CHILDREN}",
                        array.len()
                    )));
                }
                let mut children = Vec::with_capacity(array.len());
                for (index, child) in array.iter().enumerate() {
                    let child_path = format!("{path}/{index}");
                    children.push(self.node(child, &child_path, depth + 1)?);
                }
                children
            }
        };

        // Kind-specific props.
        let props = self.props(kind, object.get("props"), path)?;

        if kind == Kind::Input && object.get("id").is_none() {
            return Err(err(format!(
                "element at {path}: an 'input' requires an explicit 'id' to key its host-owned state"
            )));
        }

        Ok(Node {
            kind,
            id,
            style,
            text,
            children,
            events,
            grid_columns: props.grid_columns,
            scroll_axis: props.scroll_axis,
            input: props.input,
            icon_svg: props.icon_svg,
            image_data: props.image_data,
            progress: props.progress,
        })
    }

    fn props(&self, kind: Kind, props: Option<&Value>, path: &str) -> Result<KindProps, UiError> {
        let object = match props {
            None | Some(Value::Null) => None,
            Some(value) => Some(
                value
                    .as_object()
                    .ok_or_else(|| err(format!("element at {path}: 'props' must be an object")))?,
            ),
        };
        let get = |key: &str| object.and_then(|object| object.get(key));

        let mut out = KindProps::default();

        match kind {
            Kind::Grid => {
                let columns = match get("columns") {
                    None | Some(Value::Null) => 4,
                    Some(value) => value.as_u64().ok_or_else(|| {
                        err(format!(
                            "element at {path}: 'grid.columns' must be a number"
                        ))
                    })? as usize,
                };
                if !(1..=16).contains(&columns) {
                    return Err(err(format!(
                        "element at {path}: 'grid.columns' must be 1..=16"
                    )));
                }
                out.grid_columns = Some(columns);
            }
            Kind::Scroll => {
                let axis = get("axis").and_then(Value::as_str).unwrap_or("y");
                out.scroll_axis = match axis {
                    "x" => Axis::X,
                    "y" => Axis::Y,
                    "both" => Axis::Both,
                    other => {
                        return Err(err(format!(
                            "element at {path}: 'scroll.axis' must be x, y or both, not '{other}'"
                        )))
                    }
                };
            }
            Kind::Input => {
                let mut props = InputProps::default();
                if let Some(value) = get("placeholder") {
                    props.placeholder = short_string(value, path, "input.placeholder")?;
                }
                if let Some(value) = get("value") {
                    props.value = short_string(value, path, "input.value")?;
                }
                props.multiline = get("multiline").and_then(Value::as_bool).unwrap_or(false);
                props.password = get("password").and_then(Value::as_bool).unwrap_or(false);
                out.input = Some(props);
            }
            Kind::Icon => {
                let svg = get("svg")
                    .and_then(Value::as_str)
                    .ok_or_else(|| err(format!("element at {path}: 'icon.svg' is required")))?;
                if svg.len() > MAX_STRING_BYTES {
                    return Err(err(format!("element at {path}: 'icon.svg' is too large")));
                }
                out.icon_svg = Some(svg.to_string());
            }
            Kind::Image => {
                let data = get("data")
                    .and_then(Value::as_str)
                    .ok_or_else(|| err(format!("element at {path}: 'image.data' is required")))?;
                if !data.starts_with("data:") {
                    return Err(err(format!(
                        "element at {path}: 'image.data' must be an inline data URI"
                    )));
                }
                if data.len() > MAX_STRING_BYTES {
                    return Err(err(format!("element at {path}: 'image.data' is too large")));
                }
                out.image_data = Some(data.to_string());
            }
            Kind::Progress => {
                let value = get("value").and_then(Value::as_f64).ok_or_else(|| {
                    err(format!("element at {path}: 'progress.value' is required"))
                })?;
                if !(0.0..=1.0).contains(&value) {
                    return Err(err(format!(
                        "element at {path}: 'progress.value' must be 0.0..=1.0"
                    )));
                }
                out.progress = Some(ProgressProps {
                    value: value as f32,
                    indeterminate: get("indeterminate")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                });
            }
            _ => {}
        }

        Ok(out)
    }

    fn style(
        &self,
        style: Option<&Value>,
        path: &str,
    ) -> Result<Vec<(String, StyleValue)>, UiError> {
        let Some(style) = style.filter(|value| !value.is_null()) else {
            return Ok(Vec::new());
        };
        let object = style
            .as_object()
            .ok_or_else(|| err(format!("element at {path}: 'style' must be an object")))?;
        if object.len() > MAX_STYLE_ENTRIES {
            return Err(err(format!(
                "element at {path} has {} style entries; the limit is {MAX_STYLE_ENTRIES}",
                object.len()
            )));
        }
        let mut out = Vec::with_capacity(object.len());
        for (name, value) in object {
            let kind = style_arg_kind(name)
                .ok_or_else(|| err(format!("element at {path}: unknown style method '{name}'")))?;
            let parsed = parse_style_value(name, kind, value, path)?;
            out.push((name.clone(), parsed));
        }
        Ok(out)
    }

    fn events(&self, on: Option<&Value>, path: &str) -> Result<Vec<(EventKind, String)>, UiError> {
        let Some(on) = on.filter(|value| !value.is_null()) else {
            return Ok(Vec::new());
        };
        let object = on
            .as_object()
            .ok_or_else(|| err(format!("element at {path}: 'on' must be an object")))?;
        let mut out = Vec::with_capacity(object.len());
        for (name, value) in object {
            let event = match name.as_str() {
                "click" => EventKind::Click,
                "change" => EventKind::Change,
                "submit" => EventKind::Submit,
                other => return Err(err(format!("element at {path}: unknown event '{other}'"))),
            };
            let id = value.as_str().ok_or_else(|| {
                err(format!(
                    "element at {path}: event '{name}' must be a callback id string"
                ))
            })?;
            if id.is_empty() || id.len() > MAX_ID_LEN * 2 {
                return Err(err(format!(
                    "element at {path}: event '{name}' has an invalid callback id"
                )));
            }
            out.push((event, id.to_string()));
        }
        Ok(out)
    }
}

fn short_string(value: &Value, path: &str, field: &str) -> Result<String, UiError> {
    let text = value
        .as_str()
        .ok_or_else(|| err(format!("element at {path}: '{field}' must be a string")))?;
    if text.len() > MAX_TEXT_BYTES {
        return Err(err(format!("element at {path}: '{field}' is too large")));
    }
    Ok(text.to_string())
}

fn validate_id(id: &str, path: &str) -> Result<(), UiError> {
    if id.is_empty() {
        return Err(err(format!("element at {path}: 'id' must not be empty")));
    }
    if id.len() > MAX_ID_LEN {
        return Err(err(format!(
            "element at {path}: 'id' is longer than {MAX_ID_LEN} bytes"
        )));
    }
    if let Some(bad) = id
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':' | '/')))
    {
        return Err(err(format!(
            "element at {path}: 'id' contains an invalid character '{bad}'"
        )));
    }
    Ok(())
}

fn parse_style_value(
    name: &str,
    kind: ArgKind,
    value: &Value,
    path: &str,
) -> Result<StyleValue, UiError> {
    match kind {
        ArgKind::None => match value {
            Value::Bool(true) | Value::Null => Ok(StyleValue::Flag),
            Value::Bool(false) => Err(err(format!(
                "element at {path}: style '{name}' is a flag; omit it instead of passing false"
            ))),
            _ => Err(err(format!(
                "element at {path}: style '{name}' takes no argument"
            ))),
        },
        ArgKind::Length | ArgKind::Absolute => {
            let length = parse_length(value, path, name)?;
            if kind == ArgKind::Absolute && !matches!(length, Length::Px(_)) {
                return Err(err(format!(
                    "element at {path}: style '{name}' requires a pixel length"
                )));
            }
            Ok(StyleValue::Length(length))
        }
        ArgKind::Number => {
            let number = value.as_f64().ok_or_else(|| {
                err(format!(
                    "element at {path}: style '{name}' requires a number"
                ))
            })? as f32;
            if !number.is_finite() {
                return Err(err(format!(
                    "element at {path}: style '{name}' must be finite"
                )));
            }
            Ok(StyleValue::Number(number))
        }
        ArgKind::Color => Ok(StyleValue::Color(parse_color(value, path, name)?)),
        ArgKind::String => {
            let text = short_string(value, path, name)?;
            Ok(StyleValue::Text(text))
        }
        ArgKind::Weight => {
            let weight = value.as_str().ok_or_else(|| {
                err(format!(
                    "element at {path}: style '{name}' requires a weight name"
                ))
            })?;
            if !FONT_WEIGHTS.contains(&weight) {
                return Err(err(format!(
                    "element at {path}: unknown font weight '{weight}'"
                )));
            }
            Ok(StyleValue::Weight(weight.to_string()))
        }
    }
}

/// Font weight names accepted by the `font_weight` style method.
pub const FONT_WEIGHTS: &[&str] = &[
    "thin",
    "extra_light",
    "light",
    "normal",
    "medium",
    "semibold",
    "bold",
    "extra_bold",
    "black",
];

fn parse_length(value: &Value, path: &str, name: &str) -> Result<Length, UiError> {
    if let Some(number) = value.as_f64() {
        let number = number as f32;
        if !number.is_finite() || !(0.0..=MAX_LENGTH_PX).contains(&number) {
            return Err(err(format!(
                "element at {path}: style '{name}' length must be 0..={MAX_LENGTH_PX}"
            )));
        }
        return Ok(Length::Px(number));
    }
    let text = value.as_str().ok_or_else(|| {
        err(format!(
            "element at {path}: style '{name}' requires a number or a length string"
        ))
    })?;
    if let Some(px) = text.strip_suffix("px") {
        let number: f32 = px
            .trim()
            .parse()
            .map_err(|_| err(format!("element at {path}: invalid length '{text}'")))?;
        if !number.is_finite() || !(0.0..=MAX_LENGTH_PX).contains(&number) {
            return Err(err(format!(
                "element at {path}: style '{name}' length must be 0..={MAX_LENGTH_PX}"
            )));
        }
        return Ok(Length::Px(number));
    }
    if let Some(percent) = text.strip_suffix('%') {
        let number: f32 = percent
            .trim()
            .parse()
            .map_err(|_| err(format!("element at {path}: invalid length '{text}'")))?;
        if !number.is_finite() || !(0.0..=100.0).contains(&number) {
            return Err(err(format!(
                "element at {path}: style '{name}' percentage must be 0..=100"
            )));
        }
        return Ok(Length::Percent(number));
    }
    Err(err(format!(
        "element at {path}: invalid length '{text}' for style '{name}'"
    )))
}

fn parse_color(value: &Value, path: &str, name: &str) -> Result<Color, UiError> {
    let text = value.as_str().ok_or_else(|| {
        err(format!(
            "element at {path}: style '{name}' requires a color string"
        ))
    })?;
    if let Some(hex) = text.strip_prefix('#') {
        if hex.len() != 6 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(err(format!(
                "element at {path}: color '{text}' must be #rrggbb"
            )));
        }
        let value = u32::from_str_radix(hex, 16)
            .map_err(|_| err(format!("element at {path}: invalid color '{text}'")))?;
        return Ok(Color::Hex(value));
    }
    if color_tokens().iter().any(|token| token == text) {
        Ok(Color::Token(text.to_string()))
    } else {
        Err(err(format!(
            "element at {path}: unknown color token '{text}'"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn validate(value: Value) -> Result<Node, UiError> {
        validate_view(&value)
    }

    #[test]
    fn style_table_is_well_formed_and_unique() {
        let methods = style_methods();
        assert!(methods.len() > 90, "expected a broad style surface");
        let mut names = std::collections::HashSet::new();
        for method in methods {
            assert!(
                names.insert(method.name.clone()),
                "duplicate {}",
                method.name
            );
        }
        assert_eq!(style_arg_kind("flex_col"), Some(ArgKind::None));
        assert_eq!(style_arg_kind("gap"), Some(ArgKind::Length));
        assert_eq!(style_arg_kind("bg"), Some(ArgKind::Color));
        assert_eq!(style_arg_kind("nope"), None);
    }

    #[test]
    fn minimal_ui_view_validates() {
        let node = validate(json!({
            "type": "ui",
            "root": {
                "kind": "col",
                "style": { "gap": 8, "p": 12 },
                "children": [
                    { "kind": "text", "text": "Hello" },
                    { "kind": "button", "text": "Go", "on": { "click": "go:click" } }
                ]
            }
        }))
        .unwrap();
        assert_eq!(node.kind, Kind::Col);
        assert_eq!(node.children.len(), 2);
        assert_eq!(node.style[0].0, "gap");
        assert_eq!(
            node.children[1].callback(EventKind::Click),
            Some("go:click")
        );
        assert_eq!(node.id, "r", "path-derived ids keep events stable");
    }

    #[test]
    fn unknown_style_and_kind_are_rejected() {
        let error = validate(json!({
            "type": "ui",
            "root": { "kind": "div", "style": { "bogus": 1 } }
        }))
        .unwrap_err();
        assert!(error.0.contains("unknown style method"), "{error}");

        let error = validate(json!({
            "type": "ui",
            "root": { "kind": "wormhole" }
        }))
        .unwrap_err();
        assert!(error.0.contains("unknown element kind"), "{error}");
    }

    #[test]
    fn limits_are_enforced() {
        // Depth.
        let mut node = json!({ "kind": "text", "text": "x" });
        for _ in 0..(MAX_DEPTH + 2) {
            node = json!({ "kind": "col", "children": [node] });
        }
        let error = validate(json!({ "type": "ui", "root": node })).unwrap_err();
        assert!(error.0.contains("depth"), "{error}");

        // Absolute length rejects percentages.
        let error = validate(json!({
            "type": "ui",
            "root": { "kind": "div", "style": { "rounded": "50%" } }
        }))
        .unwrap_err();
        assert!(error.0.contains("pixel length"), "{error}");

        // Out-of-range numbers.
        let error = validate(json!({
            "type": "ui",
            "root": { "kind": "div", "style": { "gap": 99999 } }
        }))
        .unwrap_err();
        assert!(error.0.contains("length must be"), "{error}");
    }

    #[test]
    fn colors_accept_tokens_and_hex_only() {
        validate(json!({
            "type": "ui",
            "root": { "kind": "div", "style": { "bg": "primary", "border_color": "#ff00aa" } }
        }))
        .unwrap();
        let error = validate(json!({
            "type": "ui",
            "root": { "kind": "div", "style": { "bg": "chartreuse" } }
        }))
        .unwrap_err();
        assert!(error.0.contains("unknown color token"), "{error}");
    }

    #[test]
    fn leaves_reject_children_and_containers_reject_text() {
        let error = validate(json!({
            "type": "ui",
            "root": { "kind": "text", "text": "x", "children": [] }
        }))
        .unwrap_err();
        assert!(error.0.contains("does not accept 'children'"), "{error}");

        let error = validate(json!({
            "type": "ui",
            "root": { "kind": "col", "text": "x" }
        }))
        .unwrap_err();
        assert!(error.0.contains("does not accept 'text'"), "{error}");
    }

    #[test]
    fn input_requires_an_id() {
        let error = validate(json!({
            "type": "ui",
            "root": { "kind": "input", "props": { "placeholder": "q" } }
        }))
        .unwrap_err();
        assert!(error.0.contains("explicit 'id'"), "{error}");
        validate(json!({
            "type": "ui",
            "root": { "kind": "input", "id": "q", "props": { "placeholder": "q" } }
        }))
        .unwrap();
    }
}
