// Runs the 3D view's camera unit tests (apps/desktop-ui/js/*.test.mjs)
// under the same pinned Node.js as the journeys (journey-node.mjs), so
// every machine runs them the same way. They need no app and no GPU.
//
//   npm run camera-tests --prefix apps/desktop/journeys

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
const run = spawnSync(node.path, ['--test', 'apps/desktop-ui/js/*.test.mjs'], { cwd: repoRoot, stdio: 'inherit' });
if (run.error) {
  console.error(`could not start ${node.path}: ${run.error.message}`);
  process.exit(1);
}
process.exit(run.status ?? 1);
