# Saved files are the boundary with KiCad

KiCad 10 offers no supported way to read what is open but unsaved in its schematic, symbol or footprint editors. So we read only saved files and label what we show as the last saved state. We never change an open design or an editable library behind KiCad's back.

## Consequences

- A change to a project goes through KiCad itself, with the app guiding the person, or through a guarded change to a project that is closed.
- If we can't tell whether KiCad's editors have unsaved work, switching libraries waits until KiCad is closed.
- Library releases will be installed as separate read-only copies and switched in one step, never by checking out another version in a folder KiCad uses. This isn't built yet (kicad-parts-9853).

Mulch records: mx-3903cc, mx-70700f.
