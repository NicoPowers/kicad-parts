# The sample library

A small, made-up KiCad 10 library for tests. Every test of a feature that
stays after the CSV catalog goes (kicad-parts-e257) reads this library, never
`database/`, `symbols/`, `footprints/` or `3d-models/`.

It is laid out like a library repo, so a test can link it to a team as a
client's repo:

- `sym-lib-table` and `fp-lib-table` name every library, as a client repo's
  lib tables do.
- `symbols/g-*.kicad_sym` are the symbol libraries.
- `footprints/g-*.pretty` are the footprint libraries.
- `3d-models/g-*.3dshapes` hold the 3D models, next to their footprint
  library's name.
- `parts.toml` lists the parts a test team server starts with.

The libraries use the Catalog's own names: `g-` libraries and 3D model links
through `${GITPLM_PARTS}`. A team server keeps every symbol, footprint and
3D model under those names, and Add part sends library files byte for byte.
So the same files work for the library check, Add part, the Libraries
browser and a seeded team server.

## How tests use it

- **A folder of files.** `sample_library::root()` is this folder. Drawing,
  preview and real-KiCad tests read it directly. They only read.
- **A throwaway git repo.** `sample_library::SampleRepo::new()` copies the
  library into `target/test-folders` and commits it, with git reading no
  settings of yours. Tests that change files, need committed files or link a
  repo use it. The folder goes when the test ends.
- **A seeded team server.** `sample_library::seed(&admin, &["res", "cap"])`
  sets up those categories and adds their parts, as Add part does. An empty
  list seeds everything. It never uses the CSV import.
- **By hand.** `cargo run --locked -p catalog-import -- local-team-server
  --sample --desktop` starts a team server with every sample part.

## Where each file came from

Every symbol and footprint was written for this repository and saved by
KiCad 10.0.7's `kicad-cli sym upgrade` and `fp upgrade`, so the files are
exactly as KiCad writes them. `kicad-cli` draws every one.

- `3d-models/g-Capacitor_SMD.3dshapes/C_0603_1608Metric.step` is a 1.6 mm
  by 0.8 mm by 0.8 mm box, made by `kicad-cli pcb export step --board-only`
  from a board of that size.
- `3d-models/g-Package_TO_SOT_SMD.3dshapes/SOT-23-5.wrl` is a box written
  by hand in VRML.

Nothing is copied from KiCad's own libraries. Parts that use KiCad's own
symbols and footprints, such as `Device:R`, name them; the files stay in
your KiCad installation.

## The categories

| Code | Name | Fields |
| --- | --- | --- |
| `res` | Resistors | Description, Value, Tolerance, Datasheet |
| `cap` | Capacitors | Description, Value, Voltage, Material, Tolerance, Datasheet |
| `ics` | Integrated circuits | Description, Datasheet |
| `mcu` | Microcontrollers | Description, Datasheet |
| `reg` | Voltage regulators | Description, Voltage, Current, Datasheet |
| `art` | Board features | Description, Comments |
| `pcb` | Bare boards | Description, Project link |

Names, IPN prefixes and the rules that guess a category come from the app's
starting categories in `crates/catalog/src/category.rs`. Resistors and
capacitors have different fields on purpose.

## The parts

| IPN | What it is for |
| --- | --- |
| `RES-0000-0470` | The part most tests edit: a 47 Ω resistor with our own symbol `g-res:R_US` and footprint `g-Resistor_SMD:R_0603_1608Metric`. The footprint links KiCad's own 3D model. |
| `RES-0000-1000`, `-1500`, `-1800`, `-2001` | More resistors with the same symbol and footprint, so a list has several rows, a footprint has several users and a teammate can change a row in view. |
| `RES-0000-1002` | A resistor with KiCad's own `Device:R` and `Resistor_SMD:R_0402_1005Metric`, which stay outside the Catalog. |
| `CAP-0000-0020` | A capacitor with a known MPN, `CL10C020CB8NNNC`, for Add part's "already in the Catalog" check. Our own symbol `g-cap:C` and footprint `g-Capacitor_SMD:C_0603_1608Metric`, whose 3D model is our own STEP file. |
| `CAP-0000-0100` | A second capacitor of the same kind. |
| `CAP-0003-0001` | The highest capacitor number, so Add part offers `CAP-0004-0001`. |
| `ICS-0001-0001` | A buffer whose footprint `g-Package_TO_SOT_SMD:SOT-23-5` links our own VRML 3D model. |
| `ICS-0002-0001` | A part whose symbol `Device:TP4333` is missing from its library and whose footprint library `Sample_Missing` does not exist, for preview problems. |
| `MCU-0000-0001` | A symbol with two units, `g-mcu:SAMPLE_MCU`, and two footprint choices from KiCad's own libraries. |
| `REG-0000-0001` | A voltage regulator, so a third category with its own fields is there. |
| `ART-0000-0001` | A test point whose symbol says it is not in the BOM. |
| `PCB-0000-0001` | A bare board with no symbol and no footprint. |

## Changing it

Keep it small: add a part only when a test needs something no part shows.
Write new KiCad files by hand or in KiCad, then save them with
`kicad-cli sym upgrade --force` or `fp upgrade --force` so they stay real
KiCad 10 files. Run kicad-cli with `KICAD_CONFIG_HOME` and
`KICAD_CACHE_HOME` set to folders under `target/`, so it never writes your
own KiCad settings. Then update this README and run the whole workspace
tests and the journeys.
