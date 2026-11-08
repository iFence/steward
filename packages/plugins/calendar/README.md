# Calendar

Official NSH TSX plugin. Type `calendar` in Steward to open a full calendar
window. The command accepts `calendar +3`, `calendar -1`, or an absolute
`calendar 2026-09`; clicking a day copies its ISO date.

Files:

```text
App.tsx        NSH application
App.test.tsx   headless behavior test
nsh.json       NSH package metadata
plugin.json    Steward routing metadata
```

Development:

```powershell
nsh check App.tsx
nsh test .
nsh build App.tsx --release -o dist
```

Install the packaged zip by extracting its `calendar/` directory under
`%APPDATA%\Steward\plugins`.
