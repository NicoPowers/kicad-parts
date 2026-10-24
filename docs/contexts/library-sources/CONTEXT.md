# Library Sources

Where KiCad libraries come from. Today that is our own libraries and each team's library repo, read through a library source profile. Installing, switching and rolling back releases of KiCad's own libraries will be described here too.

## Language

### Where a repo keeps its libraries

**Library source profile**:
What the app knows about one library repo at one commit: its symbol and footprint libraries (each by nickname and path in the repo), its model folders, its path variables and the folder each points to, its `.kicad_dbl` files, and a short name. People see it as the repo's library layout. The app works it out with a scan and keeps it once the person reviews it (ADR 0022).
_Avoid_: repo settings, library config, mapping, library source (alone)

**Library layout**:
What people call a library source profile in the app: "Review library layout", "Use this layout".
_Avoid_: profile (in the app), structure, schema

**Default profile**:
The library source profile of our own repo: `symbols/`, `footprints/`, `3d-models/` and `${GITPLM_PARTS}`. Every feature reads our libraries through it, so it is the one place those folders are spelled out.
_Avoid_: our layout config, built-in paths

**Scan**:
The app reading its own copy of a library repo at the newest commit, to work out the repo's library layout. It never reads the person's checkout. After each fetch the app scans again, and asks for a review only when the layout changed.
_Avoid_: index, discovery, import

**Doubt**:
Something a scan found that may be wrong, such as a link to a 3D model the repo does not have. People see doubts under "Check these".
_Avoid_: error, warning, lint

### Names

**Nickname**:
The name a library has in KiCad's library tables, such as `Acme_FP` in `Acme_FP:R_0603`. Boards and schematics store it, so the app never renames a client's libraries: nicknames stay as the repo's lib tables, or else its file names, give them.
_Avoid_: library name, alias, label

**Short name**:
A few letters or digits, such as `AC`, that tell a repo's libraries apart inside the app, such as in the Libraries browser. It starts as the team's code. KiCad and boards never see it, except in libraries the app itself makes later. In code it is the profile's prefix.
_Avoid_: team code, prefix (in the app), alias

**Path variable**:
A name such as `${ACME_3D}` in a 3D model link or a library table row. KiCad replaces it with a folder each person sets in Configure Paths, so links work on every computer. The library layout says which folder of the repo each one must point to.
_Avoid_: environment variable, env var, macro

**Model folder**:
A folder of a library repo that holds 3D models. KiCad's own layout keeps one `<library>.3dshapes` folder per footprint library inside it.
_Avoid_: 3D folder, shapes folder
