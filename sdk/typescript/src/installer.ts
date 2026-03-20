import { execSync } from 'child_process';
import { existsSync } from 'fs';

const INSTALL_URL = 'https://envpod.dev/install.sh';

/**
 * Ensure envpod binary is available. Auto-install if missing.
 * @returns Path to the envpod binary.
 */
export function ensureInstalled(): string {
  // Check platform
  const platform = process.platform;
  if (platform === 'darwin') {
    throw new Error(
      'envpod requires Linux. On macOS, use OrbStack:\n' +
      '  brew install orbstack\n' +
      '  orb create ubuntu envpod-vm\n' +
      '  orb shell envpod-vm\n' +
      '  curl -fsSL https://envpod.dev/install.sh | sudo bash'
    );
  }
  if (platform === 'win32') {
    throw new Error(
      'envpod requires Linux. On Windows, use WSL2:\n' +
      '  wsl --install Ubuntu-24.04\n' +
      '  wsl\n' +
      '  curl -fsSL https://envpod.dev/install.sh | sudo bash'
    );
  }

  // Check common paths
  const paths = ['/usr/local/bin/envpod', '/usr/bin/envpod'];
  for (const p of paths) {
    if (existsSync(p)) return p;
  }

  // Try which
  try {
    const result = execSync('which envpod', { encoding: 'utf-8' }).trim();
    if (result) return result;
  } catch {}

  // Auto-install
  console.error('envpod binary not found. Installing...\n');
  try {
    execSync(`curl -fsSL ${INSTALL_URL} | sudo bash`, { stdio: 'inherit' });
  } catch {
    throw new Error(
      `Auto-install failed. Install manually:\n  curl -fsSL ${INSTALL_URL} | sudo bash`
    );
  }

  for (const p of paths) {
    if (existsSync(p)) return p;
  }
  throw new Error('envpod installed but not found in PATH');
}
