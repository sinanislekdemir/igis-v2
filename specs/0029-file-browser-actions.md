# 0029: File browser open-with, context menu and clipboard

- **Status:** Implemented
- **Scope:** `samples/apps/apps/productivity/browser`,
  `samples/apps/apps/utilities/terminal`, `samples/tests/apps/base`,
  `langhost`, `scripts`, specs
- **Supersedes:** the M18 default-open rule (spec 0028 B5) and the implicit
  "Enter launches any file" behavior of spec 0013; the rest of both specs stays
- **Superseded by:** none
- **ADRs:** none
- **Updated:** 2026-11-27

## Context

The M18 browser opens a regular file by launching it with `core.spawn` and
`core.wait`. That is right for a source application, but wrong for a PPM/PNG
image (the image viewer can show it) and for a text file (the editor can edit
it). Files the browser does not recognise also deserve explicit actions instead
of a compiler error.

There is no system-wide clipboard and no writable path outside `/home`. The
browser therefore keeps a single-entry clipboard in its own state and only
paste/delete operations that the file API already supports: reads from the
read-only application tree, and writes/rename/remove under `home/`. No new
kernel builtin, syscall, opcode or file API is introduced.

"Run in terminal" needs the terminal emulator to accept one initial command. It
already owns the shell's session; sending the command to that session after the
shell spawn is enough and does not add a shell protocol.

## Requirements

### Default open

- **O1 (MUST):** `Enter` and a double-click on a directory enter it, exactly as
  in spec 0013.
- **O2 (MUST):** A regular file whose name ends in `.ppm` or `.png` (ASCII
  case-insensitive) is opened in `apps/accessories/image_viewer` with its full
  image path as the argument. The browser **spawns without waiting**: it stays
  usable while the viewer runs.
- **O3 (MUST):** A regular file whose name ends in `.txt` (ASCII
  case-insensitive) is opened in `apps/productivity/editor` with its full image
  path as the argument, again without waiting.
- **O4 (MUST):** A source application — a name ending in `.ig`, or a name with
  no dot in `base`, `system`, `apps` or `apps/<category>` (the stager drops
  `.ig` there, spec 0018 L3) — keeps the M18 behavior: it is launched with
  `core.spawn` + `core.wait` and the browser reports the exit status.
- **O5 (MUST):** Every other regular file opens the context menu of M1 at the
  selected cell. No child is spawned by the default action.
- **O6 (MUST):** A kind-2 entry is still refused with the existing status and
  never opened, previewed or run.
- **O7 (MUST):** A full path is built with the existing bounded join; a path
  that would exceed `PATH_MAX` or a failed spawn sets the existing status line
  and launches nothing.

### Context menu

- **M1 (MUST):** A right-click (`PointerDown` with button bit 1) on a regular
  file selects it and opens a context menu at the pointer. The same menu opens
  for O5 at the selected cell. A right-click on a directory or a kind-2 entry
  does not open it.
- **M2 (MUST):** The menu has exactly these six rows, in this order:

  ```
  Open with editor
  Run in terminal
  Cut
  Copy
  Paste
  Delete
  ```

  It is 220 pixels wide, has 16-pixel rows, is framed and drawn last over the
  grid by `stdlib.ui_menu_dropdown`, and is clamped inside the 680×480 content
  with a 4-pixel margin. The row under the pointer is highlighted; `Paste` is
  disabled and drawn gray when the clipboard is empty.
- **M3 (MUST):** While the menu is open, Up/Down move the highlight, Enter
  activates the highlighted row, Escape closes it, and a left or right click
  inside the menu activates/dismisses it. A click outside the menu dismisses it
  and is consumed by the browser. A directory change closes it.
- **M4 (MUST):** `Open with editor` spawns `apps/productivity/editor` with the
  selected full path, without waiting. `Run in terminal` spawns
  `apps/utilities/terminal` with the single argument string `run <full path>`,
  without waiting. `Cut`/`Copy`/`Paste` follow C1–C5; `Delete` calls the
  existing home-only remove and reports its status.
