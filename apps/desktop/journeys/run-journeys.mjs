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
//                                   # approved screenshot, if every journey
//                                   # passes; look at them before committing
//
// On Windows the runner first re-runs itself, with the same arguments,
// under the Node.js pinned in infra/versions.lock (journey-node.mjs). It
// downloads it once into target/journey-tools/node. Node.js before 24.16.0
// crashes now and then on a TCP connect (libuv #5106).
//
// By default tauri-driver, msedgedriver, the app and WebView2 run on a
// hidden, run-scoped Win32 desktop (tooling/journey-desktop), so no window
// appears or takes focus on the user's desktop.
//
// Every screenshot is compared with its reviewed, approved screenshot in
// baselines/. A missing one, or one that differs by more than
// VISUAL_LIMITS, fails that journey's test (so it shows in junit.xml) and
// the run; a normal run never changes baselines/.
//
// Evidence (JUnit results, screenshots, images of the differences,
// visual-summary.json, axe results, evidence.jsonl) goes to
// target/journeys/latest, or JOURNEY_OUT.

import { download } from 'edgedriver';
import { spawn, spawnSync } from 'node:child_process';
import fs from 'node:fs';
import net from 'node:net';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import {
  VISUAL_LIMITS,
  appBinary,
  baselinesDir,
  driverPort,
  outDir,
  repoRoot,
  runTempRoot,
  screenshotsDir,
  stagedBaselinesDir,
  visualProblem,
  visualResultsFile,
} from './harness.mjs';
import { NODE_CRASH_EXIT_CODE, currentNode, runUnderPinnedNode, specsKilledByNodeCrash } from './journey-node.mjs';

// First, before any TCP connection: a connection made under an affected
// Node.js can crash this runner too.
await runUnderPinnedNode(repoRoot, fileURLToPath(import.meta.url));
const testNode = currentNode();

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

// A driver left over from an earlier run would answer instead of ours. A
// run still going holds the port too, so check before emptying its folders.
if (await portInUse(driverPort)) {
  console.error(
    `port ${driverPort} is already in use, probably by a tauri-driver from an earlier run. ` +
      'Stop it (for example: taskkill /IM tauri-driver.exe /F) or set TAURI_DRIVER_PORT, then run again.',
  );
  process.exit(1);
}

fs.rmSync(outDir, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 });
fs.mkdirSync(outDir, { recursive: true });
fs.rmSync(runTempRoot, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 });
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
console.log(`Node.js ${testNode.version} (${testNode.path})`);

// git, run by the app for library repos and by journeys that make them,
// reads only an empty settings file of this run's own: never the
// developer's settings, so their hosts, keys and rewrites never apply.
const gitConfig = path.join(outDir, 'gitconfig');
fs.writeFileSync(gitConfig, '');
process.env.GIT_CONFIG_GLOBAL = gitConfig;
process.env.GIT_CONFIG_NOSYSTEM = '1';
delete process.env.GIT_SSH_COMMAND;
delete process.env.GIT_SSH;

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
    testNode.path,
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
  explainNodeCrashes(path.join(outDir, 'junit.xml'), testNode);
  const seconds = ((Date.now() - started) / 1000).toFixed(1);
  console.log(`journeys took ${seconds} s on the ${visible ? 'visible' : 'hidden'} desktop`);
  fs.appendFileSync(
    path.join(outDir, 'evidence.jsonl'),
    `${JSON.stringify({ event: 'run', mode: visible ? 'visible' : 'hidden', seconds: Number(seconds), node: testNode })}\n`,
  );
} finally {
  tauriDriver.kill();
}
try {
  fs.rmSync(runTempRoot, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 });
} catch (error) {
  console.warn(`could not delete ${runTempRoot}: ${error.message}`);
}

if (!reportVisualResults(status === 0)) status = status || 1;

const shots = fs.existsSync(path.join(outDir, 'screenshots'))
  ? fs.readdirSync(path.join(outDir, 'screenshots')).sort()
  : [];
console.log(`\nEvidence in ${outDir}`);
console.log(`  junit.xml, evidence.jsonl, accessibility/*.json, ${shots.length} screenshots:`);
for (const shot of shots) console.log(`    screenshots/${shot}`);
process.exit(status);

/**
 * Says in plain words when a spec failed because its Node.js process
 * crashed, not because of anything the journey checked. node --test only
 * reports "test failed" for it, and tauri-driver may log "os error 10054"
 * for the connection the process had just opened.
 */
function explainNodeCrashes(junitFile, testNode) {
  if (!fs.existsSync(junitFile)) return;
  const crashed = specsKilledByNodeCrash(fs.readFileSync(junitFile, 'utf8'));
  const code = `0x${NODE_CRASH_EXIT_CODE.toString(16).toUpperCase()}`;
  for (const spec of crashed) {
    console.error(
      `\n${spec}: its Node.js ${testNode.version} test process crashed (exit code ${code}, a Windows ` +
        'fast-fail); anything it printed is above. No journey check failed. ' +
        'Rerun it with JOURNEYS_ONLY. If it happens again, see mulch mx-625975.',
    );
  }
  if (crashed.length > 0) {
    fs.appendFileSync(
      path.join(outDir, 'evidence.jsonl'),
      `${JSON.stringify({ event: 'node-crash', specs: crashed, node: testNode })}\n`,
    );
  }
}

