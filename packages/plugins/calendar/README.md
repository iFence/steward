# Calendar

Official NSH TSX plugin. Type `calendar` in Steward to open a full calendar
window with Gregorian dates and their Chinese lunar equivalents. Each day
shows its lunar day (or month start/festival), and the selected-date card
shows the full lunar date, zodiac year, and festival when applicable.

The command accepts `calendar +3`, `calendar -1`, or an absolute
`calendar 2026-09`. Use the header buttons to move between months or return
to today. Clicking a day selects it and copies its ISO date.

Lunar conversion covers 1900 through 2100 and includes leap months.

Files:

```text
App.tsx        NSH application
App.test.tsx   headless behavior test
lunar.ts       lunar conversion, festivals, and date labels
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