- **M5 (MUST):** The menu has no effect on the application-volume write rule:
  any action that needs a write outside `home/` reports the existing read-only
  status and changes nothing.

### Clipboard

- **C1 (MUST):** The browser owns one clipboard entry: the selected regular
  file's full path, its kind and a mode (`copy` or `cut`). It is browser-local
  and lasts until replaced, until a successful cut-paste, or until the browser
  exits. It is not a system clipboard.
- **C2 (MUST):** `Copy` stores the entry and sets `copied`; `Cut` stores it and
  sets `cut`. Copying a directory is not offered (the menu is only for regular
  files). Cut and copy do not touch the filesystem.
- **C3 (MUST):** `Paste` uses the current directory and the source basename as
  the target name. It refuses, with no change, when the clipboard is empty
  (`clipboard empty`), when the target is the source (`same directory`), when
  the target exists (`already exists`), or when the target directory is not
  writable (`read-only: outside home`). It never overwrites.
- **C4 (MUST):** A copy paste streams the source to a new destination with
  `fs_size`, 256-byte `fs_read` chunks and `fs_write` at increasing offsets. It
  creates the destination first (including a zero-byte file), copies exactly
  the source length, and removes the partial destination on any failure. The
  source may be read-only; only the destination must be under `home/`.
- **C5 (MUST):** A cut paste moves the source with `core.fs_rename` and clears
  the clipboard on success; a copy paste keeps it. On success the directory is
  reloaded, the pasted name is selected and the status line reads `pasted`.
  `Delete` removes the selected regular file or empty directory under `home/`
  exactly as in spec 0013; outside `home/` it reports the existing refusal.

### Terminal initial command

- **T1 (MUST):** `apps/utilities/terminal` accepts an optional initial command
  string. After `session_open` and the `base/shell` spawn succeed, a non-empty
  argument is sent to that session once, followed by `\r`. An empty argument
  keeps the current clean-prompt behavior. A send failure is reported on serial
  and does not leave a window or session behind.
- **T2 (MUST):** The terminal does not interpret the command itself; the shell
  does, with its ordinary `run`/`cd`/`exit` syntax. The browser's
  `Run in terminal` passes `run <full path>`.
- **T3 (MUST):** The shell's cwd starts at the image root, so the full path
  makes the action independent of the browser's current directory.

### Verification

- **V1 (MUST):** `browser check` proves the pure default-action mapping
  (directory, source, image, text, other), menu geometry/hit, the right-click
  path, copy → paste of a source file into `home/`, the copied size, and menu
  delete. It prints `browser: ok`; the host path prints `browser: host ok`
  without opening a window or spawning a child.
- **V2 (MUST):** The existing browser, iconview, terminal, file-dialog and
  home-operation proofs stay green. A `--shell` boot runs `browser check` on
  `-smp 4` and `-smp 1`; the host suite passes.
- **V3 (MUST):** No kernel, ABI, opcode, builtin, `Host` method or `stdlib`
  change. The only application behavior changes are the browser and the
  terminal's optional initial command.

## Failure modes

| Condition | Result |
|---|---|
| `.ppm`/`.png` selected but viewer spawn fails | `launch failed`; no child |
| `.txt` selected but editor spawn fails | `launch failed`; no child |
| Other file selected | context menu, no child |
| Clipboard empty | `Paste` disabled; activation is a no-op |
| Target exists | `already exists`, nothing overwritten |
| Destination outside `home` | `read-only: outside home` |
| Copy read fails mid-way | partial destination removed, `copy failed` |
| Cut source outside `home` | `paste failed` / existing rename status |
| Delete outside `home` | `read-only: outside home` |
| Menu open, click outside | menu closes, click consumed |
| Terminal initial send fails | serial `terminal: initial command failed`, window/session cleaned up |

## Open questions

- A system-wide clipboard, drag-and-drop and multi-selection are separate
  features; the current single-entry browser-local clipboard is deliberate.
- Copying a directory recursively needs a whole-tree walk and a policy for
  partial failures; it is not part of this change.
