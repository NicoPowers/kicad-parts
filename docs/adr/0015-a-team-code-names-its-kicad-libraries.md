# A team code names the team's KiCad libraries

One person may work for several teams, each on its own team server with its own parts, and see every team's libraries side by side in KiCad. So each team's KiCad libraries are named with a short team code, such as `MA`: `MA-Parts`, `MA-Resistor_SMD` and the hidden `MA-res-20241209`. The dash keeps them apart from hand-made libraries named like `MA_`. Decided by the user 2026-10-17 (kicad-parts-f397).

Boards save footprint library names, and a board must work for everyone on the team. So the code belongs to the team, not the person: the team server keeps it in its team settings, and it is set once.

## Considered options

- **One fixed prefix, such as `team-`.** Rejected, because two teams' libraries would clash in KiCad on one computer.
- **A code each person chooses.** Rejected, because boards would then name libraries that teammates do not have.

## Consequences

- The code is 2 or 3 capital letters or digits, starting with a letter. Until roles exist (kicad-parts-5514), any member who can edit the Catalog chooses it, after a confirmation. `POST /api/team/code` refuses to change it; a superuser fixes a mistake before any board uses it (see `docs/operations/TRY_THE_TEAM_CATALOG.md`).
- No KiCad libraries are made until the team has a code.
- Each team's libraries live in `kicad-libraries/<CODE>/`, which records the team's settings record ID. Signing in updates only that team's folder, and a folder that belongs to another team is never written. If another team's folder already uses the code, the app stops updating and says how to fix it. It tries again only after the sign-in or the code changes; there is no override.
- The folder also records the team server it was made from, written as sign-in writes addresses (kicad-parts-092b). Every member knows the team's ID, so any server could claim it and change that team's parts. So a folder takes updates only from the server it records. If a signed-in team's server differs, the app stops updating and asks once, on that team's row in the Teams menu, whether the team moved. Only "Use the new address" records the new server.
- Folders made since kicad-parts-f397 already name the server of their last update, which was shown but not checked, so they keep working. A folder with no recorded server takes the first update from a server with its team's ID, and records that server. This trusts that server once, as every update did before. From then on, only that server writes.

Mulch records: mx-472044, mx-544722, mx-a97f8c, mx-7549a9.
