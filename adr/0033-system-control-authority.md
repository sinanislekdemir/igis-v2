# 0033: System control has no capability check and uses focus-scoped shortcuts

- **Status:** Accepted
- **Date:** 2026-11-17
- **Deciders:** igis-v2 kernel/UI
- **Related specs:** 0001, 0003, 0005, 0007, 0010, 0012, 0017

## Context

M2.6.4 adds the desktop's System menu: About, Restart and Halt. Two lasting choices
come with it.

**Authority.** The system has no capability model: vision R14 and spec 0001 state that
every application may use every primitive, and `syslib.port_out8` already lets any task
touch any I/O port. Restart and halt therefore cannot be genuinely restricted without
introducing the capability layer the project has deliberately rejected. Hiding the menu
item would be cosmetic.

**Shortcuts.** The desktop is the boot entry and a permanent consumer of the one
machine-wide event queue (spec 0012, D19). A global key handler on the desktop would see
keys typed into every window; `Ctrl+A` in the editor would then open the About panel.
The key layer has no Command/Meta key (spec 0010), so the available namespace is
Ctrl/Alt plus function keys.

## Decision

**1. No capability check and no confirmation.** `core.halt()` and `core.reboot()` are
ordinary host builtins available to every task; a live kernel stops every other core on
the existing wake IPI, then halts or resets the calling core. The System menu uses direct
actions: selecting Restart or Halt acts immediately. The rationale is the existing R14
model; a confirmation dialog in an application would not add authority, only a second
place to make the same mistake. A task that wants to guard its own action can ask its
user first; the kernel does not mediate.

**2. Shortcuts are focus-scoped and modal-aware.** The desktop handles a key event only
when no pull-down and no About panel is open, and only when the event is routed to its
own window (`Event.window = own`) or has no target (`0`). While a pull-down is open its
menu key mapping owns Up/Down/Enter/Escape; while the About panel is open the dialog
mapping owns Enter/Escape/Tab. The shortcuts are `F10` (open/toggle System), `Ctrl+A`
(About), `Ctrl+R` (Restart) and `Ctrl+H` (Halt); only the Ctrl modifier is required.

**3. A halt is a full stop through the wake vector.** The wake IPI handler acknowledges
and halts a core that observes the shutdown flag, even mid-task. This avoids the
"idle-only acknowledgement" hole where a realtime controller on another core keeps
running while the boot core halts. `stop_all` waits bounded by the clock and a spin
ceiling, because the boot core's timer may stop before the acknowledgements arrive when
the requester is an AP.

**4. A reboot that the firmware ignores becomes a halt.** The architecture layer drives
`0xCF9` and the 8042 pulse-reset command; if the machine is still executing afterwards,
it halts instead of resuming the interpreter. A half-resumed machine after a failed
reset is worse than a stopped one.

## Consequences

- Any application can stop or reset the machine. This is consistent with the no-capability
  vision and is accepted for a single-operator personal system; it must be revisited
  before untrusted code sharing the machine is a goal.
- The desktop's shortcuts are usable only while its own window has focus (or nothing
  does). Users click the desktop window first; the menu bar is always clickable.
- `Ctrl+A` remains available to other applications because their key events are routed
  to their windows and the desktop ignores them. The editor's select-all keeps working.
- The kernel gains a second meaning for the wake vector (stop on shutdown). The handler
  is still idempotent for the TLB flush path when the machine is not shutting down.
- The System menu is application-drawn over the shadow. It has no window and no
  per-window canvas, so closing it repairs the covered pixels by raising the affected
  window and relying on `Expose`; this is the same M2 liveness model as the rest of the
  desktop.

## Alternatives considered

| Alternative | Why not |
|-------------|---------|
| Give the desktop a capability and refuse halt/reboot to other tasks | There is no capability model; adding one for two primitives would be an architecture change out of proportion to the milestone |
| Ask for a confirmation alert before Restart/Halt | The desktop's own dialog would be dismissed by the same Ctrl+R accident; the authority is unchanged and M2 accepts direct actions |
| Restart/halt through `syslib.port_out8` only | Halt needs the kernel's stop path and core count; a port write cannot acknowledge cores or report a partial stop |
| ACPI S5 power-off | No ACPI table parser; a halt is sufficient for M2 and is what the existing shutdown path already does |
| Global shortcuts regardless of focus | Would collide with every application's Ctrl+A/Ctrl+R (the editor's select-all) |
| Command-key equivalents | The key layer exposes no Meta key; inventing one is a spec-0010 change with no hardware mapping |
| Make the About panel a non-window screen overlay | Closing it would leave stale pixels in every window it covered; a real window gets the service's damage/expose repair for free |
