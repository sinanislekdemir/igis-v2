# Golden frames

`visual.png` is the whole-screen framebuffer of `samples/apps/visual`, captured
headless through QEMU's QMP `screendump` and compared pixel for pixel by
`scripts/visual_regression.py` (spec 0009, G21; ADR 0032).

- `visual.png` is a container only. The compared data is the **decoded RGB
  image**: 1024×768 at the project's bootloader mode.
- `visual.json` records the dimensions, the SHA-256 of the raw RGB bytes, and
  one SHA-256 per 64×64 tile for a localized diff. A manifest that disagrees
  with its PNG is a hard error, so a corrupt golden cannot silently pass.
- A normal harness run never writes these files. Regenerate only for a
  deliberate, reviewed visual change:

  ```
  python3 scripts/visual_regression.py build/igis-shell.iso --update
  python3 scripts/visual_regression.py build/igis-shell.iso
  python3 scripts/visual_regression.py build/igis-shell.iso --cores 1
  ```

- `--tamper` flips one captured pixel in memory and requires the comparator to
  reject it; it is the negative check for the harness itself.
- The golden is environment-scoped: a QEMU display-format change or a
  bootloader mode change also requires a reviewed `--update`.

The specimen is a full-screen window, so both kernel text windows and every
boot line are covered and the frame is deterministic on `-smp 4` and `-smp 1`.
