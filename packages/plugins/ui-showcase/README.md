# UI Showcase

Official NSH TSX plugin demonstrating native components, signals, input,
buttons, hosted presentation navigation, and the NSH `Switch` control. It is
opened as an embedded presentation, but still runs in Steward's isolated
plugin-host process rather than inside the launcher process.

Development:

```bash
nsh check App.tsx
nsh test .
nsh build App.tsx --release -o dist
```
