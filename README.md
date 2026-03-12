# GitPLM Parts Project
This repository has been forked from [git-plm/parts](https://github.com/git-plm/parts).  Check it out for excellent info on IPN numbering schemes.  All that info has been stripped out in this fork to focus on the KiCad parts database and its day-to-day usage.

## Debugging broken `*.kicad_dbl` files
Sometimes when you modify the `#gplm.kicad_dbl` file, there is a typo and KiCad will no longer load it and does not give you any helpful debugging messages. You can use the [`jq`](https://github.com/jqlang/jq) command line utility to quickly find errors in the file, since the `kicad_dbl` format appears to be JSON.

`jq . \#gplm.kicad_dbl`

## Adding New Parts
If the symbol and footprint already exist, adding a new part is simple as:

1. If you are adding a new part category, create a new `csv` file, then edit `update_db.sh` to add to the list of imports.
2. Add a line to one of the `csv` files. The `csv` files should be sorted by `IPN`. This ensures the `IPN` is unique (which is the lib/db key), and merge operations are simpler if the file is always sorted.
3. run `update_db.sh`
4. No need to restart KiCad 9

If you need to add a symbol or footprint, add to the matching `g-XXX.kicad_sym`, or `g-XXX.pretty` libraries.

## Implementation details
The IPN (Internal Part Number) format used is specified in [this document](partnumbers.md).

So we use the following flow:

`CSV -> Sqlite3 -> ODBC -> KiCad`

This might seem overly complex, but it is actually pretty easy as SQLite3 can import `csv` files, so no additional tooling is required. See the [`envsetup.sh`](envsetup.sh) file for how this is done.

`csv` files can be easily edited in [LibreOffice](https://www.libreoffice.org/) or [VisiData](https://www.visidata.org/). **Note, in LibreOffice make sure you import CSV files with character set as `UTF-8` (`UTF-7`, which seems to be the default, will cause bad things to happen)**

A separate `csv` file is used for each [part category](https://github.com/git-plm/gitplm/blob/main/partnumbers.md#three-letter-category-code) (ex: `IND`, `RES`, `CAP`, etc.). There are several reasons for this:

Initially, this part database will be optimized for low-cost rapid prototyping at places like [JLCPCB](https://jlcpcb.com/) and [Seeed Studio](https://www.seeedstudio.com/fusion_pcb.html) using parts from:

- https://jlcpcb.com/Parts
- https://www.seeedstudio.com/opl.html

(this may not work out so the approach may change)

## Directories
- `database` - CSV files and the SQLite3 database file
- `symbols` - custom KiCad symbols
- `footprints` - custom KiCad footprints

## Guidelines
### Reference designators
* [KiCad Library Conventions](https://klc.kicad.org/symbol/s6/s6.1/)
* [Wikipedia](https://en.wikipedia.org/wiki/Reference_designator)