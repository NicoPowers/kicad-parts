# One part list over every team

The person designs for many clients in one day, and each client is its own team on its own team server (ADR 0018). Switching the part list from team to team got in the way. So the part list now shows every signed-in team's parts at once, with one search over all of them. No team is "the shown team" any more. Decided 2026-10-18 by the user, built 2026-10-23 (kicad-parts-8a71).

- **A part is its team and its IPN.** Two teams may both have RES-0000-0470, and each is its own row. Selection, list focus, previews, Edit part, archive and CSV reviews all name the part by its team's server address and its IPN.
- **Every request names its team.** Each Tauri command about a team's parts takes the team's server address. A footprint drawn from a Catalog revision names its team, so its 3D view asks that team for the models.
- **One list, one search.** The search runs over each team's last-known parts in the app, with no new request to any server. Rows rank as before. Within a rank they keep catalog order, and the same IPN in two teams goes by the order of the Teams menu.
- **The CSV catalog.** The CSV row shows only while no signed-in team has its IPN. Team parts keep no record of the CSV file they came from, so which team owns this CSV folder is not known yet; the link between a team and its library repository will decide it (kicad-parts-e9a3). Until then, the CSV row's values (its file, fields, symbol and footprint) fill in a team's part only while exactly one team shows parts. So a person with one team sees the list exactly as before, and a client team never shows our CSV data. With two teams or more, our own team's rows lose the CSV fill too: their Source says "Team catalog", and the CSV table name, such as `g-res`, no longer matches a search, until kicad-parts-e9a3. Add part still counts every CSV IPN as taken, in every team.
- **Team marks.** With two teams or more showing parts, each row shows its team's code, such as `MA`, in a chip of the team's colour; a part only the CSV files have shows `CSV`. Rows in a team's own section need no mark. Colour is never the only sign. Each team gets a colour once, from a fixed palette of eight, and the list file keeps it. The header then says "Parts from 2 teams and 21 CSV files".
- **Filter and grouping.** Above the list, a Team filter offers All teams (the default) or one team, and Group by offers Category (the default) or Team. Both are kept for the next start. With one team, neither shows. The team switcher is gone.
- **Teams not reachable.** An offline team's rows stay, from the parts last seen in this run, and say "offline". Its parts can't be edited, and the part panel says why. A team that must sign in again shows no parts, and a note under the count says how many teams are hidden and where to sign in.
- **Features that work on one team.** Add part, the library check and the KiCad libraries panel use the team chosen in the filter. With All teams, each has its own Team list box, which starts on the team it used last. The library check and the KiCad libraries panel show and act on the team of the answer they show. The Libraries browser's "used by" follows the filter and lists each part with its team.
- **Add part keeps its team.** It starts on the filter's team, else the last team added to, else the first, but only while the form is empty. Once the form holds anything, or the person chose a team, the team never changes by itself: if that team can't take parts for a while, Save is off and the form says why. While two teams or more show parts, the form always says the team.

## Considered options

- **Keep the switcher, and add an "All teams" choice to it.** Rejected: every request still went to "the shown team", so an All teams view would have layered a second model over the first.
- **Search each team's server on each keystroke.** Rejected: 10 teams would mean 10 requests per keystroke, and an offline team would slow every search. The app already follows each team's catalog.
- **Rank whole teams first, then by match.** Rejected: an exact match in the second team would sit below loose matches in the first. Group by Team gives a team-first view when the person wants one.
- **Let the person pick team colours.** Left out for now: automatic colours from a palette that passes contrast need no setup.

## Consequences

- Each team's parts reach the part list, the part panel and the Libraries browser through one seam, `TeamCatalog::last_known_parts`; the panel takes the list's own row (`CatalogBrowser::part_in_list`). Today the seam reads the session's copy of the team catalog, so a team keeps its parts only while the app runs. Keeping them on disk for offline starts (kicad-parts-ed46) changes only that seam.
- The list's rows for a team are made once per change of that team and kept, so a search over 10 teams of a few thousand parts each only compares text.
- The list file (`team.json`) now keeps each team's colour, the filter, the grouping, and the team each one-team feature used last. It no longer keeps a shown team.
- Copying parts between teams (kicad-parts-1a46) and roles per team (kicad-parts-5514) can name both teams by server address, as every other request now does.
- The app has no dark theme. Each chip carries its own background and a border in its text colour, so it reads alike on a white row, a selected row and the navy header.

Mulch records: mx-c5104a, and the records of kicad-parts-8a71.
