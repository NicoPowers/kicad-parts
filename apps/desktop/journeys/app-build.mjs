// Builds what the journeys run, and only what changed.
//
// cargo already rebuilds only what changed: with nothing changed, its four
// builds take about 1.5 s together. trunk does not: every `trunk build`
// rewrites dist/ (about 8 s), and a new dist/ makes cargo rebuild the
// desktop app, which embeds it (about 7 s more). So trunk runs only when one
// of its inputs changed since its last successful build. Its inputs are
// the UI crate's folder, the folders of the crates it uses (found with
// `cargo metadata`), and the workspace files every cargo build reads.
//
// The stamp target/journey-tools/trunk-inputs.sha256 holds a hash of the
// content of every input file, written only after trunk succeeds. It is
// deleted before trunk starts, so a failed build always builds again.
// Content, not file times: switching git branches back and forth changes
// file times but not content.
//
// The stamp also holds a hash of dist/ as trunk left it. Other tools write
// dist/ too: `cargo tauri dev` and `cargo tauri build` run trunk their own
// way, and apps/desktop/build.rs (run by clippy) writes a placeholder page
// when dist/ is missing. Embedding either could fail every journey, so
// trunk runs again whenever dist/ is not exactly what it made.

import { spawnSync } from 'node:child_process';
import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';

/**
 * Builds the UI (if its inputs changed), the desktop app and the helper
 * programs the journeys start. Exits the process if a build fails.
 * @param {{ repoRoot: string, hiddenDesktop: boolean }} options
 */
export function buildForJourneys({ repoRoot, hiddenDesktop }) {
  const uiDir = path.join(repoRoot, 'apps', 'desktop-ui');
  const stamp = path.join(repoRoot, 'target', 'journey-tools', 'trunk-inputs.sha256');
  const dist = path.relative(repoRoot, path.join(uiDir, 'dist'));
  const inputsHash = hashFiles(repoRoot, trunkInputs(repoRoot));
  // Two lines: the hash of trunk's inputs, then the hash of what it made.
  const stamped = fs.existsSync(stamp) ? fs.readFileSync(stamp, 'utf8') : '';
  if (stamped === `${inputsHash}\n${hashFiles(repoRoot, [dist])}\n`) {
    console.log('> trunk build skipped: no UI input changed since its last build, and dist/ is as it left it');
  } else {
    fs.rmSync(stamp, { force: true });
    run('trunk', ['build'], uiDir, repoRoot);
    fs.mkdirSync(path.dirname(stamp), { recursive: true });
    fs.writeFileSync(stamp, `${inputsHash}\n${hashFiles(repoRoot, [dist])}\n`);
  }
  run('cargo', ['build', '--locked', '-p', 'kicad-parts-desktop', '--features', 'custom-protocol'], repoRoot, repoRoot);
  run('cargo', ['build', '--locked', '-p', 'fake-kicad-cli'], repoRoot, repoRoot);
  run('cargo', ['build', '--locked', '-p', 'catalog-import'], repoRoot, repoRoot);
  if (hiddenDesktop) run('cargo', ['build', '--locked', '-p', 'journey-desktop'], repoRoot, repoRoot);
}

function run(command, args, cwd, repoRoot) {
  console.log(`> ${command} ${args.join(' ')}  (in ${path.relative(repoRoot, cwd) || '.'})`);
  const result = spawnSync(command, args, { cwd, stdio: 'inherit' });
  if (result.status !== 0) {
    console.error(`${command} failed with ${result.status ?? result.error?.message}`);
    process.exit(result.status || 1);
  }
}

/**
 * The files and folders whose content decides what `trunk build` makes,
 * relative to the repository root.
 */
function trunkInputs(repoRoot) {
  const workspaceFiles = ['Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml', path.join('.cargo', 'config.toml')];
  return [...workspaceFiles, ...uiCrateFolders(repoRoot)];
}

/**
 * The folder of the UI crate and of every crate in this repository it
 * uses, directly or not. `--no-deps` lists only workspace crates, so it
 * needs no network and takes well under a second.
 */
function uiCrateFolders(repoRoot) {
  const result = spawnSync('cargo', ['metadata', '--no-deps', '--format-version', '1', '--locked'], {
    cwd: repoRoot,
    encoding: 'utf8',
    maxBuffer: 64 * 1024 * 1024,
  });
  if (result.status !== 0) {
    console.error(`cargo metadata failed:\n${result.stderr}`);
    process.exit(result.status || 1);
  }
  const packages = new Map(JSON.parse(result.stdout).packages.map((p) => [p.name, p]));
  const folders = new Set();
  const visit = (name) => {
    const crate = packages.get(name);
    const folder = path.relative(repoRoot, path.dirname(crate.manifest_path));
    if (folders.has(folder)) return;
    folders.add(folder);
    for (const dependency of crate.dependencies) {
      if (dependency.path && packages.has(dependency.name)) visit(dependency.name);
    }
  };
  visit('kicad-parts-desktop-ui');
  return [...folders].sort();
}

/**
 * A SHA-256 over the relative path and content of every file under
 * `entries`, in a fixed order. Build output (dist/, target/, node_modules/)
 * inside an entry is left out; a missing entry counts as empty.
 */
function hashFiles(repoRoot, entries) {
  const skipped = new Set(['dist', 'target', 'node_modules']);
  const files = [];
  const walk = (relative) => {
    const full = path.join(repoRoot, relative);
    if (!fs.existsSync(full)) return;
    if (fs.statSync(full).isDirectory()) {
      for (const name of fs.readdirSync(full)) {
        if (!skipped.has(name)) walk(path.join(relative, name));
      }
    } else {
      files.push(relative);
    }
  };
  for (const entry of entries) walk(entry);
  const hash = crypto.createHash('sha256');
  for (const file of files.sort()) {
    hash.update(`${file.split(path.sep).join('/')}\0`);
    hash.update(fs.readFileSync(path.join(repoRoot, file)));
    hash.update('\0');
  }
  return hash.digest('hex');
}
