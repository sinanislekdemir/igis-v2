# 0029: Boot into the desktop; launch the shell from its Terminal item

- **Status:** Accepted
- **Date:** 2026-11-17
- **Supersedes:** the boot-order detail in ADR 0017's addendum (the REPL direction and
  spawn queue stay as decided there)
- **Related:** spec 0012 (D9–D11, D19), spec 0014 (T12/T14), spec 0002, spec 0010

## Context

ADR 0017 made boot try `shell` first and `entrypoint` second, because the first
milestone had no desktop. M2.3.4 added `samples/apps/desktop_shell` as an ordinary
application, and M2.5.5 gave the text shell a task-owned terminal window. The desktop
milestone's last application ticket (M2.5.6) has to make the desktop the primary shell
without breaking the pre-desktop boot or the one-machine-wide-terminal rule.

Two constraints shape the decision:

1. The text shell is the only REPL: it owns the console byte stream, the line
   discipline and (from M2.5.5) the attached terminal grid. There is exactly one
   terminal; a second shell would get `Full`.
2. The desktop is a normal application, not kernel boot code. It draws the menu bar and
   responds to the pointer through the ordinary `syslib`/event surface.

## Decision

- **Boot entry.** The REPL tries `entrypoint` first and `shell` second. `xtask image`
  stages the `desktop_shell` source a second time under the file name `entrypoint`; the
  unit keeps its name, so the source is not duplicated and diagnostics still say
  `desktop_shell`. An image without `entrypoint` (or an `entrypoint` that fails to
  load) boots the text shell exactly as before. `xtask image --shell` omits the alias
  and writes `build/igis-shell.iso`.
- **Menu launch.** The desktop's direct `Terminal` item `core.spawn`s `shell` without
  waiting and records the task id. A later click raises the live `Terminal` window
  instead of spawning a second shell; if the shell exited and the window is gone, the
  click spawns a fresh one. The desktop never attaches the terminal and never blocks on
  the shell.
- **No graphics.** A boot desktop that cannot draw prints `desktop_shell: no graphics`
  and spawns the text shell, preserving a serial REPL on a machine without a
  framebuffer.
- **Event queue.** The desktop is a permanent consumer of the one machine-wide event
  queue (spec 0010, I31). Applications launched from the shell that also call
  `next_event` compete with it. The event-injecting regression suite therefore runs on
  the shell-entry image, which is the pre-M2.5.6 boot. The shell's `core.write` /
  `core.read_line` terminal is independent of the event queue and works in both boots.

## Alternatives considered

| Alternative | Why not |
|-------------|---------|
| Keep `shell` first and let the desktop be a manual `run` | The milestone is boot-to-desktop; a user would have to type `run desktop_shell` after every boot, and the shell would already hold the terminal |
| Auto-launch the shell from the desktop at boot | The ticket's contract is a menu launch, and an automatic shell would hide whether the menu path works; it also makes the desktop's first frame race the shell's window |
| Duplicate the desktop source into `samples/apps/entrypoint` | Two sources drift; the compiler only needs the file name to differ, not the unit |
| Make `Apps` a dropdown with Clock and Terminal | M2.3.4 deliberately uses direct items; a dropdown would pull the M2.4.3 menu machinery into the boot path. A direct `Terminal` item is the smaller change |
| Spawn a second shell on every Terminal click | The one-terminal rule makes its attach `Full`; it would run serial-only and compete for `read_line` with the first shell |
| Per-task event mailboxes now | A larger change to spec 0010's one-queue model; the shell fallback image keeps the existing event-injecting proofs meaningful until that work is scheduled |
| Keep the shell as the boot entry and only add a menu | Contradicts the milestone and leaves the desktop as a manual demo |

## Consequences

- The boot image lands on the desktop; the shell is one menu click away.
- The shell-entry image is a first-class build product for the fallback proof and for
  the event-injecting regression suite.
- The single event queue is a documented M2 limitation for apps launched under the boot
  desktop; the terminal path (the REPL itself) is unaffected.
- ADR 0017's REPL service and spawn-queue decision is unchanged; only its boot-order
  addendum is superseded.
