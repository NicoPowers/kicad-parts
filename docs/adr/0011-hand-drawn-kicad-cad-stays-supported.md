# Engineers can still draw CAD by hand in KiCad

Amended by ADR 0024 (2026-10-26): there is no import now; the library check compares the local library folder with the Catalog.

Engineers may keep drawing symbols and footprints with KiCad in our `g-` libraries. The library check in `crates/catalog/src/library_check/` finds the problems this can cause, and both the import and the desktop app run it. It warns about items no part uses, and it checks each footprint's 3D model link. Decided 2026-10-12.

## Consequences

- A broken model link in a new or changed footprint stops the import.
- The team server refuses an uploaded footprint whose model link points outside `${GITPLM_PARTS}/3d-models`.
- A footprint whose bytes still match today's CSV catalog keeps its old problems as clean-up work, so the check doesn't block it.

Mulch records: mx-e22fb9, mx-c05d31, mx-81e01f.
