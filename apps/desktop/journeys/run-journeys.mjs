// Builds the desktop app, starts tauri-driver with an msedgedriver that
// matches the installed WebView2 runtime, and runs every journey in specs/.
//
//   npm run journeys                # build, then run on a hidden desktop
//   npm run journeys -- --skip-build
//   JOURNEYS_ONLY=06 npm run journeys  # only specs whose names contain 06
//   npm run journeys -- --visible   # show the app windows (debugging);
//                                   # JOURNEYS_VISIBLE=1 does the same
//   npm run journeys -- --update-baselines
//                                   # accept every screenshot as its new
//                                   # baseline, after you have looked at it
//
// By default tauri-driver, msedgedriver, the app and WebView2 run on a
// hidden, run-scoped Win32 desktop (tooling/journey-desktop), so no window
// appears or takes focus on the user's desktop.
//
// Every screenshot is compared with its reviewed baseline in baselines/.
// A missing baseline, or one that differs by more than VISUAL_LIMITS, fails
// the run; a normal run never changes a baseline.
//
// Evidence (JUnit results, screenshots, diff images, visual-summary.json,
// axe results, evidence.jsonl) goes to target/journeys/latest, or
// JOURNEY_OUT.

import { download } from 'edgedriver';
import { spawn, spawnSync } from 'node:child_process';
import fs from 'node:fs';
import net from 'node:net';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import {
  VIEWPORT,
  VISUAL_LIMITS,
  appBinary,
  baselinesDir,
  driverPort,
  outDir,
  repoRoot,
  runTempRoot,
  visualResultsFile,
} from './harness.mjs';

const here = path.dirname(fileURLToPath(import.meta.url));
const skipBuild = process.argv.includes('--skip-build');
const updateBaselines = process.argv.includes('--update-baselines');
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
fs.rmSync(runTempRoot, { recursive: true, force: true });

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
    {
      cwd: here,
      stdio: 'inherit',
      env: { ...process.env, JOURNEY_OUT: outDir, JOURNEYS_UPDATE_BASELINES: updateBaselines ? '1' : '0' },
    },
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
try {
  fs.rmSync(runTempRoot, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 });
} catch (error) {
  console.warn(`could not delete ${runTempRoot}: ${error.message}`);
}

if (!reportVisualResults()) status = status || 1;

const shots = fs.existsSync(path.join(outDir, 'screenshots'))
  ? fs.readdirSync(path.join(outDir, 'screenshots')).sort()
  : [];
console.log(`\nEvidence in ${outDir}`);
console.log(`  junit.xml, evidence.jsonl, accessibility/*.json, ${shots.length} screenshots:`);
for (const shot of shots) console.log(`    screenshots/${shot}`);
process.exit(status);

/**
 * Prints how each screenshot compared with its baseline, writes
 * visual-summary.json, and returns false when any screenshot needs
 * attention. In a run of every journey, a baseline that no screenshot used
 * also fails (or, with --update-baselines, is deleted).
 */
function reportVisualResults() {
  const results = fs.existsSync(visualResultsFile)
    ? fs
        .readFileSync(visualResultsFile, 'utf8')
        .split('\n')
        .filter(Boolean)
        .map((line) => JSON.parse(line))
    : [];
  const taken = new Set(results.map((r) => r.name));
  const everyJourney = !process.env.JOURNEYS_ONLY;
  const unused = !everyJourney || !fs.existsSync(baselinesDir)
    ? []
    : fs
        .readdirSync(baselinesDir)
        .filter((file) => file.endsWith('.png') && !taken.has(file.slice(0, -4)))
        .sort();
  if (updateBaselines) {
    for (const file of unused) fs.rmSync(path.join(baselinesDir, file));
  }
  const problems = [];
  for (const r of results) {
    const percent = ((100 * r.differentPixels) / (VIEWPORT.width * VIEWPORT.height)).toFixed(3);
    if (r.status === 'missing') {
      problems.push(
        `${r.name} has no baseline. Look at screenshots/${r.name}.png; if it is right, ` +
          'accept it with `npm run journeys -- --update-baselines`.',
      );
    } else if (r.status === 'changed') {
      problems.push(
        r.reason
          ? `${r.name} does not match its baseline: ${r.reason}.`
          : `${r.name} does not match its baseline: ${r.differentPixels} pixels (${percent}%) differ, ` +
              `more than the ${VISUAL_LIMITS.MAX_DIFFERENT_PIXELS} allowed. See diffs/${r.name}.png. ` +
              'If the change is intended, look at it, then run `npm run journeys -- --update-baselines`.',
      );
    }
  }
  if (!updateBaselines) {
    for (const file of unused) {
      problems.push(`baselines/${file} was not used by any journey. Delete it, or run with --update-baselines.`);
    }
  }
  const summary = {
    limits: VISUAL_LIMITS,
    updated: updateBaselines,
    unusedBaselines: unused,
    screenshots: results,
    problems,
  };
  fs.writeFileSync(path.join(outDir, 'visual-summary.json'), JSON.stringify(summary, null, 2));

  console.log(`\nScreenshots compared with baselines/ (fail above ${VISUAL_LIMITS.MAX_DIFFERENT_PIXELS} different pixels):`);
  for (const r of results) {
    const detail = r.status === 'same' ? `${r.differentPixels} pixels differ` : r.status;
    console.log(`  ${r.status === 'same' || r.status === 'updated' ? 'ok  ' : 'FAIL'} ${r.name}: ${detail}`);
  }
  if (updateBaselines) {
    console.log(`  ${results.length} baselines written${unused.length ? `, ${unused.length} unused ones deleted` : ''}.`);
  }
  for (const problem of problems) console.error(`  ${problem}`);
  return problems.length === 0;
}
