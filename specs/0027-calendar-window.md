# 0027: The calendar window

- **Status:** Implemented
- **Scope:** `samples/apps` (`apps/productivity/calendar`, `base/desktop_shell`),
  `langhost`, `scripts`
- **Supersedes:** none
- **Superseded by:** none
- **ADRs:** none
- **Updated:** 2026-11-26

## Context

The desktop ships a clock but no way to look at a month. The user asked for a
calendar application. The first version is deliberately read-only: it shows one
month at a time, highlights today from the CMOS realtime clock and lets the user
walk through months and years. Event notes, reminders and a persistent store are
out of scope; they would need a file format and write policy of their own.

The application is ordinary source text compiled by the kernel, like the clock
and the calculator. It adds no opcode, no builtin, no `Host` method and no kernel
service.

## Requirements

### Data model

- **C1 (MUST):** The calendar owns a view year, a view month (1–12), an optional
  selected day (0 = none) and the host's today (`known`, year, month, day).
  `today` is read once at start from the CMOS RTC registers (day 0x07, month
  0x08, year 0x09, century 0x32), honouring the status-B binary/BCD flag, with
  the update-in-progress wait and a stable re-read. A two-digit year whose
  century register is absent or implausible is read as `2000 + yy`.
- **C2 (MUST):** A year is a leap year when divisible by 4 and not by 100, or
  divisible by 400. Month lengths are the Gregorian lengths; February follows
  the leap rule.
- **C3 (MUST):** Weekdays are computed from the civil date with the
  days-from-civil algorithm; the grid is Monday-first, so the first column is
  Monday and `lead = (sunday_index(year, month, 1) + 6) mod 7`.
- **C4 (MUST):** The grid always draws six week rows (42 cells). A cell outside
  the month is blank and is not a hit target.
- **C5 (MUST):** Navigation clamps at years 1970 and 9999: the previous month at
  1970-01 and the next month at 9999-12 are no-ops. A month change clears the
  selected day.

### Window and input

- **C6 (MUST):** The application opens one 252×240 content window titled
  `Calendar` at screen `(300, 110)` and draws a white client panel, one
  one-pixel bezel, a header with a `<` and a `>` button, the month and year, a
  Monday-first weekday row, the six-row grid and two footer lines. It draws the
  whole content and presents once; an owner `Expose` repaints the whole content in
  window-local coordinates (spec 0026 C8), with no origin refresh.
- **C7 (MUST):** Today's cell is inverted (black fill, white number) only when
  the shown month is the host's month and the RTC read succeeded. A selected
  day that is not today gets a one-pixel accent rectangle; selection never
  changes today's look.
- **C8 (MUST):** Left/Right move one month, Up/Down move one year and `H` (or
  Home) returns to the host's month and selects today. `<`/`>` are ordinary
  pressed/released buttons: a press inside arms the button and a release inside
  activates it; a release outside cancels. A press on a day cell selects that
  day.
- **C9 (MUST):** A close request closes only this window and exits the task; a
  live instance blocks in `syslib.next_event` between frames so blocked time is
  not charged to the normal-task budget.
- **C10 (MUST):** With graphics disabled (`screen_width() == 0`) no window is
  opened; the pure date, grid, navigation and key-dispatch logic runs and the
  task prints `calendar: host ok`. A screen smaller than 592×410 prints
  `calendar: screen too small`.

### Modes

- **C11 (MUST):** `check` fixes today to 2026-11-26, draws the month and
  verifies the bezel, the base, both nav frames, the today fill, a blank
  neighbouring cell, the nav hit test, a day hit test, one Left-key dispatch
  and one `>` press/release, then closes the window and prints `calendar: ok`.
- **C12 (MUST):** `shot` draws the host month (or the fixed date when the RTC is
  unavailable), prints `calendar: ready YYYY-MM-DD` (or
  `calendar: ready (no rtc)`), holds about five seconds with `core.sleep` and
  closes the window.
- **C13 (MUST):** Live mode prints `calendar: ready YYYY-MM-DD` after the first
  frame and `calendar: closed` after the close request.

### Desktop integration

- **C14 (MUST):** The Apps pull-down gains `Calendar` at the end of the
  Productivity section (`apps/productivity/calendar`). The menu becomes 14 rows
  (226 pixels): rows 10 and 13 are the Calendar and Settings items, the
  `#Utilities` header moves to 11 and Terminal to 12.

## Design

The application mirrors `clock_window`: integer-only geometry, the 8×8 console
font through `syslib.draw_text`, `stdlib.ui_button`/`ui_frame` chrome, one
`window_present` per complete frame and no `syslib.clear`. The date math is pure
and is exercised on the host; the RTC is read through the existing
`syslib.port_in8`/`port_out8` builtins.

The footer shows `Today: <day> <month> <year>` when the RTC read succeeded and
`Today: unavailable` otherwise. The second line is the key hint
`Arrows: month/year  H: today`.

### Invariants

- A frame is drawn entirely before it is presented.
- The view month is always 1–12 and the view year 1970–9999.
- `selected_day` is either 0 or a day that exists in the shown month.
- A failed RTC read leaves `today_known` false and draws no today highlight.

## Interfaces

No new interface. The application uses only existing `core`, `stdlib` and
`syslib` calls and is staged by `xtask` as an ordinary source file under
`apps/productivity/`.

## Failure modes

- No RTC / unstable read: the calendar still works, without a today highlight
  and with `Today: unavailable`.
- No graphics: the host path runs and prints the host verdict.
- Window open failure: the task prints `calendar: no window` and exits.
- A close request while a shot holds the frame is not serviced until the hold
  ends; the hold is bounded to five seconds.

## Alternatives considered

- Event notes in `/home` were rejected for the first version: they need a
  format, a parser, an editor and a write policy, and the user chose the
  read-only month view.
- A fixed 2026-only view was rejected; navigation is cheap and makes the
  application useful.

## Open questions

| # | Question | Trigger / owner |
|---|----------|-----------------|
| 1 | Should a later version add day notes in `/home`? | product owner |

## Changelog

| Date | Change |
|------|--------|
| 2026-11-26 | Initial version implemented. |
