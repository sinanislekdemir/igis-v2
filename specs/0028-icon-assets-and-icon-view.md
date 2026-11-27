# 0028: RGB icon assets and the icon view widget

- **Status:** Implemented
- **Scope:** `samples/icons`, `scripts`, `xtask`, `samples/apps/lib`
  (`icons`, `iconview`), `samples/apps/apps/productivity/browser`,
  `samples/tests/apps/base/iconview`, `langhost`
- **Supersedes:** the preview/listing requirements of spec 0013 ("The file
  browser"); no other requirement
- **Superseded by:** none
- **ADRs:** 0062
- **Updated:** 2026-11-27

## Context

The file browser lists one 16-pixel text row per entry. The owner wants an
icon view in the style of the old Windows Explorer: large (32×32) icons on a
grid with a two-line label under each one. The desktop already has a
monochrome 32×32 `ui_icon` bitmap for the M2.4.5 icon proof, but those masks
cannot carry colour and every new icon would be hand-drawn bit data.

The owner supplied four 64×64 SVG icons in `samples/icons/`: `folder`,
`igfile`, `file` and `image`. Parsing and rasterizing SVG in the kernel is not
an option: it needs an XML parser, path filling, gradients and anti-aliasing,
none of which the no_std tree has, and the icons never change at run time.
Instead the SVGs are source assets rasterized once to 32×32 PPM P6, the same
uncompressed RGB format the image viewer already reads (spec 0024). A 32×32
icon is 3,072 raster bytes, so the browser can load all four at start into
12,288 bytes and redraw them with one `syslib.blit_rgb` call each.

The change removes the browser's text preview pane. The window becomes a
full-width icon grid with the existing path line, scrollbar and status line;
`Enter`/double-click opens an entry like Explorer (a directory is entered, a
regular file is launched).

## Requirements

### Icon assets

- **A1 (MUST):** The tracked SVG sources are
  `samples/icons/{folder,igfile,file,image}.svg`. They are source assets, not
  runtime input; no application, `/lib` module, kernel component or build step
  parses SVG on the target.
- **A2 (MUST):** `scripts/make_icons.py` rasterizes those four names to
  `samples/icons/ppm/<name>.ppm`. Each output is binary PPM P6, exactly 32×32,
  maxval 255, 8-bit RGB, no alpha, raster 3,072 bytes. Transparent SVG areas
  are composited on `#FFFFFF` before the final resize. The conversion is
  deterministic: the same source and ImageMagick (librsvg delegate) produce
  the same bytes.
- **A3 (MUST):** The script is idempotent, takes `--force` to rewrite existing
  outputs, and validates every output (magic, dimensions, maxval, raster
  length). It fails with a clear message when ImageMagick is unavailable. The
  four generated PPMs are committed because they are 12 KB in total and make
  the image build independent of a rasterizer.
- **A4 (MUST):** `samples/icons/scalable/` is a source library that is not
  part of the shipped image or git history in this change; it is git-ignored.
- **A5 (MUST):** The application image gains one read-only top-level directory
  `icons/` holding the four PPMs as `icons/<name>.ppm`. `cargo xtask image`
  stages `samples/icons/ppm` there for both the product and `--shell` images
  and fails when a required PPM is missing. No file is staged at the image
  root.
- **A6 (MUST):** No kernel icon codec, parser, drawing path, opcode, builtin,
  `Host` method or VFS special case is added. Loading and drawing are ordinary
  `/lib` source over `core.fs_*`, `lib.ppm` and `syslib.blit_rgb`; the existing
  boot tree self-check only lists the new `icons/` directory (spec 0025 B10).

### `lib.icons`

- **L1 (MUST):** `samples/apps/lib/icons.ig` is an ordinary `/lib` module
  (spec 0018 M5/M6): `unit icons;`, no `main`, no top-level `var` state, no
  stream endpoint. It uses `core`, `lib.ppm` and `syslib`.
- **L2 (MUST):** The public contract is:

  ```
  const
    IconSide = 32;
    IconStride = 96;       // IconSide * 3
    IconBytes = 3072;      // IconStride * IconSide
    IconCount = 4;
    FolderId = 0;
    SourceId = 1;
    FileId = 2;
    ImageId = 3;

  type Set = record
    pixels: array[bytes[IconBytes], IconCount];
    ready: array[bool, IconCount];
    loaded: int;
  end;

  procedure icons.clear(s: var Set);
  function icons.load(s: var Set; base: const string[0]) -> Status;
  function icons.ready(s: const Set; id: int) -> bool;
  function icons.draw(s: const Set; id: int; x: int; y: int;
                      fallback: uint) -> Status;
  ```

  Module constants are not exported across a unit boundary in the current
  language, so an importer repeats the fixed ids `FolderId = 0`,
  `SourceId = 1`, `FileId = 2`, `ImageId = 3`; the numeric values are the
  contract.

- **L3 (MUST):** `load` reads `folder.ppm`, `igfile.ppm`, `file.ppm` and
  `image.ppm` in that order from `base/<name>.ppm` (or `<name>.ppm` when
  `base` is empty). Each file must be a P6 image whose dimensions are exactly
  32×32; rows are read with `lib.ppm.read_row` and copied into the icon's
  raster. An icon becomes `ready` only after all 32 rows are stored, so a
  failed load never publishes a partial icon. One failed icon does not stop the
  other three; `load` returns the first failure class (`NotFound`, `Invalid`,
  `CapacityExceeded`, `IoError`) and `Ok` only when all four are ready.
- **L4 (MUST):** `draw` copies a ready icon with one
  `syslib.blit_rgb(x, y, 32, 32, pixels, 32, 32)` at the icon's top-left. An
  unknown id or an icon that is not ready draws a 32×32 monochrome fallback
  outline in `fallback` instead and returns `Ok`; a disabled screen propagates
  `Invalid` from the drawing primitive. There is no alpha and no scaling.

### `lib.iconview`

- **V1 (MUST):** `samples/apps/lib/iconview.ig` is an ordinary `/lib` module
  using `core`, `stdlib`, `syslib` and `lib.icons`. The application owns the
  view; the module owns no top-level state.
- **V2 (MUST):** The public contract is:

  ```
  const
    EntryCapacity = 96;
    NameCapacity = 64;
    IconSide = 32;
    Margin = 4;
    LabelGap = 4;
    LabelLineH = 12;
    LabelLines = 2;

  type View = record
    x, y, w, h: int;
    cell_w, cell_h: int;
    cols, rows: int;       // computed by init
    names: array[string[NameCapacity], EntryCapacity];
    kinds: array[uint, EntryCapacity];
    icon_ids: array[int, EntryCapacity];
    count: int;
    selected: int;
    top: int;              // first visible grid row
  end;

  procedure iconview.init(v: var View; x: int; y: int; w: int; h: int;
                          cell_w: int; cell_h: int);
  procedure iconview.clear(v: var View);
  function iconview.add(v: var View; name: const string[0]; kind: uint;
                        icon_id: int) -> int;
  function iconview.columns(v: const View) -> int;
  function iconview.visible_rows(v: const View) -> int;
  function iconview.total_rows(v: const View) -> int;
  function iconview.page_size(v: const View) -> int;
  function iconview.index_at(v: const View; px: int; py: int) -> int;
  function iconview.move(v: var View; dx: int; dy: int) -> bool;
  function iconview.move_to(v: var View; at: int) -> bool;
  procedure iconview.page(v: var View; rows: int);
  procedure iconview.first(v: var View);
  procedure iconview.last(v: var View);
  procedure iconview.ensure_visible(v: var View);
  procedure iconview.set_top(v: var View; top: int);
  function iconview.label_line(v: const View; at: int; line: int;
                               out: var string[NameCapacity]) -> bool;
  function iconview.draw(v: const View; s: const icons.Set) -> Status;
  ```

- **V3 (MUST):** `init` stores the rectangle and cell size and computes
  `cols = max(1, (w - 2*Margin) div cell_w)` and
  `rows = max(1, h div cell_h)`. `clear` sets `count = 0`, `selected = -1` and
  `top = 0`. `add` appends and returns the new index; it returns `-1` and adds
  nothing when the view is full or the name is longer than `NameCapacity`. The
  first added entry becomes selected.
- **V4 (MUST):** A point inside the grid maps to `at = (top + row) * cols +
  col`; a point outside the grid rectangle, in the margin, or past `count` maps
  to `-1`. A cell's top-left is `(x + Margin + col*cell_w,
  y + Margin + row*cell_h)`; its icon is centred horizontally and starts at
  `cell_y + Margin`. The exact hit rule follows V3's computed geometry, not the
  drawn glyphs.
- **V5 (MUST):** `move(dx, dy)` moves the selection by `dx` entries or `dy`
  grid rows, clamps to `[0, count-1]` and never wraps; it returns whether the
  selection changed. `move_to` clamps an absolute index. `page` moves by
  `rows * page_size` entries, `first` selects 0 and `last` selects
  `count - 1`. Every selection change calls `ensure_visible`, which scrolls
  `top` by grid rows so the selected entry is inside the visible rows.
  `set_top` clamps to `[0, total_rows - visible_rows]`.
- **V6 (MUST):** `label_line` publishes line 0 or 1 of a two-line label into
  `out` and returns whether the line is non-empty. A line holds at most
  `(cell_w - 8) div 8` glyphs. Line 0 breaks after the last space inside the
  limit when one exists, otherwise at a UTF-8 code-point boundary; line 1 takes
  the remainder up to the same limit. Text beyond two lines is cut at a
  code-point boundary, not wrapped further. `out` is untouched on an empty
  line.
- **V7 (MUST):** `draw` fills the grid rectangle white and draws a one-pixel
  black border, then draws every visible entry row by row. An unselected entry
  has a black label; the selected entry fills its two-line label band with the
  accent colour, draws the label white and draws a two-pixel accent border
  around the 32×32 icon. Labels are centred horizontally. Drawing uses only
  `syslib.fill_rect`, `syslib.draw_rect`, `syslib.draw_text` and
  `icons.draw`; a failure returns the first non-`Ok` status. A view with
  `count = 0` draws only the white rectangle and border.
- **V8 (MUST):** Every geometry, navigation and label helper is pure and runs
  on `langhost` (disabled screen) without a window; only `draw` and
  `icons.draw` need a screen.

### Browser integration

- **B1 (MUST):** `samples/apps/apps/productivity/browser.ig` loads the four
  icons once at start with `icons.load(set, "icons")` and reuses the loaded set
  for every frame. A failed load is non-fatal: the browser keeps its normal
  navigation and draws the `icons.draw` fallback. The browser never reloads
  icons per frame or per directory.
- **B2 (MUST):** The browser is a full-width icon grid: the content is the
  680×480 window, the path line is unchanged, the grid owns the body below it
  (with the existing 16-pixel scrollbar at the right) and the status line is
  unchanged. The text preview pane, `PreviewCapacity`, the preview read path
  and its UTF-8 publish are removed. No file content is read for display.
- **B3 (MUST):** The grid uses 100×68 cells with `Margin = 4`; the browser
  computes `cols` and visible rows from the view contract (6 columns and 6
  rows at 680×420 body). The listing capacity stays 96 entries and a larger
  directory still reports a truncated listing.
- **B4 (MUST):** Icon selection is:
  - kind 1 (directory) → `FolderId`;
  - kind 2 (device, socket, symlink) → `FileId`;
  - kind 0 whose name ends in `.ig` (ASCII case-insensitive), or whose name
    has no dot and whose directory is `base`, `system`, `apps` or
    `apps/<category>` (the stager drops `.ig` there, spec 0018 L3) →
    `SourceId`;
  - kind 0 whose name ends in `.png`, `.ppm`, `.jpg` or `.jpeg`
    (ASCII case-insensitive) → `ImageId`;
  - every other kind 0 → `FileId`.
- **B5 (MUST):** `Enter` and a double-click open the selected entry: kind 1 is
  entered, kind 0 is launched with `core.spawn` + `core.wait` by its full image
  path, and kind 2 is refused with the existing status line. `Ctrl+L` keeps the
  same launch meaning. `Backspace`/`Ctrl+Up` and the visible `Up` button keep
  going up; path construction stays bounded by `PATH_MAX`.
- **B6 (MUST):** The label is the two-line `iconview` label. When the status
  line is empty the browser shows the selected entry's full name and its
  size/kind at the left of the status bar; the key hint stays at the right but
  is omitted when the left text would overlap it. Long names are never
  truncated in this status text beyond the 64-byte line capacity.
- **B7 (MUST):** The keyboard map is Up/Down (one row), Left/Right (one
  entry), PageUp/PageDown (one page), Home/End, Enter (open), Backspace/Ctrl+Up
  (up), Ctrl+L (launch), N (create `home/newdir`), Delete/D (remove under home)
  and Ctrl+Q (quit). The scrollbar track pages and a held thumb still defers
  the view to the release (M16.8); the thumb geometry now uses grid rows.
- **B8 (MUST):** `browser check` proves the listing, navigation, icon mapping,
  grid geometry, label wrapping, selection/paging, scrollbar, home operations
  and launch behaviour in the kernel; `browser shot` holds the icon grid for
  the screenshot harness and prints `browser: ready`. The `langhost` path runs
  the same pure checks with no screen, no icon load and no child launch and
  prints `browser: host ok`.

### Proof and verification

- **P1 (MUST):** `samples/tests/apps/base/iconview.ig` is a test-only proof
  with the usual `check`/`shot` forms. On the host it proves the pure geometry,
  hit, navigation, scroll and label rules and prints `iconview: host ok`; in
  the kernel it draws a grid with the loaded icons, checks representative
  pixels (background, border, selection band, icon ink, label ink), presents
  and prints `iconview: ok`; `shot` prints `iconview: ready` and holds.
- **P2 (MUST):** `langhost/tests/e2e.rs` compiles and runs the browser and the
  `iconview` proof on the host and asserts their documented transcripts.
- **P3 (MUST):** `scripts/make_icons.py --check` validates the four committed
  PPMs without rewriting them; `cargo xtask image` stages them and the built
  image lists `icons/` with the four files. A `--shell` boot runs
  `browser check`, `iconview check` and `ui_icons` on `-smp 4` and `-smp 1`.

## Design

The icon set is a plain record of four 3,072-byte buffers. `load` reuses
`lib.ppm` rather than duplicating the P6 header parser, and copies one 96-byte
row at a time; the icon is opaque because PPM has no alpha. The widget is a
record of the listing plus its geometry: no allocation, no function pointers,
no hidden global state, so two views can coexist. The browser keeps the
existing directory walk, path builder, home operations and launcher; only the
listing model and the drawing change.

```
icons/                 read-only image directory
├── folder.ppm         32×32 P6, white background
├── igfile.ppm
├── file.ppm
└── image.ppm
```

```
+--------------------------------------------------+
| igis:/base            62 entries      home writable [Up] |
+--------------------------------------------------+
|  [icon]   [icon]   [icon]   [icon]   [icon]   [icon] |^|
|  shell    child    text     ...                    | |
|                                                    | |
|  ... 6 rows ...                                    |v|
+--------------------------------------------------+
| shell  1,234 B          Enter open  Ctrl+L run ... |
+--------------------------------------------------+
```

## Interfaces

- `lib.icons.Set` / `icons.load` / `icons.draw` — asset loading and blitting.
- `lib.iconview.View` / `iconview.*` — grid model, geometry, labels and drawing.
- `icons/` image directory — the only new top-level image directory since
  spec 0018 L1.
- No ABI, opcode, builtin or `Host` change.

## Failure modes

| Condition | Result |
|---|---|
| `samples/icons/ppm/<name>.ppm` missing | `icons.load` returns `NotFound`; that icon draws the fallback; the browser still works |
| PPM magic/dimensions not P6 32×32 | `Invalid`; that icon is not ready, the others load |
| PPM raster short | `Invalid` from `read_row`; the icon is cleared and not ready |
| `icons/` absent on the host | `NotFound`; the host path never draws and prints `host ok` |
| Disabled screen | `icons.draw`/`iconview.draw` propagate `Invalid`; no fault |
| Directory over 96 entries | listing truncated with the existing status line |
| Name over 64 bytes | `iconview.add` returns `-1`; names from the VFS are capped at 64 |
| Path over `PATH_MAX` | existing `path too long` status, nothing opened |

## Open questions

- More icons (the `samples/icons/scalable/` library) can be added later by
  extending the fixed list and the four-name loader; the widget itself is
  size-agnostic.
- Alpha-blended icons would need an RGBA asset format and a blending blit;
  the current PPM path is opaque by design.
