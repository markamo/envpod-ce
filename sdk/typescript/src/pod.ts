import { execSync, execFileSync } from 'child_process';
import { existsSync, readFileSync, writeFileSync, mkdirSync } from 'fs';
import { join } from 'path';
import { homedir } from 'os';
import { ensureInstalled } from './installer';

export interface PodOptions {
  config?: string;
  preset?: string;
  mode?: 'standard' | 'full';
  mountCwd?: boolean;
}

export interface RunOptions {
  root?: boolean;
  env?: Record<string, string>;
  capture?: boolean;
  display?: boolean;
  audio?: boolean;
  background?: boolean;
}

export interface CommitOptions {
  exclude?: string[];
  output?: string;
  rollbackRest?: boolean;
}

export interface ResizeOptions {
  cpus?: number;
  memory?: string;
  tmpSize?: string;
  maxPids?: number;
  gpu?: boolean;
}

export class Pod {
  readonly name: string;
  private config?: string;
  private preset?: string;
  private mode: string;
  private binary: string;

  private mountCwd: boolean;

  private constructor(name: string, opts: PodOptions = {}) {
    this.name = name;
    this.config = opts.config;
    this.preset = opts.preset;
    this.mode = opts.mode || getMode();
    this.mountCwd = opts.mountCwd !== false; // default true
    this.binary = ensureInstalled();
  }

  /**
   * Create a new pod with setup.
   */
  static async create(name: string, opts: PodOptions = {}): Promise<Pod> {
    const pod = new Pod(name, opts);
    if (!pod.exists()) {
      pod.init();
    }
    return pod;
  }

  /**
   * Create a pod, run a callback, then destroy + gc.
   * Node.js equivalent of Python's context manager.
   *
   * @example
   * await Pod.with('my-agent', { config: 'pod.yaml' }, async (pod) => {
   *   pod.run('python3 agent.py');
   *   pod.commit(['src/'], { rollbackRest: true });
   * });
   */
  static async with(name: string, opts: PodOptions, fn: (pod: Pod) => void | Promise<void>): Promise<void> {
    const pod = await Pod.create(name, opts);
    try {
      await fn(pod);
    } finally {
      pod.destroy();
      Pod.gc();
    }
  }

  /**
   * Wrap an existing pod (no init).
   */
  static wrap(name: string, opts: PodOptions = {}): Pod {
    return new Pod(name, opts);
  }

  /**
   * Initialize the pod (create + setup).
   */
  init(opts?: { verbose?: boolean }): void {
    const args = ['init', this.name];
    if (this.config) args.push('-c', this.config);
    else if (this.preset) args.push('--preset', this.preset);
    if (opts?.verbose) args.push('--verbose');
    this.exec(args);
  }

  /**
   * Run a command inside the pod.
   */
  run(command: string, opts: RunOptions = {}): string | void {
    const args = ['run', this.name];
    if (opts.root) args.push('--root');
    if (this.mountCwd) args.push('-w');
    if (opts.display) args.push('-d');
    if (opts.audio) args.push('-a');
    if (opts.background) args.push('-b');
    if (opts.env) {
      for (const [k, v] of Object.entries(opts.env)) {
        args.push('--env', `${k}=${v}`);
      }
    }
    args.push('--', 'sh', '-c', command);
    if (opts.capture) {
      return this.exec(args, true);
    }
    this.exec(args);
  }

  /**
   * Run inline code inside the pod.
   * Writes to a temp file in the overlay, executes, cleans up.
   */
  runScript(code: string, opts: RunOptions & { interpreter?: string } = {}): string | void {
    const interp = opts.interpreter || 'python3';
    const encoded = Buffer.from(code).toString('base64');
    const cmd = `echo "${encoded}" | base64 -d > /tmp/.envpod-script && ${interp} /tmp/.envpod-script && rm -f /tmp/.envpod-script`;
    return this.run(cmd, opts);
  }

  /**
   * Copy a local file into the pod and run it.
   * Auto-detects interpreter from file extension.
   */
  runFile(path: string, opts: RunOptions & { interpreter?: string } = {}): string | void {
    const { readFileSync } = require('fs');
    const code = readFileSync(path, 'utf-8');
    const ext = path.split('.').pop()?.toLowerCase();
    const interp = opts.interpreter || ({
      py: 'python3', js: 'node', ts: 'npx tsx',
      sh: 'bash', rb: 'ruby', pl: 'perl',
    } as Record<string, string>)[ext || ''] || 'bash';
    return this.runScript(code, { ...opts, interpreter: interp });
  }

