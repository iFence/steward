# UI Showcase

Official example plugin for the Virtual UI Tree (`{ "type": "ui" }`) view. It
builds a small interactive tree with the `@steward/extension-api` element
builder: layout containers, styled text, a button whose `click` handler redraws
the view, and a host-owned input whose `change` handler updates a label.

Try it: build the plugin, install it under `<data dir>/Steward/plugins`, then
run `uishowcase` in the launcher. Because the command declares `detachable`,
the tree can also be popped out into its own window.
