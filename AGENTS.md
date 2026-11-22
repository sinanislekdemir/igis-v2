# AGENTS.md

Operational guide for humans and coding agents working in this repository.
Keep this file short and current. It points at documents; it does not duplicate them.

**Last updated:** 2026-11-21

## What this is

`igis-v2` is a from-scratch second version of a personal operating system. The first
version (2005) began as a Linux driver for a modem; the user/kernel
ring boundary got in the way and it grew into a kernel. No code survives from it; the
continuity is design intent and the constraints learned there.

- **Language:** Rust (`no_std`, freestanding). x86_64 first, with ISA-specific code
  isolated so other architectures can follow (ADR 0013).
- **Thesis:** the kernel is a programmable system — a language runtime plus standard
  library that owns the machine. Applications are source text compiled in the kernel at
  load, never prebuilt binaries (ADR 0019). Everything runs at ring 0 in one address
  space (ADR 0001).
- **Execution:** one interpreter loop per core; tasks are claimed from a global runnable
  set and are never pinned to a core (ADR 0011).

The long-form narrative, the decision index and the dated status log live in
[`docs/HISTORY.md`](docs/HISTORY.md). Normative behavior is in `specs/`, lasting
decisions in `adr/`, and work items in `PLAN.md`.

## Current state

- Specs 0001–0024 are accepted; **M2–M14 are complete** (desktop, language,
  filesystems, networking, terminal emulator, runtime display modes, PNG/PPM image
  viewer). Current milestone work is tracked in `PLAN.md`.
- **634 host tests** (556 language + 56 `net` + 22 `ext2w`); ABI v1 has **75 opcodes**
  (runtime display modes add builtins 80–83; the machine-level bit and bulk buffer
  primitives are builtins 84–90 and `syslib.blit_rgb` is 91, no opcode).
- Boot: GRUB/multiboot2 → `system/entrypoint` (the desktop shell), falling back to
  `base/shell`; a graphics-less boot keeps a serial REPL.
- Filesystems: a read-only 128 MiB ext2 application image (`lib`, `base`, `apps`,
  `home`, `system`, `image`) with the full-resolution PPM P6 corpus at `image/`
  (the PNG decoder is retained for PNG files), and a persistent 8 MiB ext2
  `/home` on an ATA disk (`ext2w`, `e2fsck -fn` clean). GUI applications live
  under `apps/<category>/`.
- Networking: virtio-net and e1000/e1000e, IPv4/UDP/TCP, with DHCP/DNS/HTTP/`nc`/`purl`
  in `/lib` and `base/`.
