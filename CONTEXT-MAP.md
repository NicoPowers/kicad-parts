# Context Map

## Contexts

- [Catalog](./docs/contexts/catalog/CONTEXT.md) — the parts a team designs with: what each one is, how it is numbered, how its record has changed, and which exact CAD it uses
- **Library Sources** — where KiCad libraries come from, and how releases of them are installed, switched, and rolled back _(glossary not written yet)_
- **KiCad Integration** — translating between the product and KiCad: saved files in, projections and previews out _(glossary not written yet)_
- **Collaboration** — the team, its members and roles, and the shared change history every context publishes through _(glossary not written yet)_
- **KiCad Projects** — what saved KiCad projects use and need _(glossary not written yet)_
- [Inventory](./docs/contexts/inventory/CONTEXT.md) — usable stock across local and partner sites, without false precision
- **Procurement** — turning material demand into reviewed supplier handoffs, and the supplier offers and prices behind them _(glossary not written yet)_
- **Design Blocks** — proven circuit blocks reused across organizations _(glossary not written yet)_

## Relationships

- **Catalog → KiCad Integration**: KiCad Integration projects Managed Parts and their exact Asset Revisions into the forms KiCad reads. KiCad never edits the Catalog directly.
- **Library Sources → Catalog**: Catalog Assets can come from a library source. The Catalog records exactly which Asset Revision each part uses.
- **KiCad Projects → Catalog**: Projects refer to Managed Parts by IPN. KiCad Projects reads saved project files through KiCad Integration.
- **Inventory → Catalog**: Inventory refers to Managed Parts by identity only. Stock never creates Part Revisions.
- **Procurement → Catalog**: Supplier offers and prices attach to Managed Parts and their Approved Manufacturer Parts. They are operational facts and never create Part Revisions.
- **Procurement ↔ Inventory**: Procurement fills demand that Inventory cannot. Once supply is received into Inventory, Procurement stops counting it as inbound.
- **Design Blocks → Catalog**: Design Blocks refer to Managed Parts and Catalog Assets.
- **Collaboration ↔ every context**: Collaboration provides membership, roles, and the protected path through which each context publishes changes.
