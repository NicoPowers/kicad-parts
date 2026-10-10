// Builds the desktop app, starts tauri-driver with an msedgedriver that
// matches the installed WebView2 runtime, and runs every journey in specs/.
//
//   npm run journeys                # build, then run on a hidden desktop
//   npm run journeys -- --skip-build
//   JOURNEYS_ONLY=06 npm run journeys  # only specs whose names contain 06
//   npm run journeys -- --visible   # show the app windows (debugging);
//                                   # JOURNEYS_VISIBLE=1 does the same
//
// By default tauri-driver, msedgedriver, the app and WebView2 run on a
// hidden, run-scoped Win32 desktop (tooling/journey-desktop), so no window
// appears or takes focus on the user's desktop.
//
// Evidence (JUnit results, screenshots, axe results, evidence.jsonl) goes to
// target/journeys/latest, or JOURNEY_OUT.

import { download } from 'edgedriver';
import { spawn, spawnSync } from 'node:child_process';
import fs from 'node:fs';
import net from 'node:net';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { appBinary, driverPort, outDir, repoRoot } from './harness.mjs';

const here = path.dirname(fileURLToPath(import.meta.url));
const skipBuild = process.argv.includes('--skip-build');
const visible =
  process.argv.includes('--visible') || process.env.JOURNEYS_VISIBLE === '1' || process.platform !== 'win32';
const journeyDesktop = path.join(repoRoot, 'target', 'debug', 'journey-desktop.exe');

function run(command, args, cwd) {
  console.log(`> ${command} ${args.join(' ')}  (in ${path.relative(repoRoot, cwd) || '.'})`);
  const result = spawnSync(command, args, { cwd, stdio: 'inherit' });
  if (result.status !== 0) {
    console.error(`${command} failed with ${result.status}`);
    process.exit(result.status ?? 1);
  }
}

function webview2Version() {
  if (process.env.WEBVIEW2_VERSION) return process.env.WEBVIEW2_VERSION;
  const roots = [process.env['ProgramFiles(x86)'], process.env.ProgramFiles].filter(Boolean);
  for (const root of roots) {
    const dir = path.join(root, 'Microsoft', 'EdgeWebView', 'Application');
    if (!fs.existsSync(dir)) continue;
    const versions = fs
      .readdirSync(dir)
      .filter((name) => /^\d+\.\d+\.\d+\.\d+$/.test(name))
      .sort((a, b) => b.localeCompare(a, undefined, { numeric: true }));
    if (versions.length > 0) return versions[0];
  }
  throw new Error('WebView2 runtime not found; set WEBVIEW2_VERSION');
}

function portInUse(port) {
  return new Promise((resolve) => {
    const socket = net.connect(port, '127.0.0.1');
    socket.once('connect', () => {
      socket.end();
      resolve(true);
    });
    socket.once('error', () => {
      socket.destroy();
      resolve(false);
    });
  });
}

function waitForPort(port, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  return new Promise((resolve, reject) => {
    const attempt = () => {
      const socket = net.connect(port, '127.0.0.1');
      socket.once('connect', () => {
        socket.end();
        resolve();
      });
      socket.once('error', () => {
        socket.destroy();
        if (Date.now() > deadline) reject(new Error(`port ${port} never opened`));
        else setTimeout(attempt, 200);
      });
    };
    attempt();
  });
}

if (!skipBuild) {
  run('trunk', ['build'], path.join(repoRoot, 'apps', 'desktop-ui'));
  run('cargo', ['build', '--locked', '-p', 'kicad-parts-desktop', '--features', 'custom-protocol'], repoRoot);
  run('cargo', ['build', '--locked', '-p', 'fake-kicad-cli'], repoRoot);
  run('cargo', ['build', '--locked', '-p', 'catalog-import'], repoRoot);
  if (!visible) run('cargo', ['build', '--locked', '-p', 'journey-desktop'], repoRoot);
}
if (!fs.existsSync(appBinary)) {
  console.error(`missing ${appBinary}; run without --skip-build`);
  process.exit(1);
}