- UI: a kernel window server with service-drawn close and minimize boxes, proportional
  font, one kernel System Log window, a terminal emulator
  (`apps/utilities/terminal`) over kernel console sessions, a Settings app
  (`apps/utilities/settings`) that changes the real mode through the Bochs VBE dispi
  (spec 0022, ADR 0056), and source apps (`editor`, `browser`, `calculator`,
  `minesweeper`, `clock_window`, `widgets`, `image_viewer`). The image viewer
  opens PNG (Adam7 pass-1 stream) and PPM (fixed-stride row seek and 1/8
  sampling), both through `syslib.blit_rgb`, with no full-image buffer and a
  decode progress strip. Every terminal
  instance has its own session, shell and cwd; the `Windows` pull-down lists every
  window (minimized rows marked `min`) and `File` opens a clickable window list. A
  window switch or a completed move runs a full-stack repaint (desktop, chrome
  bottom-to-top, every window's whole-content `Expose`) and presents only the pixels the
  service painted, so a moved window cannot leave stale content on the windows below
  (ADR 0055).

## Ground rules

1. **Spec before code.** Behavior changes are described in `specs/` and accepted before
   implementation. If reality and an accepted spec disagree, fix one of them explicitly.
2. **ADRs for decisions, specs for behavior.** A lasting constraint or technology choice
   gets an ADR in `adr/`. See `adr/README.md` for the immutability rule.
3. **Ideas are free.** Capture them in `IDEAS.md`; promote them when they matter.
4. **Don't invent requirements.** Ambiguity is resolved by asking, not by assumption.
   Record the answer in the relevant spec.
5. **Keep this file true.** Update the current-state block, layout and toolchain notes when
   they change. Anything that grows beyond a few lines belongs in a spec, an ADR or
   `docs/HISTORY.md`.
6. **Work is broken into tickets in `PLAN.md`.** One ticket ≈ one session and one commit;
   its acceptance criteria and the deferred backlog live there, not in chat.

### Delegating tickets to subagents

Delegate each ticket to a **fresh subagent session**. Do not continue an old session across
tickets: its context grows with every slice and the model becomes less reliable (observed
after three slices). For every delegation the parent supplies the full context in the new
prompt — spec/ADR, current commit, exact files and symbols, lock order, acceptance
criteria, verification commands and boundaries — and independently verifies the result
(host tests, kernel transcript on `-smp 4` and `-smp 1`, diff, regressions) before
accepting it. A running session may finish the ticket it was given; the next ticket starts
fresh.

## Repository layout

```
.
├── AGENTS.md          # this file: operational facts and conventions
├── README.md          # short project entry point
├── IDEAS.md           # unstructured inbox
├── OPEN-QUESTIONS.md  # parked decisions and unresolved questions
├── PLAN.md            # milestone plan, ticket backlog, deferred items
├── docs/              # long-form history and notes (HISTORY.md)
├── adr/               # decision log (immutable once accepted)
├── specs/             # normative specifications (living)
├── arch/x86_64/       # boot, paging, IDT/GDT, APIC/IOAPIC, PIT, ports, serial, keyboard
├── abi/               # opcodes, headers, Status/FaultCode, shared layout
├── compiler/          # lexer, parser, type/capacity check, codegen, manifest
├── interpreter/       # package verifier, machine, Host trait
├── langhost/          # host runner: compile, verify, run
├── ext2w/             # host-testable read/write ext2 over SectorDevice
├── net/               # host-testable Ethernet/ARP/IPv4/ICMP/UDP/TCP stack
├── kernel/            # boot/service modules at the crate root (main, repl, stream,
│   ├── mm/            #   event, clock, sync, net) plus the subject groups:
│   ├── platform/x86_64/  # PC devices: ATA, PCI, UHCI/HID, PS/2, AP bring-up
│   ├── sched/         #   interpreter loop and the task table
│   ├── ui/            #   console, sessions, fonts, framebuffer shadow, window server
│   └── vfs/           #   in-memory ext2 image reader and the path router
├── stdlib/            # standard library source, embedded into the compiler
├── samples/apps/      # shipped corpus staged into every image
│   ├── lib/           #   importable modules (`*.ig`, names kept)
│   ├── base/          #   command-line/session apps (`<name>.ig`, staged as `<name>`)
│   ├── apps/          #   GUI apps under accessories/games/productivity/utilities
│   └── system/        #   boot and probe applications (same rule)
├── samples/tests/     # test-only corpus, staged only by `image --shell`
│   └── apps/          #   proofs (`base/`) and binary fixtures (`system/`)
├── samples/image/     # full-resolution PPM P6 corpus staged as `image/`
├── samples/home/      # single seed tree for `home/` and the writable disk
├── samples/lang/      # language examples run by `langhost`
├── scripts/           # QEMU/QMP harnesses (screenshots, proofs, regressions)
├── tests/golden/      # visual-regression golden frame + manifest
├── xtask/             # build, image and QEMU orchestration
├── Cargo.toml         # workspace
├── rust-toolchain.toml
└── .cargo/config.toml
```

Kernel layout (ADR 0052): `arch/x86_64` is the ISA/boot layer;
`kernel/src/platform/x86_64` holds the PC device drivers (PCI, ATA, UHCI/HID, PS/2) and AP
bring-up; `mm`, `sched`, `ui` and `vfs` group the remaining kernel services, and the
boot/service modules stay at the crate root. Crate-root re-exports keep the historical
`crate::gfx`-style paths. The crate is still one compilation unit; a future `platform`
crate can take `platform/x86_64` wholesale. The two ext2 readers are deliberate:
`vfs/ext2` reads the in-memory GRUB image, while `ext2w` owns the writable disk and its
`RefCell` cache is not `Sync` (Q59).

## Toolchain

| Item | Version / value |
|------|-----------------|
| stable (default) | rustc 1.99.0 |
| nightly | rustc 1.101.0-nightly (2026-11-02) |
| targets installed | `x86_64-unknown-linux-gnu`, `x86_64-unknown-none` |
| extra components | `rust-src` (nightly), required for `-Z build-std` |
| QEMU | qemu-system-x86_64 10.2.2 |
| boot tooling | GRUB 2.12 (`grub2-mkrescue`), xorriso |

## Build commands

The kernel is cross-built for `x86_64-unknown-none`; `xtask` itself builds for the host.

| Command | Purpose |
|---------|---------|
| `cargo xtask build` | build the kernel ELF (release) |
| `cargo xtask image` | build and assemble `build/igis.iso` with GRUB and ensure `build/home.img` |
| `cargo xtask run` | image + QEMU with a display and the home disk, serial on stdio |
| `cargo xtask run --headless` | image + QEMU with no display (serial only) |
| `cargo xtask clean` | remove build artifacts |
| `cargo test -p abi -p compiler -p interpreter -p langhost -p ext2w -p net` | host language, filesystem and network-stack tests (the kernel crates have no test harness) |
| `cargo run -p langhost -- samples/lang/hello.ig` | compile and run one unit on the host |

`.cargo/config.toml` pins the linker script, `code-model=kernel`, and the static
relocation model for the bare-metal target. The workspace has no default target, so
host tools are unaffected.

## Document conventions

- Specs: `specs/NNNN-kebab-title.md`, template `specs/0000-template.md`, index in
  `specs/README.md`. Status must be one of Draft / Accepted / Implemented / Withdrawn /
  Superseded.
- ADRs: `adr/NNNN-kebab-title.md`, template `adr/0000-template.md`, index in
  `adr/README.md`.
- Requirements are numbered and use MUST / SHOULD / MAY (RFC 2119).
- Dates are ISO 8601 (`YYYY-MM-DD`).
- Work in small commits; one logical change per commit. Do not commit unrelated edits.

## Open questions

See [`OPEN-QUESTIONS.md`](OPEN-QUESTIONS.md) for the remaining parked items: tuning
constants, the deferred debug/DMA/streams work, the float follow-ups, virtio-blk, a
journal/atomic replace, the file handle model (Q51) and networking follow-ups
(TLS/HTTPS Q52, IPv6 Q53, Wi-Fi Q54, multiple NICs Q55, TCP backlog Q56, DHCP/DNS
follow-ups Q57, network trust Q58). Boot, platform, scheduler, REPL, language and
filesystem decisions are settled.
When an item is resolved, record the answer in its spec or a new ADR and remove it there.

## History and decisions

The detailed narrative, the decision index and the dated status log are archived in
[`docs/HISTORY.md`](docs/HISTORY.md). Per-decision detail stays in [`adr/`](adr/README.md);
per-requirement behavior stays in [`specs/`](specs/README.md).
