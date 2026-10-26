// Builds what changed in the desktop app, starts tauri-driver with an
// msedgedriver that matches the installed WebView2 runtime, and runs every
// journey in specs/, several side by side.
//
//   npm run journeys                # build what changed, then run on hidden desktops
//   npm run journeys -- --skip-build
//   npm run journeys -- --build-only  # build what changed, run nothing
//                                   # (tooling/gate.sh builds this way first)
//   JOURNEYS_ONLY=06 npm run journeys  # only specs whose names contain 06
//   npm run journeys -- --workers=1 # one spec at a time (JOURNEYS_WORKERS=1 too)
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
// Building: cargo rebuilds only what changed, and trunk runs only when a UI
// input changed (app-build.mjs), so a run after editing only specs starts
// in about 2 s.
//
// Running: DEFAULT_WORKERS workers run specs side by side, each with its
// own tauri-driver on its own hidden, run-scoped Win32 desktop
// (tooling/journey-desktop), so no window appears or takes focus on the
// user's desktop. The specs in RUN_ALONE check how fast the app answers;
// they run one at a time after the others, so other apps do not slow them.
// See journey-workers.mjs for what keeps specs apart, and
// docs/operations/TEST_TIMES.md for the timings behind these choices.
//
// Every screenshot is compared with its reviewed, approved screenshot in
// baselines/. A missing one, or one that differs by more than
// VISUAL_LIMITS, fails that journey's test (so it shows in junit.xml) and
// the run; a normal run never changes baselines/.
//
// Evidence (JUnit results, screenshots, images of the differences,
// visual-summary.json, axe results, evidence.jsonl, each spec's console
// output in logs/) goes to target/journeys/latest, or JOURNEY_OUT.

import { download } from 'edgedriver';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { buildForJourneys } from './app-build.mjs';
import {
  VISUAL_LIMITS,
  appBinary,
  baselinesDir,
  outDir,
  repoRoot,
  runTempRoot,
  screenshotsDir,
  stagedBaselinesDir,
  visualProblem,
  visualResultsFile,
} from './harness.mjs';
import { NODE_CRASH_EXIT_CODE, currentNode, runUnderPinnedNode, specsKilledByNodeCrash } from './journey-node.mjs';
import {
  checkSpecNumbers,
  countTests,
  mergeSpecOutputs,
  runQueue,
  runSpecFile,
  startWorker,
} from './journey-workers.mjs';

/**
 * How many specs run side by side by default, at most a quarter of the
 * cores (see chooseWorkerCount). Measured on a 24-core PC
 * (docs/operations/TEST_TIMES.md): 6 workers took 100 s, 8 only 10 s less.
 */
const DEFAULT_WORKERS = 6;

/**
 * Specs that run alone, one at a time after the others, so other apps do
 * not slow them and their limits stay as strict as before:
 * - 04 and 17 check how fast the app answers (search within 500 ms; a
 *   close-up within 5 s, a zoom step within 500 ms).
 * - 06 waits only 5 s for some live updates and menus.
 * - 09 has a known race (kicad-parts-ca8e) that a busy PC makes likelier.
 */
const RUN_ALONE = ['04-', '06-', '09-', '17-'];

// First, before any TCP connection: a connection made under an affected
// Node.js can crash this runner too.
await runUnderPinnedNode(repoRoot, fileURLToPath(import.meta.url));
const testNode = currentNode();

const here = path.dirname(fileURLToPath(import.meta.url));
const skipBuild = process.argv.includes('--skip-build');
const buildOnly = process.argv.includes('--build-only');
const updateBaselines = process.argv.includes('--update-baselines');
const visible =
  process.argv.includes('--visible') || process.env.JOURNEYS_VISIBLE === '1' || process.platform !== 'win32';
const journeyDesktop = path.join(repoRoot, 'target', 'debug', 'journey-desktop.exe');
const workerCount = chooseWorkerCount();
/** Each spec's seconds in the last runs; the longest start first. */
const timesFile = path.join(repoRoot, 'target', 'journey-tools', 'journey-seconds.json');

