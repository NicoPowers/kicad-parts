# Manual Windows KiCad interoperability checklist

KiCad 10 is a required dependency. The application and automated tests run the installed `kicad-cli` wherever it is useful. This checklist covers what the command line cannot show: how generated data behaves when opened in KiCad's own editors.

Use a supported KiCad installation on Windows and a disposable copy of the application output. Do not point the application or a test at the user's KiCad configuration directory.

- Record the application commit/build, Windows version, KiCad version shown in the GUI, and the fixture or generated file under review.
- Generate or export the representative symbol, footprint, board, or library data with the application.
- Open the generated data through KiCad's normal GUI file-open or library-management flow.
- Confirm that KiCad reports no parse, rescue, missing-library, or format-upgrade error.
- Inspect the relevant semantics: symbol pins and fields, footprint pads and geometry, board nets/layers, and referenced 3D models as applicable.
- Save to a disposable copy, close it, reopen it in KiCad, and confirm the same content is present.
- If useful, capture screenshots and a short note with the versions and observed result.
- Record any failure as a follow-up issue. A skipped manual check must not be reported as an automated pass.

When a check can be automated with `kicad-cli`, such as parsing, upgrading, or exporting a file, automate it and drop it from this list.