/**
 * Prints how each screenshot compared with its approved screenshot,
 * writes visual-summary.json, and returns false when any screenshot needs
 * attention.
 *
 * With --update-baselines, the screenshots staged in new-baselines/ are
 * copied into baselines/ only if every journey passed; in a run of every
 * journey, approved screenshots no journey took are then deleted. After a
 * failed run nothing in baselines/ changes.
 *
 * Without it, in a run of every journey, an approved screenshot no journey
 * took also fails: "not used" if every journey passed, "not reached" if
 * one failed first.
 */
/**
 * Says which screenshots share a number. Every file written to
 * screenshots/, compared or not, takes its own number: the next one free
 * in one sequence. One exception: "<name>-drawn", the 3D view as drawn
 * next to the compared screenshot <name>, shares that screenshot's number.
 * `names` are file names without ".png", from this run and baselines/.
 */
function numberClashes(names) {
  const all = new Set(names);
  const byNumber = new Map();
  for (const name of all) {
    const number = /^(\d+)-/.exec(name)?.[1];
    if (number === undefined) continue;
    const owner = name.endsWith('-drawn') && all.has(name.slice(0, -'-drawn'.length)) ? name.slice(0, -'-drawn'.length) : name;
    byNumber.set(number, new Set([...(byNumber.get(number) ?? []), owner]));
  }
  return [...byNumber]
    .filter(([, owners]) => owners.size > 1)
    .map(
      ([number, owners]) =>
        `These screenshots share the number ${number}: ${[...owners].sort().join(', ')}. ` +
        'Give each one its own number, the next one free.',
    );
}

/** The names, without ".png", of the screenshots in `dir`. */
function pngNames(dir) {
  return fs.existsSync(dir)
    ? fs
        .readdirSync(dir)
        .filter((file) => file.endsWith('.png'))
        .map((file) => file.slice(0, -'.png'.length))
    : [];
}

function reportVisualResults(everyJourneyPassed) {
  const results = fs.existsSync(visualResultsFile)
    ? fs
        .readFileSync(visualResultsFile, 'utf8')
        .split('\n')
        .filter(Boolean)
        .map((line) => JSON.parse(line))
    : [];
  const taken = new Set(results.map((r) => r.name));
  const everyJourney = !process.env.JOURNEYS_ONLY;
  const notTaken =
    !everyJourney || !fs.existsSync(baselinesDir)
      ? []
      : fs
          .readdirSync(baselinesDir)
          .filter((file) => file.endsWith('.png') && !taken.has(file.slice(0, -4)))
          .sort();

  const problems = numberClashes([...pngNames(screenshotsDir), ...pngNames(baselinesDir)]);
  const accepted = [];
  const deleted = [];
  if (updateBaselines) {
    if (everyJourneyPassed) {
      fs.mkdirSync(baselinesDir, { recursive: true });
      for (const r of results) {
        fs.copyFileSync(path.join(stagedBaselinesDir, `${r.name}.png`), path.join(baselinesDir, `${r.name}.png`));
        accepted.push(r.name);
      }
      for (const file of notTaken) {
        fs.rmSync(path.join(baselinesDir, file));
        deleted.push(file);
      }
    } else {
      problems.push(
        'No approved screenshot was changed, because a journey failed. Fix the failure and run ' +
          '`npm run journeys -- --update-baselines` again. The screenshots this run took are in ' +
          `${path.relative(repoRoot, stagedBaselinesDir)}, for reference only.`,
      );
    }
  } else {
    for (const r of results) {
      if (r.status === 'missing' || r.status === 'changed') problems.push(visualProblem(r));
    }
    for (const file of notTaken) {
      problems.push(
        everyJourneyPassed
          ? `baselines/${file} was not used by any journey. Delete it, or run with --update-baselines.`
          : `baselines/${file} was not compared: no journey reached it, because a journey failed.`,
      );
    }
  }
  const summary = {
    limits: VISUAL_LIMITS,
    updateBaselines,
    everyJourneyPassed,
    accepted,
    deleted,
    notTaken,
    screenshots: results,
    problems,
  };
  fs.writeFileSync(path.join(outDir, 'visual-summary.json'), JSON.stringify(summary, null, 2));

  console.log(
    `\nScreenshots compared with their approved screenshots in baselines/ ` +
      `(fail above ${VISUAL_LIMITS.MAX_DIFFERENT_PIXELS} different pixels):`,
  );
  for (const r of results) {
    const ok = r.status === 'same' || r.status === 'staged';
    const detail = r.status === 'same' ? `${r.differentPixels} pixels differ` : r.status;
    console.log(`  ${ok ? 'ok  ' : 'FAIL'} ${r.name}: ${detail}`);
  }
  if (accepted.length > 0) {
    console.log(
      `  ${accepted.length} approved screenshots written` +
        `${deleted.length ? `, ${deleted.length} no longer taken deleted (${deleted.join(', ')})` : ''}.`,
    );
  }
  for (const problem of problems) console.error(`  ${problem}`);
  return problems.length === 0;
}