fs.rmSync(outDir, { recursive: true, force: true });
fs.mkdirSync(outDir, { recursive: true });

// A driver left over from an earlier run would answer instead of ours.
if (await portInUse(driverPort)) {
  console.error(
    `port ${driverPort} is already in use, probably by a tauri-driver from an earlier run. ` +
      'Stop it (for example: taskkill /IM tauri-driver.exe /F) or set TAURI_DRIVER_PORT, then run again.',
  );
  process.exit(1);
}
if (!visible && !fs.existsSync(journeyDesktop)) {
  console.error(
    `missing ${journeyDesktop}, which runs the journeys on a hidden desktop. ` +
      'Build it with `cargo build --locked -p journey-desktop`, run without --skip-build, or use --visible.',
  );
  process.exit(1);
}

const version = webview2Version();
const driverCache = path.join(repoRoot, 'target', 'journey-tools', 'msedgedriver');
fs.mkdirSync(driverCache, { recursive: true });
const nativeDriver = await download(version, driverCache);
console.log(`WebView2 ${version}; msedgedriver ${nativeDriver}`);

const driverArgs = ['--port', String(driverPort), '--native-driver', nativeDriver];
console.log(`mode: ${visible ? 'visible desktop' : 'hidden desktop (journey-desktop)'}`);
// Killing journey-desktop closes its job, which ends tauri-driver and
// everything it started.
// In hidden mode journey-desktop also ends everything if this runner is
// killed outright, because it watches its parent process.
const tauriDriver = visible
  ? spawn('tauri-driver', driverArgs, { stdio: ['ignore', 'inherit', 'inherit'] })
  : spawn(journeyDesktop, ['tauri-driver', ...driverArgs], { stdio: ['ignore', 'inherit', 'inherit'] });
const started = Date.now();
tauriDriver.on('error', (error) => {
  const what = visible ? 'tauri-driver (cargo install tauri-driver --locked)' : `journey-desktop (${journeyDesktop})`;
  console.error(`could not start ${what}: ${error.message}`);
  process.exit(1);
});

let status = 1;
try {
  await waitForPort(driverPort, 20_000);
  const specs = fs
    .readdirSync(path.join(here, 'specs'))
    .filter((name) => name.endsWith('.test.mjs'))
    // JOURNEYS_ONLY=06 runs only the specs whose names contain it.
    .filter((name) => !process.env.JOURNEYS_ONLY || name.includes(process.env.JOURNEYS_ONLY))
    .sort()
    .map((name) => path.join(here, 'specs', name));
  const result = spawnSync(
    process.execPath,
    [
      '--test',
      '--test-concurrency=1',
      '--test-reporter=spec',
      '--test-reporter-destination=stdout',
      '--test-reporter=junit',
      `--test-reporter-destination=${path.join(outDir, 'junit.xml')}`,
      ...specs,
    ],
    { cwd: here, stdio: 'inherit', env: { ...process.env, JOURNEY_OUT: outDir } },
  );
  status = result.status ?? 1;
  const seconds = ((Date.now() - started) / 1000).toFixed(1);
  console.log(`journeys took ${seconds} s on the ${visible ? 'visible' : 'hidden'} desktop`);
  fs.appendFileSync(
    path.join(outDir, 'evidence.jsonl'),
    `${JSON.stringify({ event: 'run', mode: visible ? 'visible' : 'hidden', seconds: Number(seconds) })}\n`,
  );
} finally {
  tauriDriver.kill();
}

const shots = fs.existsSync(path.join(outDir, 'screenshots'))
  ? fs.readdirSync(path.join(outDir, 'screenshots')).sort()
  : [];
console.log(`\nEvidence in ${outDir}`);
console.log(`  junit.xml, evidence.jsonl, accessibility/*.json, ${shots.length} screenshots:`);
for (const shot of shots) console.log(`    screenshots/${shot}`);
process.exit(status);
