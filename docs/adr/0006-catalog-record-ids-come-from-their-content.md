# Catalog record IDs come from their content

Each Catalog record's ID is the first 15 hex digits of `SHA-256("kicad-parts:<kind>:v1\n<key>")`, where the key is something stable, such as the part's IPN. So importing the same CSV again finds the same records instead of making new ones. The desktop and the server use the same rule: `crates/catalog/src/ids.rs` and `infra/pocketbase/pb_hooks/catalog_lib.js`.

## Consequences

- Changing the rule would change every ID, so both copies of the rule must always match.

Mulch records: mx-0075f5.
