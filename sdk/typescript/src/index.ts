/**
 * envpod — TypeScript SDK for the zero-trust governance layer for AI agents.
 *
 * Thin wrapper around the envpod CLI binary. Every method calls the binary
 * via child_process — no reimplementation of isolation logic.
 *
 * @example
 * ```typescript
 * import { Pod, screen } from 'envpod';
 *
 * const pod = await Pod.create('my-agent', { config: 'examples/coding-agent.yaml' });
 * await pod.run('python3 agent.py');
 * const diff = await pod.diff();
 * await pod.commit(['src/'], { rollbackRest: true });
 * await pod.destroy();
 * ```
 */

export { Pod } from './pod';
export { screen, screenApi, screenFile } from './screen';
export { ensureInstalled } from './installer';