if (!skipBuild) buildForJourneys({ repoRoot, hiddenDesktop: !visible });
if (buildOnly) process.exit(0);
if (!fs.existsSync(appBinary)) {
  console.error(`missing ${appBinary}; run without --skip-build`);
  process.exit(1);
}
if (!visible && !fs.existsSync(journeyDesktop)) {
  console.error(
    `missing ${journeyDesktop}, which runs the journeys on a hidden desktop. ` +
      'Build it with `cargo build --locked -p journey-desktop`, run without --skip-build, or use --visible.',
  );
  process.exit(1);
}

const specNames = fs
  .readdirSync(path.join(here, 'specs'))
  .filter((name) => name.endsWith('.test.mjs'))
  .sort();
checkSpecNumbers(specNames);
const chosen = specNames.filter((name) => !process.env.JOURNEYS_ONLY || name.includes(process.env.JOURNEYS_ONLY));
if (chosen.length === 0) {
  console.error(`No spec name contains JOURNEYS_ONLY=${process.env.JOURNEYS_ONLY}.`);
  process.exit(1);
}

// Two runs at once would empty each other's folders.
const lock = takeRunLock(`${outDir}.lock`);
fs.rmSync(outDir, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 });
fs.mkdirSync(outDir, { recursive: true });
fs.rmSync(runTempRoot, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 });

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

const together = chosen.filter((name) => !RUN_ALONE.some((prefix) => name.startsWith(prefix)));
const alone = chosen.filter((name) => RUN_ALONE.some((prefix) => name.startsWith(prefix)));
const workersNeeded = Math.max(1, Math.min(workerCount, together.length));
console.log(
  `mode: ${visible ? 'visible desktop' : 'hidden desktops (journey-desktop)'}; ` +
    `${chosen.length} specs, ${workersNeeded} side by side` +
    (alone.length > 0 ? `, then ${alone.map((n) => n.slice(0, 2)).join(', ')} alone` : ''),
);

const started = Date.now();
const bySpecDir = path.join(outDir, 'by-spec');
const lastTimes = readTimes(timesFile);
const results = new Map();
let workers = [];
let status = 1;
try {
  workers = await Promise.all(
    Array.from({ length: workersNeeded }, (_, i) => startWorker(i + 1, { visible, journeyDesktop, nativeDriver })),
  );
  const runSpec = async (worker, name) => {
    const result = await runSpecFile({
      testNode,
      spec: path.join(here, 'specs', name),
      specOut: path.join(bySpecDir, stem(name)),
      worker,
      cwd: here,
      env: { ...process.env, JOURNEYS_UPDATE_BASELINES: updateBaselines ? '1' : '0' },
      live: workersNeeded === 1,
    });
    results.set(name, result);
  };
  // Longest first, so no long spec starts last while the others wait.
  const longestFirst = [...together].sort((a, b) => (lastTimes[b] ?? 999) - (lastTimes[a] ?? 999));
  await runQueue(workers, longestFirst, runSpec);
  for (const name of alone) await runSpec(workers[0], name);
  status = [...results.values()].every((r) => r.status === 0) && results.size === chosen.length ? 0 : 1;
} catch (error) {
  console.error(error.message);
} finally {
  for (const worker of workers) worker.stop();
}

mergeSpecOutputs(bySpecDir, chosen.map(stem), outDir);
writeTimes(timesFile, lastTimes, results);
const junit = fs.readFileSync(path.join(outDir, 'junit.xml'), 'utf8');
explainNodeCrashes(junit, testNode);
const seconds = ((Date.now() - started) / 1000).toFixed(1);
fs.appendFileSync(
  path.join(outDir, 'evidence.jsonl'),
  `${JSON.stringify({
    event: 'run',
    mode: visible ? 'visible' : 'hidden',
    seconds: Number(seconds),
    workers: workersNeeded,
    specSeconds: Object.fromEntries([...results].map(([name, r]) => [stem(name), Number(r.seconds.toFixed(1))])),
    node: testNode,
  })}\n`,
);
try {
  fs.rmSync(runTempRoot, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 });
} catch (error) {
  console.warn(`could not delete ${runTempRoot}: ${error.message}`);
}

