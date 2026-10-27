# KiCad Parts
A desktop app for hardware engineers who design in KiCad 10. Each team keeps its parts on its own team server. The app lists, edits and adds the team's parts, and writes the team's KiCad libraries. It started as a fork of [git-plm/parts](https://github.com/git-plm/parts), which explains the IPN numbering scheme it uses.

## Try the desktop app
The app (Rust and Tauri, in `apps/`, `crates/` and `tooling/`) is a prototype. Each team keeps its parts on its own team server. To try it, you start a team server that runs only on your computer, so nothing in the repository changes. You need Rust, [trunk](https://trunkrs.dev/) and KiCad 10.

Build it once, and again after each update:

```bash
cd apps/desktop-ui
trunk build
cd ../..
cargo build --locked -p kicad-parts-desktop --features custom-protocol
```

Start a team server and open the app:

```bash
cargo run --locked -p catalog-import -- local-team-server --desktop
```

The team server starts with the 21 starting categories and the 16 made-up parts of the [sample library](fixtures/sample-library/README.md). The app uses that library as its local library folder. The console prints three accounts and their passwords, which are new each time. Sign in at the top right of the app. Close the app to stop the server and delete its data.

See [Try the team catalog by hand](docs/operations/TRY_THE_TEAM_CATALOG.md) for what to try.

### Checks before a change is done
Run the gate from the repository root once, at the end. It builds what changed, runs every check below and times each one. All must pass.

```bash
tooling/gate.sh
```

It writes one line per check to `target/gate/summary.txt`, and each check's output to `target/gate/<check>.txt`. It takes about 4 minutes. On macOS and Linux it skips the journeys, which need WebView2 on Windows, and says so. These are the checks it runs, if you want to run one by hand:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo clippy -q -p kicad-parts-desktop-ui --target wasm32-unknown-unknown --locked -- -D warnings
cargo test --workspace --locked --no-fail-fast
npm run js-tests --prefix apps/desktop/journeys
npm run journeys --prefix apps/desktop/journeys
ml doctor
```

The JavaScript tests check the 3D view's camera rules and the team server's own rules, under the journeys' pinned Node.js.

### Fast test loop
While you work, run only the test you are changing. Run the gate once at the end.

```bash
# One journey: the specs whose names contain 06. It rebuilds only what changed.
JOURNEYS_ONLY=06 npm run journeys --prefix apps/desktop/journeys

# One Rust test file, or one test by name
cargo test --locked -p <crate> --test <file name without .rs>
cargo test --locked -p <crate> <part of the test name>
```

With cargo-nextest on your PATH (the gate downloads it into `target/tools/nextest/`), `cargo nextest run -p <crate> -E 'test(<part of the name>)'` does the same and runs tests side by side.

The journeys run 6 at a time by default. Add `-- --workers=1` to run them one at a time. Timings are in [Test times](docs/operations/TEST_TIMES.md).

## Part numbers
The IPN (Internal Part Number) format is specified in [this document](partnumbers.md).

## Guidelines
### Reference designators
* [KiCad Library Conventions](https://klc.kicad.org/symbol/s6/s6.1/)
* [Wikipedia](https://en.wikipedia.org/wiki/Reference_designator)