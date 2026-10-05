/**
 * Element builder for the `ui` view (`{ type: "ui", root }`).
 *
 * The plugin never renders: it builds a serializable element tree and returns
 * it from a command or an event handler. The host validates the tree and
 * replays it into real gpui elements, so the same tree type-checks here and is
 * rejected or rendered there.
 *
 * Style methods come from `styles.generated.ts`, which is generated from the
 * host's canonical style table; there is no second list to keep in sync.
 */

import { StyledElement } from "./styles.generated";

/** One element on the wire. */
export interface UiNode {
  kind: string;
  id?: string;
  style?: Record<string, unknown>;
  text?: string;
  props?: Record<string, unknown>;
  children?: UiNode[];
  on?: Record<string, string>;
}

/** A `ui` view: a tree the host renders. */
export interface UiView {
  type: "ui";
  root: UiNode;
}

/** An element event delivered on `view.invoke`. */
export interface ElementEvent {
  type: "click" | "change" | "submit";
  node_id: string;
  /** Present for `change` / `submit`: the current text of the input. */
  value?: string;
}

/** An event handler. Returning a `ui` view replaces the current tree. */
export type ElementHandler = (event: ElementEvent) => UiView | void | Promise<UiView | void>;

type EventName = "click" | "change" | "submit";

/** Upper bound on retained handlers, so a long session cannot grow unbounded. */
const MAX_CALLBACKS = 4096;
const callbacks = new Map<string, ElementHandler>();

function trimCallbacks(): void {
  while (callbacks.size > MAX_CALLBACKS) {
    const oldest = callbacks.keys().next().value;
    if (oldest === undefined) {
      break;
    }
    callbacks.delete(oldest);
  }
}

/**
 * The builder. Every style method from the generated base returns `this`, so
 * calls chain exactly like the Rust/gpui API they mirror.
 */
export class Element extends StyledElement {
  readonly kind: string;
  text?: string;
  /** @internal */
  readonly _props: Record<string, unknown> = {};
  /** @internal */
  readonly _children: Element[] = [];
  /** @internal */
  _id?: string;
  /** @internal */
  readonly _handlers: Partial<Record<EventName, ElementHandler>> = {};

  constructor(kind: string) {
    super();
    this.kind = kind;
  }

  /** A stable id. Required for inputs; used to key callbacks otherwise. */
  id(value: string): this {
    this._id = value;
    return this;
  }

  /** A kind-specific prop (`placeholder`, `columns`, `value`, ...). */
  prop(name: string, value: unknown): this {
    this._props[name] = value;
    return this;
  }

  /** Append children. */
  child(...nodes: Element[]): this {
    this._children.push(...nodes);
    return this;
  }

  /** Append a list of children. */
  children(...nodes: Element[]): this {
    this._children.push(...nodes);
    return this;
  }

  onClick(handler: ElementHandler): this {
    this._handlers.click = handler;
    return this;
  }

  onChange(handler: ElementHandler): this {
    this._handlers.change = handler;
    return this;
  }

  onSubmit(handler: ElementHandler): this {
    this._handlers.submit = handler;
    return this;
  }

  /** @internal */
  _snapshotStyle(): Record<string, unknown> {
    return this._style;
  }
}

function finalize(node: Element | UiNode, path: string): UiNode {
  if (node instanceof Element) {
    const id = node._id && node._id.length > 0 ? node._id : path;
    const out: UiNode = { kind: node.kind, id };
    const style = node._snapshotStyle();
    if (Object.keys(style).length > 0) {
      out.style = { ...style };
    }
    if (node.text !== undefined) {
      out.text = node.text;
    }
    if (Object.keys(node._props).length > 0) {
      out.props = { ...node._props };
    }
    const on: Record<string, string> = {};
    for (const [event, handler] of Object.entries(node._handlers)) {
      if (!handler) {
        continue;
      }
      const callbackId = `${id}:${event}`;
      callbacks.set(callbackId, handler);
      on[event] = callbackId;
    }
    trimCallbacks();
    if (Object.keys(on).length > 0) {
      out.on = on;
    }
    if (node._children.length > 0) {
      out.children = node._children.map((child, index) => finalize(child, `${path}/${index}`));
    }
    return out;
  }
  if (Array.isArray(node.children)) {
    node.children = node.children.map((child, index) => finalize(child, `${path}/${index}`));
  }
  return node;
}

