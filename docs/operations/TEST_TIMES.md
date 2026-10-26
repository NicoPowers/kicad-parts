# Test times

How long the checks take, before and after seed kicad-parts-eb82. Measured on a 24-core Windows 11 PC with everything built once.

| What | Before | After |
|---|---|---|
| One journey loop: edit a spec, run that one journey | about 26 s (trunk 8 s, desktop rebuild 7 s, spec waits 15 s at the end) | 17 s (the spec itself 7.6 s) |
| Build for journeys when nothing changed | 16.5 s | 2.5 s |
| Every journey | 737 s | about 110 s (6 workers) |
| Rust tests (`cargo test --workspace` against `cargo nextest run`) | 339 s | 65 s |
| Full gate (`tooling/gate.sh`) | about 15 min, by hand | 4 min 48 s |

## What made it faster

- **Force-exit.** Each spec now runs as `node --test --test-force-exit`. Before, each spec process waited about 15 s after its last test, because the team server's process handle stayed open.
- **trunk only when the UI changed.** `app-build.mjs` hashes the UI crate and the crates it uses. It skips `trunk build` when the hash matches the last good build. A new `dist/` would also make cargo rebuild the desktop app.
- **Journeys side by side.** Six workers run specs at once. Each has its own `tauri-driver`, ports and hidden desktop. Specs 04, 06, 09 and 17 then run alone, because they check how fast the app answers or have a known race.
- **cargo-nextest.** It runs every Rust test in its own process, all at once.
- **A gate script.** `tooling/gate.sh` runs the checks that need no app side by side, after one build step.

## Journey workers

| Workers | Every journey |
|---|---|
| 1 | 273 to 378 s |
| 2 | 197 s |
| 4 | 117 s |
| 6 (default) | 100 to 125 s |
| 8 | 90 s |

Six is the default: eight saved only 10 s more. The runner uses at most a quarter of the cores. Screenshots from 6 workers and from 1 worker match with 0 pixels different.

## Gate steps

One run of `tooling/gate.sh` on 2026-10-26, after a change to `tooling/afk`. The steps in group 3 run side by side; the others run one after the other.

| Group | Step | Time |
|---|---|---|
| 1. Lint | fmt | 1 s |
| | clippy | 23 s |
| | clippy for wasm | 7 s |
| 2. Build | journey app | 3 s |
| | Rust tests (`nextest run --no-run`) | 65 s |
| | csv-catalog-fixture | 2 s |
| 3. Side by side | Rust tests (914 tests) | 75 s |
| | JavaScript tests | 3 s |
| | CSV fixture check | 1 s |
| | 21 pilots, one after the other | 73 s |
| | full pilot | 71 s |
| | `ml doctor` | 9 s |
| 4. Alone | journeys | 109 s |
| | **Total** | **4 min 48 s** |

The Rust test build takes most of group 2 only when Rust code changed. With nothing changed it takes a few seconds.
