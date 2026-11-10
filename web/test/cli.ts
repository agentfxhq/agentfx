// Test helper: run the agentfx CLI (parse / timing) on a temp file.
import { execFileSync } from 'node:child_process';
import { existsSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

function findCli(): string | null {
  if (process.env.AGENTFX_BIN) return process.env.AGENTFX_BIN;
  let d = resolve(dirname(fileURLToPath(import.meta.url)), '..', '..');
  for (let i = 0; i < 8; i++) {
    const c = join(d, 'target', 'release', 'agentfx');
    if (existsSync(c)) return c;
    d = dirname(d);
  }
  return null;
}

export const CLI = findCli();

function run(args: string[], name: string, content: string): string {
  const dir = mkdtempSync(join(tmpdir(), 'afx-web-'));
  try {
    const f = join(dir, name);
    writeFileSync(f, content);
    return execFileSync(CLI!, [...args.slice(0, 1), f, ...args.slice(1)], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] });
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

export const toJson = (src: string) => JSON.parse(run(['parse', '--to', 'json'], 'in.reel', src));
export const toText = (doc: unknown) => run(['parse', '--to', 'text'], 'in.json', JSON.stringify(doc));
export const timing = (src: string) => JSON.parse(run(['timing', '--no-tts'], 'in.reel', src));
