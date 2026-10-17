# KiCad 10 is required, and kicad-cli does what KiCad already does

Everyone who uses this app already has KiCad installed, and CI runners will too. So KiCad 10 is required, and the app, its tests and CI may all run it. Wherever KiCad already does a job, we call the installed `kicad-cli` instead of rebuilding it. Symbol and footprint previews are its SVG exports, and the 3D view is its GLB export.

## Considered options

- **Draw symbols and footprints with our own code.** Rejected, because it copies KiCad's drawing, drifts from what KiCad shows, and costs far more than calling `kicad-cli`.
- **Keep KiCad out of automation and check by hand on Windows.** Rejected, because it forced us to redraw parts ourselves and left working with KiCad untested.

## Consequences

- Each preview is exported the first time someone views that item, then cached by library hash, KiCad version and theme. One item takes about 0.3 to 0.6 seconds. Exporting all 35 libraries up front took about 11 seconds.
- `kicad-cli` runs only through `run_kicad_cli` in `crates/kicad-integration/src/process.rs`, with its own settings folder, so it never changes the person's KiCad settings.
- Tests run against a real KiCad 10. A manual checklist covers only what `kicad-cli` can't show.

Mulch records: mx-f05667, mx-8513c9, mx-9926b9.