if (!reportVisualResults(status === 0)) status = status || 1;

const shots = fs.existsSync(screenshotsDir) ? fs.readdirSync(screenshotsDir).sort() : [];
const failedSpecs = chosen.filter((name) => results.get(name)?.status !== 0);
const counts = countTests(junit);
console.log(`\nEvidence in ${outDir}`);
console.log(`  junit.xml, evidence.jsonl, logs/*.txt, accessibility/*.json, ${shots.length} screenshots`);
for (const name of failedSpecs) {
  console.error(`  FAILED ${name}: ${results.has(name) ? `see logs/${stem(name)}.txt` : 'it did not run'}`);
}
console.log(
  `journeys: ${counts.tests} tests, ${counts.passed} passed, ${counts.failed} failed, ${counts.skipped} skipped; ` +
    `${chosen.length} specs in ${seconds} s with ${workersNeeded} worker(s) on the ${visible ? 'visible' : 'hidden'} desktop`,
);
lock.release();
process.exit(status);

/** "06-team-catalog" for "06-team-catalog.test.mjs". */
function stem(name) {
  return name.replace(/\.test\.mjs$/, '');
}

/**
 * --workers=N, else JOURNEYS_WORKERS, else DEFAULT_WORKERS (1 with
 * visible windows, which would take focus from each other). At most a
 * quarter of the cores: each worker runs the app, WebView2, a team server
 * and kicad-cli at once.
 */
function chooseWorkerCount() {
  const given = process.argv.find((arg) => arg.startsWith('--workers='))?.slice('--workers='.length) ?? process.env.JOURNEYS_WORKERS;
  if (given !== undefined) {
    const count = Number(given);
    if (!Number.isInteger(count) || count < 1) {
      console.error(`--workers needs a whole number of 1 or more, not ${given}`);
      process.exit(1);
    }
    return count;
  }
  if (visible) return 1;
  return Math.max(1, Math.min(DEFAULT_WORKERS, Math.floor(os.availableParallelism() / 4)));
}

/**
 * Makes `file` hold this process's ID, unless a run that is still going
 * holds it. A lock left by a run that ended without releasing it is taken
 * over.
 */
function takeRunLock(file) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  const holder = fs.existsSync(file) ? Number(fs.readFileSync(file, 'utf8')) : 0;
  if (holder && holder !== process.pid && processIsRunning(holder)) {
    console.error(`Another journey run (process ${holder}) is using ${outDir}. Wait for it, or set JOURNEY_OUT.`);
    process.exit(1);
  }
  fs.writeFileSync(file, String(process.pid));
  return { release: () => fs.rmSync(file, { force: true }) };
}

function processIsRunning(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    return error.code === 'EPERM';
  }
}

/** Spec file name to seconds; empty when there is no file yet or it is damaged. */
function readTimes(file) {
  try {
    return JSON.parse(fs.readFileSync(file, 'utf8'));
  } catch {
    return {};
  }
}

/** Keeps the latest seconds of every spec that still exists. */
function writeTimes(file, times, results) {
  const updated = {};
  for (const name of specNames) {
    const seconds = results.get(name)?.seconds ?? times[name];
    if (seconds !== undefined) updated[name] = Number(seconds.toFixed(1));
  }
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, `${JSON.stringify(updated, null, 2)}\n`);
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

/**
 * Says in plain words when a spec failed because its Node.js process
 * crashed, not because of anything the journey checked. node --test only
 * reports "test failed" for it, and tauri-driver may log "os error 10054"
 * for the connection the process had just opened.
 */
function explainNodeCrashes(junitXml, testNode) {
  const crashed = specsKilledByNodeCrash(junitXml);
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