/**
 * Finalize a returned view before the host serializes it: elements become
 * plain JSON and their handlers are registered under stable callback ids.
 * Installed as `globalThis.__stewardPrepareView`.
 */
export function prepareView(view: unknown): unknown {
  if (view && typeof view === "object" && (view as UiView).type === "ui" && (view as UiView).root) {
    (view as UiView).root = finalize((view as UiView).root, "r");
  }
  return view;
}

/**
 * Run the handler registered for `callbackId`. Throws on an unknown id, which
 * the runtime maps to `CALLBACK_NOT_FOUND`. Installed as
 * `globalThis.__stewardInvokeCallback`.
 */
export function invokeElementCallback(callbackId: string, eventJson: string): unknown {
  const handler = callbacks.get(callbackId);
  if (!handler) {
    throw new Error(`unknown callback: ${callbackId}`);
  }
  return handler(JSON.parse(eventJson) as ElementEvent);
}

// The runtime calls these hooks; the bundle installs them at import time.
(globalThis as Record<string, unknown>).__stewardPrepareView = prepareView;
(globalThis as Record<string, unknown>).__stewardInvokeCallback = invokeElementCallback;

/** A plain container that lays out its children with an explicit style. */
export const div = (): Element => new Element("div");
/** A horizontal flex container. */
export const row = (): Element => new Element("row");
/** A vertical flex container. */
export const col = (): Element => new Element("col");
/** A wrapping grid of `columns` columns. */
export const grid = (columns = 4): Element => new Element("grid").prop("columns", columns);
/** A scroll container. */
export const scroll = (axis: "x" | "y" | "both" = "y"): Element =>
  new Element("scroll").prop("axis", axis);
/** A text run. */
export const text = (value: string): Element => {
  const element = new Element("text");
  element.text = value;
  return element;
};
/** An inline-SVG icon. */
export const icon = (svg: string): Element => new Element("icon").prop("svg", svg);
/** An image from an inline `data:` URI (SVG is materialized in v1). */
export const image = (data: string): Element => new Element("image").prop("data", data);
/** A clickable button. */
export const button = (label?: string): Element => {
  const element = new Element("button");
  if (label !== undefined) {
    element.text = label;
  }
  return element;
};
/** A link-styled clickable text run. */
export const link = (label: string): Element => {
  const element = new Element("link");
  element.text = label;
  return element;
};
/** A small pill label. */
export const badge = (label: string): Element => {
  const element = new Element("badge");
  element.text = label;
  return element;
};
/** A 1px rule. */
export const separator = (): Element => new Element("separator");
/** A flexible gap that pushes siblings apart. */
export const spacer = (): Element => new Element("spacer");
/** A 0.0..=1.0 progress bar. */
export const progress = (value: number): Element => new Element("progress").prop("value", value);

/** Options for {@link input}. */
export interface InputOptions {
  placeholder?: string;
  value?: string;
  multiline?: boolean;
  password?: boolean;
}

/**
 * A host-owned text input. The host keeps the buffer (typing stays local) and
 * delivers `change` / `submit` events; `value` is only the initial text.
 */
export const input = (id: string, options: InputOptions = {}): Element => {
  const element = new Element("input").id(id);
  if (options.placeholder !== undefined) {
    element.prop("placeholder", options.placeholder);
  }
  if (options.value !== undefined) {
    element.prop("value", options.value);
  }
  if (options.multiline) {
    element.prop("multiline", true);
  }
  if (options.password) {
    element.prop("password", true);
  }
  return element;
};

/** Wrap a root element as a `ui` view. */
export const ui = (root: Element): UiView => ({ type: "ui", root: finalize(root, "r") });
