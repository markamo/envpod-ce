import { execSync, execFileSync } from 'child_process';
import { existsSync, readFileSync, writeFileSync, mkdirSync } from 'fs';
import { join } from 'path';
import { homedir } from 'os';
import { ensureInstalled } from './installer';

export interface PodOptions {
  config?: string;
  preset?: string;
  mode?: 'standard' | 'full';
}

export interface RunOptions {
  root?: boolean;
  env?: Record<string, string>;
  capture?: boolean;
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

  private constructor(name: string, opts: PodOptions = {}) {
    this.name = name;
    this.config = opts.config;
    this.preset = opts.preset;
    this.mode = opts.mode || getMode();
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
