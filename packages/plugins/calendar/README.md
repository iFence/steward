# Calendar

Official NSH TSX plugin. Type `calendar`, `日历`, `rili`, or `rl` in Steward to
open a launcher-positioned calendar page with Gregorian dates and their Chinese
lunar equivalents. Each day shows its lunar day (or month start/festival).

The interface follows Steward's language setting and supports English,
Simplified Chinese, Japanese, Korean, German, French, and Russian. In the
standalone NSH development host it follows the operating-system locale.
Settings can switch the week start between Monday and Sunday and hide the ISO
week-number column.

The command accepts `calendar +3`, `calendar -1`, or an absolute
`calendar 2026-09`. Use the header buttons to move between months or return
to today. In embedded presentation, the Back button or Escape returns to the
launcher. Clicking a day keeps a selected state that is visually distinct from
today's marker. A small label after the month title shows how many days the
selected date is before or after today, and is hidden when the selected date is
today. Use Left/Right to move the selection by one day and Up/Down to move it
by one week.
Leading and trailing cells show adjacent-month dates in a muted color; clicking
one switches to that month.

Each cell combines public festivals, lunar festivals, and the 24 solar terms.
Mainland China public holidays carry a holiday badge, while adjusted workdays
carry a workday badge. The official schedule is fetched from
[Timor's free holiday API](https://timor.tech/api/holiday/) instead of being
bundled with the plugin.

Each full-year response is cached in the plugin's persistent storage. Empty or
failed responses are not treated as valid cached schedules, and unavailable
data is retried when the page becomes visible again. Steward also keeps a
host-side year cache under its data directory; entries older than seven days
are refreshed, while a failed refresh continues to use the previous schedule.
If no schedule is available, the calendar remains usable and omits official
holiday badges.

Lunar conversion covers 1900 through 2100 and includes leap months.

Files:

```text
App.tsx        NSH application
App.test.tsx   headless behavior test
calendar-i18n.ts localized interface copy
calendar-meta.ts solar terms and holiday API normalization
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

## Acknowledgements

Thank you to [Timor's free holiday API](https://timor.tech/api/holiday/) for
providing mainland China holiday and adjusted-workday data. The service is
free, requires no login, and currently limits requests to 10,000 per IP address
per 24 hours. Site content is published under
[CC BY-NC-SA 4.0](https://creativecommons.org/licenses/by-nc-sa/4.0/).
