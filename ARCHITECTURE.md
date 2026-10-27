# KiCad Parts: architecture

This is a short map of the code. Terms follow the glossaries in [CONTEXT-MAP.md](CONTEXT-MAP.md). Decisions and their reasons are in [docs/adr/](docs/adr/).

## What it is

A desktop app for hardware engineers who design in KiCad 10. Each client is a team with its own team server, a PocketBase that holds the team's Catalog: its parts, their numbers, their history and the exact CAD each revision uses. The app lists every signed-in team's parts in one list, edits and adds parts, and writes each team's KiCad libraries so KiCad places the team's parts.

```
team server (PocketBase + pb_hooks)  <--HTTPS-->  desktop app (Tauri + Leptos)  -->  KiCad libraries
        ^                                                |
        |                                                +--> kicad-cli previews and 3D views
   migrations set up                                     +--> local library folder (our g- libraries)
   the starting categories                               +--> each team's linked library repo (git)
```

Every write goes through a protected route on the team server, in one transaction, and saves one Change with a reason. Nothing writes Catalog records directly (ADR 0003).

## Folders

| Folder | What it holds |
|---|---|
| `apps/desktop` | The Tauri shell: wires the use cases to the UI. Launch options are for tests and unusual installs. |
| `apps/desktop-ui` | The Leptos UI, compiled to WebAssembly. |
| `apps/desktop/journeys` | End-to-end journeys that drive the built app on a hidden desktop, with screenshots compared against approved ones. |
| `crates/catalog` | The Catalog: parts, details, revisions, CAD items, categories (with the starting categories), the category guess, the library check, and the client for the team server's routes. |
| `crates/catalog-browser` | The app's use cases without Tauri: the part list, part panel, Add part, Edit part, previews, the Libraries browser, the 3D view, teams and their sign-ins, linked library repos. |
| `crates/collaboration` | The PocketBase client: sign-in, protected commands, realtime changes. |
| `crates/desktop-contract` | The messages between the shell and the UI. |
| `crates/kicad-integration` | Finds kicad-cli, reads KiCad library files and tables, draws previews, and describes where a repo keeps its libraries (library source profiles). |
| `crates/kicad-libraries` | Writes a team's KiCad libraries: a database library with one entry per part revision, and the files it uses. |
| `infra/pocketbase` | The team server: migrations, hooks (the protected routes and write guards) and their JavaScript tests. |
| `fixtures/sample-library` | A small made-up KiCad library and its parts, for tests, journeys and trying the app. |
| `fixtures/category-guess` | The clues of 178 parts the category guess is measured on. |
| `tooling/local-team` | Starts a disposable local team server from the sample library, and writes KiCad libraries without the app. Its tests run the routes and the use cases against real team servers. |
| `tooling/sample-library` | Reads the sample library, makes throwaway git repos of it, and adds its parts to a team server. |
| `tooling/native-services` | Fetches pinned PocketBase and MinIO builds and runs them windowless on 127.0.0.1 for tests. |
| `tooling/gate.sh` | Every check a change must pass, timed. |

## Where parts and libraries come from

- **Parts** live on each team's server. A new team server starts with the 21 starting categories (ADR 0024). People add parts with Add part, from downloaded CAD files, KiCad's own libraries, an installed library or our own libraries.
- **The local library folder** (`--catalog-root`) holds our own `g-` libraries. The library check, Add part from our libraries, the Libraries browser's Ours and the 3D view read it. It has no default; making each team's linked library repo its folder is kicad-parts-ff3c.
- **A team's library repo** is a git repo the person links in the Teams menu (ADR 0021). The app reads it through a library source profile (ADR 0022).

## Tests

- Rust tests: `cargo nextest run --workspace` (or `cargo test`). Team server tests start their own PocketBase on 127.0.0.1.
- JavaScript tests of the team server's rules and the 3D view: `npm run js-tests --prefix apps/desktop/journeys`.
- Journeys: `npm run journeys --prefix apps/desktop/journeys`. They run on Windows only.
- Run one test file or journey while you work; run `tooling/gate.sh` once at the end. See the README's fast test loop.
