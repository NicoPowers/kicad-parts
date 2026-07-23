# Manual Windows KiCad interoperability checklist

This checklist is a human, non-blocking product check. It is not part of the AFK Docker gate, CI, or an agent-run workflow, and completing the Docker lab does not claim native KiCad interoperability.

Use a supported KiCad installation on Windows and a disposable copy of the application output. Do not point the application or Docker stack at the user's KiCad configuration directory.

- Record the application commit/build, Windows version, KiCad version shown in the GUI, and the fixture or generated file under review.
- Generate or export the representative symbol, footprint, board, or library data with the application.
- Open the generated data through KiCad's normal GUI file-open or library-management flow.
- Confirm that KiCad reports no parse, rescue, missing-library, or format-upgrade error.
- Inspect the relevant semantics: symbol pins and fields, footprint pads and geometry, board nets/layers, and referenced 3D models as applicable.
- Save to a disposable copy, close it, reopen it in KiCad, and confirm the same content is present.
- If useful, capture screenshots and a short note with the versions and observed result. Keep this manual evidence separate from `.afk/runs`.
- Record any failure as a follow-up issue. A skipped or failed manual check does not block the Docker application test gate and must not be represented as an automated pass.

Do not add a hosted Linux GUI job, a Docker native-application service, or command-line native-application execution as a substitute for this checklist.
