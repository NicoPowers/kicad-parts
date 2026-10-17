# Context Map

Terms use plain, everyday words. Each glossary lists old or rejected terms under _Avoid_.

Decisions for every context are in [docs/adr/](./docs/adr/), numbered in one list. The contexts have no ADR folders of their own.

## Contexts

- [Catalog](./docs/contexts/catalog/CONTEXT.md) — the parts a team designs with: what each one is, its number, how its details changed, and which exact CAD items it uses
- **Library Sources** — where KiCad libraries come from, and how you install, switch and roll back their releases _(glossary not written yet)_
- **KiCad Integration** — moving data between this product and KiCad: saved files in, library data and previews out _(glossary not written yet)_
- [Collaboration](./docs/contexts/collaboration/CONTEXT.md) — the team, its members and roles, and the shared change history every context saves through
- **KiCad Projects** — what saved KiCad projects use and need _(glossary not written yet)_
- [Inventory](./docs/contexts/inventory/CONTEXT.md) — the parts in stock at your own sites and at partners
- **Procurement** — turning demand into reviewed supplier orders, and the supplier data behind them _(glossary not written yet)_
- **Design Blocks** — proven circuit blocks reused across organizations _(glossary not written yet)_

## Relationships

- **Catalog → KiCad Integration**: KiCad Integration turns parts and their exact CAD item revisions into what KiCad reads. KiCad never edits the Catalog directly.
- **Library Sources → Catalog**: A CAD item can come from a library source. The Catalog records exactly which revision each part uses.
- **KiCad Projects → Catalog**: Projects refer to parts by IPN. KiCad Projects reads saved project files through KiCad Integration.
- **Inventory → Catalog**: Inventory refers to parts by their identity only. Stock never creates a revision.
- **Procurement → Catalog**: Supplier data attaches to parts and their manufacturer parts. It never creates a revision.
- **Procurement ↔ Inventory**: Procurement covers demand that stock cannot. When supply arrives in Inventory, Procurement stops counting it as inbound.
- **Design Blocks → Catalog**: Design blocks refer to parts and CAD items.
- **Collaboration ↔ every context**: Collaboration provides membership, roles and the protected path each context uses to save changes.