  /**
   * Mount a host directory into the pod (COW isolated).
   */
  mount(path: string, readonly: boolean = true): void {
    const args = ['mount', this.name, path];
    if (readonly) args.push('--readonly');
    this.exec(args);
  }

  /**
   * Copy a local file into the pod's overlay.
   */
  inject(localPath: string, podPath: string = '/tmp/', executable: boolean = false): void {
    const { readFileSync } = require('fs');
    const { basename } = require('path');
    const content = readFileSync(localPath);
    const encoded = content.toString('base64');
    const name = basename(localPath);
    const dest = podPath.endsWith('/') ? `${podPath}${name}` : podPath;
    let cmd = `echo "${encoded}" | base64 -d > ${dest}`;
    if (executable) cmd += ` && chmod +x ${dest}`;
    this.run(cmd, { root: true });
  }

  /**
   * Show filesystem changes.
   */
  diff(opts?: { all?: boolean; json?: boolean }): string {
    const args = ['diff', this.name];
    if (opts?.all) args.push('--all');
    if (opts?.json) args.push('--json');
    return this.exec(args, true);
  }

  /**
   * Commit overlay changes to host.
   */
  commit(paths: string[] = [], opts: CommitOptions = {}): void {
    const args = ['commit', this.name];
    if (opts.rollbackRest) args.push('--rollback-rest');
    if (opts.output) args.push('--output', opts.output);
    if (opts.exclude) {
      for (const e of opts.exclude) args.push('--exclude', e);
    }
    args.push(...paths);
    this.exec(args);
  }

  /**
   * Discard all overlay changes.
   */
  rollback(): void {
    this.exec(['rollback', this.name]);
  }

  /**
   * Show audit log or security analysis.
   */
  audit(opts?: { security?: boolean; json?: boolean }): string {
    const args = ['audit', this.name];
    if (opts?.security) args.push('--security');
    if (opts?.json) args.push('--json');
    return this.exec(args, true);
  }

  /**
   * Show pod status.
   */
  status(): string {
    return this.exec(['status', this.name], true);
  }

  /**
   * Start pod in background.
   */
  start(): void {
    this.exec(['start', this.name]);
  }

  /**
   * Stop pod.
   */
  stop(): void {
    this.exec(['stop', this.name]);
  }

  /**
   * Freeze pod.
   */
  lock(): void {
    this.exec(['lock', this.name]);
  }

  /**
   * Resume frozen pod.
   */
  unlock(): void {
    this.exec(['unlock', this.name]);
  }

  /**
   * Restart the pod (stop + start).
   */
  restart(): void {
    this.exec(['restart', this.name]);
  }

  /**
   * Terminate pod processes and rollback changes.
   */
  kill(): void {
    this.exec(['kill', this.name]);
  }

  /**
   * Remove pod entirely.
   */
  destroy(): void {
    try {
      this.exec(['destroy', this.name]);
    } catch {
      // Pod may already be destroyed
    }
  }

  /**
   * Resize pod resources.
   */
  resize(opts: ResizeOptions): void {
    const args = ['resize', this.name];
    if (opts.cpus !== undefined) args.push('--cpus', String(opts.cpus));
    if (opts.memory) args.push('--memory', opts.memory);
    if (opts.tmpSize) args.push('--tmp-size', opts.tmpSize);
    if (opts.maxPids !== undefined) args.push('--max-pids', String(opts.maxPids));
    if (opts.gpu !== undefined) args.push('--gpu', String(opts.gpu));
    this.exec(args);
  }

  /**
   * Store a secret in the vault.
   */
  vaultSet(key: string, value: string): void {
    this.exec(['vault', this.name, 'set', key, value]);
  }

  /**
   * Create a snapshot of the pod's current overlay state.
   */
  snapshotCreate(name?: string): void {
    const args = ['snapshot', this.name, 'create'];
    if (name) args.push('--name', name);
    this.exec(args);
  }

  /**
   * Restore a snapshot.
   */
  snapshotRestore(name: string): void {
    this.exec(['snapshot', this.name, 'restore', name]);
  }

  /**
   * List all snapshots.
   */
  snapshotList(): string {
    return this.exec(['snapshot', this.name, 'ls'], true);
  }

  /**
   * Delete a snapshot.
   */
  snapshotDestroy(name: string): void {
    this.exec(['snapshot', this.name, 'destroy', name]);
  }

  /**
   * Show pod output logs.
   */
  logs(): string {
    return this.exec(['logs', this.name], true);
  }

  /**
   * Get pod info (name, status, IP, display URL, etc.).
   */
  info(): Record<string, any> {
    try {
      const output = this.exec(['ls', '--json'], true);
      const pods = JSON.parse(output) as Array<Record<string, any>>;
      return pods.find(p => p.name === this.name) || {};
    } catch {
      return {};
    }
  }

