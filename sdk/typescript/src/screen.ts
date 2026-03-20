import { execFileSync } from 'child_process';
import { ensureInstalled } from './installer';

export interface ScreeningResult {
  matched: boolean;
  category: string | null;
  pattern: string | null;
  fragment: string | null;
}

const CLEAN: ScreeningResult = { matched: false, category: null, pattern: null, fragment: null };

/**
 * Screen text for prompt injection, credential exposure, PII, and exfiltration.
 */
export function screen(text: string): ScreeningResult {
  const binary = ensureInstalled();
  try {
    const output = execFileSync(binary, ['screen', '--json'], {
      encoding: 'utf-8',
      input: text,
      stdio: ['pipe', 'pipe', 'pipe'],
    });
    return JSON.parse(output);
  } catch {
    return CLEAN;
  }
}

/**
 * Screen an API request body (JSON) for injection, credentials, PII.
 */
export function screenApi(body: string): ScreeningResult {
  const binary = ensureInstalled();
  try {
    const output = execFileSync(binary, ['screen', '--api', '--json'], {
      encoding: 'utf-8',
      input: body,
      stdio: ['pipe', 'pipe', 'pipe'],
    });
    return JSON.parse(output);
  } catch {
    return CLEAN;
  }
}

/**
 * Screen a file's contents.
 */
export function screenFile(path: string): ScreeningResult {
  const binary = ensureInstalled();
  try {
    const output = execFileSync(binary, ['screen', '--json', '--file', path], {
      encoding: 'utf-8',
      stdio: ['pipe', 'pipe', 'pipe'],
    });
    return JSON.parse(output);
  } catch {
    return CLEAN;
  }
}
