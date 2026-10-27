# Working on kicad-parts

kicad-parts is a desktop app (Rust, Tauri, Leptos) that keeps a team's KiCad parts catalog on a PocketBase team server. It is a proof of concept with no users yet: no migration or backward compatibility is owed to old data.

## The people it serves

- **Users are hardware engineers, not software people.** Aim for the simplest interface: plain words, few choices, sensible defaults, a clear next step in every message.
- **One team per client.** The owner designs for many clients. Each client is its own team with its own team server, and the owner stays signed in to all of them at once.
- **Libraries live in git repos.** Each team links its own KiCad library repo, on any git host. KiCad sees the repo's own library names; boards must open with only the client's own library.

## Quality bar

Hold every change, and every review, to this:

- **Keep changes as small as the seed needs.** Refactor first, in its own commits, only where the existing shape strains. Never pile new layers on an old shape.
- **The 3 a.m. test.** Write as if you will be woken at 3 a.m. to fix it: clear names, comments that say why, small modules, tests that explain behaviour.
- **Cross-platform.** The app runs on Windows, macOS and Linux. Design nothing that works on one only.
- **Offline at run time.** The app reaches only the team servers and repos the person chose. While developing, use the internet freely.
- **Plain language** in UI text, docs, comments, commit messages, seeds and mulch: short sentences, active voice, at most 30 words each. Load the Skill `iso-24495-plain-language:iso-24495-1` before writing seeds or mulch.

## Where the plan and decisions live

- **Seeds** (`sd`) hold the work: `sd ready`, `sd show <id>`. Epics hold the owner's decisions.
- **Mulch** (`ml`) holds lessons and conventions: `ml prime --files <file>` before editing a file, `ml search <topic>`.
- **ADRs** in `docs/adr/` and glossaries in `docs/contexts/` hold domain rules and terms.
- `sd` and `ml` come from the greenhouse plugin. Run them in Bash. A subagent's shell may lack them on its PATH: give it the full path to the plugin's `bin` folder.

## How a seed gets built

1. Claim it: `sd update <id> --claim --as claude`, then branch `seed/<id>` from `main`.
2. One implementer agent per seed, one seed at a time. Brief it fully: the seed, the ADRs and files to read, the decisions already made, this quality bar, the constraints below, and the gate.
3. A separate high-effort reviewer agent reviews `git diff main..HEAD`. Send its findings back to the same implementer, so it keeps its context. Repeat until the reviewer approves.
4. Fast-forward `main`, close the seed, `sd sync`, and file follow-ups as new seeds.
5. Push only when the owner says so.

In autonomous runs, make product and UX calls yourself: choose the simplest option with the best UX, record it in the seed, ADR or mulch, and tell the owner so they can overturn it. Still ask before anything destructive or outward-facing.

## Testing

The "Fast test loop" section of `README.md` is the loop: while working, run only the journey or test file you are changing; run `tooling/gate.sh` once at the end (about 4 minutes). Inspect every new or changed journey screenshot against the approved look before accepting it.

## Guardrails

- **The owner's own library repos and KiCad setup stay untouched.** Tests use only `fixtures/sample-library` and throwaway local git repos.
- **KiCad settings:** read KiCad's config folders only to inform; write only in the app's private `KICAD_CONFIG_HOME`.
- **Secrets:** tests and journeys use in-memory sign-ins (`--sign-ins=memory`), never the real OS credential store or real app data. Never touch `secrets.env`.
- **Processes:** run hidden, bind test services to 127.0.0.1, and stop only processes you started. Leave the owner's KiCad and app windows alone.
- **Scratch files** go under `target/`; delete them when done. Windows may clear `%TEMP%`, so keep nothing there.
- **Mulch:** never run `ml doctor --fix`; it deleted valid records here. Run `ml doctor` after any merge that touches `.mulch/`.
- **Agents with worktree isolation** can't be resumed once their worktree is gone. Prefer working in the main checkout, one agent at a time.