  /**
   * Get the noVNC display URL if web display is enabled.
   */
  get displayUrl(): string | null {
    const info = this.info();
    return info.display_url || info.novnc_url || null;
  }

  /**
   * Get the pod's IP address.
   */
  get ip(): string | null {
    const info = this.info();
    return info.ip || info.pod_ip || null;
  }

  /**
   * Create pod and save as base for fast cloning.
   */
  initWithBase(opts?: { verbose?: boolean; baseName?: string }): void {
    const args = ['init', this.name];
    if (opts?.baseName) {
      args.push(`--create-base=${opts.baseName}`);
    } else {
      args.push('--create-base');
    }
    if (this.config) args.push('-c', this.config);
    else if (this.preset) args.push('--preset', this.preset);
    if (opts?.verbose) args.push('--verbose');
    this.exec(args);
  }

  /**
   * Clone a pod from a base (fast — ~8ms).
   */
  static clone(source: string, name: string, opts: PodOptions = {}): Pod {
    const pod = new Pod(name, opts);
    pod.exec(['clone', source, name]);
    return pod;
  }

  /**
   * Clone from base, run command, optionally save results, destroy.
   * The fastest governed execution — ~8ms clone, run, commit, destroy.
   * Equivalent to `docker run --rm` but with governance.
   */
  static disposable(base: string, name: string, command: string, opts: {
    commitPaths?: string[];
    output?: string;
    mode?: 'standard' | 'full';
    root?: boolean;
    env?: Record<string, string>;
  } = {}): string | null {
    const pod = Pod.clone(base, name, { mode: opts.mode });
    try {
      pod.run(command, { root: opts.root, env: opts.env });
      if (opts.commitPaths) {
        const diff = pod.diff();
        pod.commit(opts.commitPaths, { output: opts.output, rollbackRest: true });
        return diff;
      }
      return null;
    } finally {
      pod.destroy();
    }
  }

  /**
   * Clean up orphaned resources (iptables, cgroups, netns).
   */
  static gc(): void {
    const binary = ensureInstalled();
    try {
      execFileSync('sudo', [binary, 'gc'], { stdio: 'pipe' });
    } catch {}
  }

  /**
   * Check if pod exists.
   */
  exists(): boolean {
    try {
      const output = this.exec(['ls', '--json'], true);
      const pods = JSON.parse(output) as Array<{ name: string }>;
      return pods.some(p => p.name === this.name);
    } catch {
      return false;
    }
  }

  private exec(args: string[], capture?: true): string;
  private exec(args: string[], capture?: false): void;
  private exec(args: string[], capture: boolean = false): string | void {
    const cmd = this.mode === 'full'
      ? ['sudo', this.binary, ...args]
      : [this.binary, ...args];

    if (capture) {
      return execFileSync(cmd[0], cmd.slice(1), {
        encoding: 'utf-8',
        stdio: ['pipe', 'pipe', 'pipe'],
      });
    }
    execFileSync(cmd[0], cmd.slice(1), {
      stdio: 'inherit',
    });
  }
}

function getMode(): string {
  // Check environment variable
  const envMode = process.env.ENVPOD_MODE;
  if (envMode === 'standard' || envMode === 'full') return envMode;

  // Check saved config
  const configDir = join(homedir(), '.config', 'envpod');
  const configFile = join(configDir, 'sdk.json');
  if (existsSync(configFile)) {
    try {
      const config = JSON.parse(readFileSync(configFile, 'utf-8'));
      if (config.mode) return config.mode;
    } catch {}
  }

  // Interactive prompt
  const readline = require('readline');
  const rl = readline.createInterface({ input: process.stdin, output: process.stderr });

  console.error('\n  envpod — choose isolation mode:');
  console.error('    [1] Standard — full governance, no sudo needed');
  console.error('        (no cgroup limits, no network namespace)');
  console.error('    [2] Full — complete isolation + governance (requires sudo)');
  console.error('        (cgroup limits, network namespace, DNS filtering)\n');

  // Synchronous prompt via spawnSync
  const result = require('child_process').spawnSync('bash', ['-c', 'read -p "  Choice [1/2]: " c; echo $c'], {
    stdio: ['inherit', 'pipe', 'inherit'],
    encoding: 'utf-8',
  });
  const choice = (result.stdout || '').trim();
  rl.close();

  const mode = choice === '1' ? 'standard' : 'full';

  // Save preference
  try {
    mkdirSync(configDir, { recursive: true });
    writeFileSync(configFile, JSON.stringify({ mode }));
    console.error(`  Saved to ${configFile}`);
  } catch {}

  return mode;
}
