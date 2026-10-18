// Runs the JavaScript unit tests that need no app, no GPU and no team
// server, under the same pinned Node.js as the journeys (journey-node.mjs),
// so every machine runs them the same way: the 3D view's camera rules
// (apps/desktop-ui/js/*.test.mjs) and the team server's pure rules
// (infra/pocketbase/tests/*.test.mjs).
//
//   npm run js-tests --prefix apps/desktop/journeys

import { spawnSync } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { currentNode, runUnderPinnedNode } from './journey-node.mjs';

const here = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(here, '../../..');

await runUnderPinnedNode(repoRoot, fileURLToPath(import.meta.url));
const node = currentNode();
console.log(`Node.js ${node.version} (${node.path})`);
// node --test expands the glob itself; a folder would not work.
const run = spawnSync(node.path, ['--test', 'apps/desktop-ui/js/*.test.mjs', 'infra/pocketbase/tests/*.test.mjs'], {
  cwd: repoRoot,
  stdio: 'inherit',
});
if (run.error) {
  console.error(`could not start ${node.path}: ${run.error.message}`);
  process.exit(1);
}
process.exit(run.status ?? 1);
