# Replace the Python prototype with a Rust, Tauri and Leptos app

The first version of this product was a Python and PyQt prototype. We rebuilt it in Rust, with Tauri for the desktop app and Leptos for its screens, instead of porting the prototype feature by feature. The CSV catalog and its CAD files are inputs to a checked import, not behaviour the new app has to copy.

## Considered options

- **Keep the prototype running until the new app matches it.** Rejected, because Python and PyQt are no longer supported. Fixed input files and an import that detects any loss keep the data safe without making the prototype's behaviour a gate.

## Consequences

- `tooling/csv-catalog-fixture` keeps a list of the CSV catalog's files, CAD links and hashes, and reports any difference.
- `tooling/catalog-import/src/zero_loss.rs` checks that an import loses nothing from the CSV catalog.
- The reasons for choosing Rust, Tauri and Leptos over other stacks were in `docs/PRODUCT_PLAN.md`, which was never committed.

Seeds plans: pl-40b7, pl-d0f7.
