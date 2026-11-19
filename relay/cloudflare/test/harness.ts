/** Starts the Worker under `wrangler dev` (local workerd) for a test file. */

import { spawn, type ChildProcess } from 'node:child_process';
import { join } from 'node:path';

export interface LocalWorker {
  readonly base: string;
  /** Stops the process; the persisted state stays for another `startWorker` on the same folder. */
  stop(): Promise<void>;
}

/** `wrangler dev` on a free-ish port with `vars` and Durable Object state in `state`, once /healthz answers. */
export async function startWorker(state: string, vars: Record<string, string>): Promise<LocalWorker> {
  const port = 8799 + Math.floor(Math.random() * 400);
  const base = `127.0.0.1:${port}`;
  const varArgs = Object.entries(vars).flatMap(([key, value]) => ['--var', `${key}:${value}`]);
  const worker: ChildProcess = spawn(
    'npx',
    ['wrangler', 'dev', '--ip', '127.0.0.1', '--port', String(port), '--persist-to', state, '--show-interactive-dev-session=false', ...varArgs],
    { cwd: join(import.meta.dirname, '..'), env: { ...process.env, WRANGLER_SEND_METRICS: 'false', CI: '1' }, stdio: ['ignore', 'pipe', 'pipe'] },
  );
  let output = '';
  worker.stdout?.on('data', (chunk: Buffer) => { output += chunk.toString(); });
  worker.stderr?.on('data', (chunk: Buffer) => { output += chunk.toString(); });
  const deadline = Date.now() + 90_000;
  for (;;) {
    try {
      if ((await fetch(`http://${base}/healthz`)).status === 200) break;
    } catch {
      // not up yet
    }
    if (Date.now() > deadline || worker.exitCode !== null) {
      worker.kill('SIGTERM');
      throw new Error(`wrangler dev did not start:\n${output}`);
    }
    await new Promise((resolve) => setTimeout(resolve, 300));
  }
  return {
    base,
    stop: () =>
      new Promise((resolve) => {
        if (worker.exitCode !== null) return resolve();
        worker.once('exit', () => resolve());
        worker.kill('SIGTERM');
        setTimeout(() => { worker.kill('SIGKILL'); resolve(); }, 5000);
      }),
  };
}
