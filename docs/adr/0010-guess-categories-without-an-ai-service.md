# Guess a new part's category from clues, without an AI service

Descriptions alone give poor guesses. So `crates/catalog/src/guess.rs` uses clues in order of how far we trust them:

1. Confirmed matches and known manufacturer part numbers (MPNs).
2. The footprint name.
3. The symbol's reference letter.
4. The nearest existing parts.
5. Supplier categories.

The app preselects the best guess with its reason and two alternatives, and the engineer always confirms. Decided 2026-10-11.

## Considered options

- **An AI service.** Rejected, because the guess must work offline and give the same answer every time.

Mulch records: mx-2c0f7a.
