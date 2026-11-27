unit icons;

// The four 32x32 RGB icon assets (spec 0028; ADR 0062). Ordinary `/lib` module
// source under the M5/M6 rules: no `main`, no top-level `var` state, no stream
// endpoint and no kernel service. Loading and drawing are plain `/lib` source
// over `core.fs_*`, `lib.ppm` and `syslib.blit_rgb`; no new opcode, builtin or
// `Host` method exists for icons.
//
//   var set: icons.Set;
//   st := icons.load(set, "icons");     // the read-only image directory
//   ...
//   st := icons.draw(set, 0, x, y, black);   // 0 is the folder id
//
// The set is four 3,072-byte rasters: PPM P6 has no alpha and the icons are
// never scaled, so a draw is one opaque 32x32 `syslib.blit_rgb` call. `load`
// reuses `lib.ppm` instead of duplicating the P6 header parser and copies one
// 96-byte row at a time into the icon's raster. An icon becomes `ready` only
// after all 32 rows are stored, so a failed load never publishes a partial
// icon; one failed icon does not stop the other three.
//
// Failure rule: `load` returns the first failing class (`NotFound`, `Invalid`,
// `CapacityExceeded`, `IoError`) in file order and `Ok` only when all four
// icons are ready. `draw` never fails for a missing icon: an unknown id or a
// not-ready icon draws a monochrome fallback outline in the caller's colour.
// Only a disabled screen makes `draw` return `Invalid`, propagated from the
// drawing primitive.

uses core, lib.ppm, syslib;

const
  IconSide = 32;
  IconStride = 96;       // IconSide * 3
  IconBytes = 3072;      // IconStride * IconSide
  IconCount = 4;
  FolderId = 0;
  SourceId = 1;
  FileId = 2;
  ImageId = 3;

  PathCapacity = 256;    // `ppm.open` decoder path capacity
  ErrorCapacity = 24;    // one `<name>.ppm` file name

type Set = record
  pixels: array[bytes[IconBytes], IconCount];
  ready: array[bool, IconCount];
  loaded: int;           // icons ready after the last successful loads
end;

// The file name of one icon id, in loader order. `false` for an unknown id.
function file_name(id: int; name: var string[ErrorCapacity]): bool;
begin
  if id = FolderId then
    name := "folder.ppm"
  else if id = SourceId then
    name := "igfile.ppm"
  else if id = FileId then
    name := "file.ppm"
  else if id = ImageId then
    name := "image.ppm"
  else
    return false;
  return true;
end;

// `base/name`, or just `name` when `base` is empty, into a 256-byte image path.
// A result that does not fit is `CapacityExceeded` and writes nothing.
function join_path(base: const string[0]; name: const string[ErrorCapacity];
                   out: var string[PathCapacity]): Status;
var
  base_len: int;
  name_len: int;
  total: int;
begin
  base_len := int(core.len(base));
  name_len := int(core.len(name));
  if base_len = 0 then
    total := name_len
  else
  begin
    total := base_len + 1 + name_len;
    if total > PathCapacity then
      return CapacityExceeded;
  end;
  if base_len = 0 then
    out := name
  else
    out := base + "/" + name;
  return Ok;
end;

// Loads one icon. The raster and `ready` flag are cleared first, so a missing
// file, a bad header, a wrong size or a short raster never leaves a partial
// icon behind. `Ok` means all 32 rows are stored and the icon is ready.
function load_one(s: var Set; id: int; base: const string[0]): Status;
var
  path: string[PathCapacity];
  name: string[ErrorCapacity];
  d: ppm.Decoder;
  row: bytes[16384];
  w, h, got_w: uint;
  y: int;
  st: Status;
begin
  s.ready[id] := false;
  s.pixels[id] := "";
  if not file_name(id, name) then
    return Invalid;
  st := join_path(base, name, path);
  if st <> Ok then
    return st;
  st := ppm.open(path, d);
  if st <> Ok then
    return st;
  w := uint(0);
  h := uint(0);
  st := ppm.info(d, w, h);
  if st <> Ok then
    return st;
  if w <> uint(IconSide) or h <> uint(IconSide) then
    return Invalid;
  y := 0;
  while y < IconSide do
  begin
    st := ppm.read_row(d, uint(y), row, got_w);
    if st <> Ok then
    begin
      // A short raster is never partially published.
      s.pixels[id] := "";
      return st;
    end;
    if got_w <> uint(IconSide) then
    begin
      s.pixels[id] := "";
      return Invalid;
    end;
    st := core.copy_bytes(
      s.pixels[id], uint(y) * uint(IconStride), row, uint(0), uint(IconStride));
    if st <> Ok then
    begin
      s.pixels[id] := "";
      return st;
    end;
    y := y + 1;
  end;
  s.ready[id] := true;
  return Ok;
end;

// Clears every raster and flag.
procedure clear(s: var Set);
var
  i: int;
begin
  i := 0;
  while i < IconCount do
  begin
    s.pixels[i] := "";
    s.ready[i] := false;
    i := i + 1;
  end;
  s.loaded := 0;
end;

// Loads `folder.ppm`, `igfile.ppm`, `file.ppm` and `image.ppm` in that order
// from `base/<name>.ppm` (or `<name>.ppm` when `base` is empty). One failed
// icon does not stop the others; the first failure class is returned.
function load(s: var Set; base: const string[0]): Status;
var
  id: int;
  st: Status;
  first: Status;
begin
  first := Ok;
  s.loaded := 0;
  id := 0;
  while id < IconCount do
  begin
    st := load_one(s, id, base);
    if st = Ok then
      s.loaded := s.loaded + 1
    else if first = Ok then
      first := st;
    id := id + 1;
  end;
  return first;
end;

// Whether icon `id` was fully loaded. An out-of-range id is not ready.
function ready(s: const Set; id: int): bool;
begin
  if id < 0 or id >= IconCount then
    return false;
  return s.ready[id];
end;

// A 32x32 monochrome outline for a missing or unknown icon.
function draw_fallback(x: int; y: int; color: uint): Status;
begin
  return syslib.draw_rect(x, y, IconSide, IconSide, color);
end;

// Copies a ready icon at `(x, y)` with one 32x32 blit. An unknown id or a
// not-ready icon draws the fallback outline and still returns `Ok`; a disabled
// screen propagates `Invalid`.
function draw(s: const Set; id: int; x: int; y: int;
              fallback: uint): Status;
begin
  if id >= 0 and id < IconCount and s.ready[id] then
    return syslib.blit_rgb(
      x, y, IconSide, IconSide, s.pixels[id], IconSide, IconSide);
  return draw_fallback(x, y, fallback);
end;
