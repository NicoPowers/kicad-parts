# Roles and priority decide who changes what

Each member of a team has a role:

- **Owner** changes anything, including roles and priorities.
- **Engineer** changes part details and CAD items.
- **Purchaser** changes supplier data and orders.
- **Viewer** changes nothing.

When two members edit the same field, the one with the higher priority wins. On a tie, the app asks. A member who can't make a change sends a change request, and a member with a higher priority accepts or rejects it. Decided 2026-10-10.

## Consequences

- This replaces the single `users.can_edit_catalog` setting the server checks today. Not built yet (kicad-parts-5514).

Mulch records: mx-5cb241.
