import { spawn } from 'node:child_process';
import { createInterface } from 'node:readline';
import { mkdirSync, existsSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { hostname } from 'node:os';
import { setTimeout as sleep } from 'node:timers/promises';
import { projectRoot } from './config.mjs';
import { findWindowMatch } from './cli-settings.mjs';

export const DESKTOP_TOOLS = ['desktop.observe', 'desktop.screenshot', 'desktop.video', 'desktop.click', 'desktop.click_at', 'desktop.drag', 'desktop.type', 'desktop.key', 'desktop.hold_key', 'desktop.scroll', 'desktop.sequence', 'desktop.sustain', 'desktop.release'];
export const LOCAL_TOOLS = ['local.list', 'local.read', 'local.write', 'local.run'];

export function buildDesktopTools(allowDesktop, allowLocal)
{
  return [...(allowDesktop ? DESKTOP_TOOLS : []), ...(allowLocal ? LOCAL_TOOLS : [])];
}

export function resolveExe(root = projectRoot)
{
  const exe = join(root, 'Qwen27.exe');
  if (!existsSync(exe)) throw Error('Qwen27.exe is missing. Build the native client first.');
  return exe;
}

export async function listWindows({ root = projectRoot, spawnImpl = spawn } = {})
{
  const exe = resolveExe(root);
  return new Promise((yes, no) => {
    const child = spawnImpl(exe, ['--api', 'windows'], { cwd: root, windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'] });
    let out = '', err = '';
    child.stdout.on('data', d => { out += d.toString('utf8'); });
    child.stderr.on('data', d => { err += d.toString('utf8'); });
    child.on('error', no);
    child.on('close', code => {
      for (const line of out.split(/\r?\n/))
      {
        if (!line.trim()) continue;
        try { const msg = JSON.parse(line); if (msg.type === 'result') return yes(msg.value || []); if (msg.type === 'error') return no(Error(msg.message)); }
        catch {}
      }
      no(Error(err.trim() || `windows failed (exit ${code})`));
    });
  });
}

export async function resolveTargetWindow(query, opts)
{
  if (!query) throw Error('PC screen control requires --window <title|hwnd>');
  const matched = findWindowMatch(await listWindows(opts), query);
  if (!matched) throw Error(`Target window not found: ${query}`);
  return matched;
}

function createWorkerCaller(exe, args, { root, spawnImpl, onProgress })
{
  const child = spawnImpl(exe, args, { cwd: root, windowsHide: true, stdio: ['pipe', 'pipe', 'pipe'] });
  const rl = createInterface({ input: child.stdout });
  let pending = null;
  rl.on('line', line => {
    if (!line.trim()) return;
    let msg;
    try { msg = JSON.parse(line); } catch { return; }
    if (msg.progress !== undefined) { onProgress?.(msg.progress); return; }
    if (pending) { const res = pending; pending = null; res.resolve(msg); }
  });
  child.on('close', code => {
    if (pending) { const res = pending; pending = null; res.reject(Error(`Desktop worker exited (${code})`)); }
  });
  return {
    perform(action)
    {
      return new Promise((resolve, reject) => {
        if (child.exitCode !== null) return reject(Error('Desktop worker closed'));
        pending = { resolve, reject };
        child.stdin.write(JSON.stringify(action) + '\n');
      });
    },
    kill() { rl.close(); try { child.stdin?.end(); } catch {} try { child.kill(); } catch {} },
  };
}

export function createDesktopBridge({ client, hwnd = 0, winTitle = '', allowDesktop = false, allowLocal = false, workdir = '', readRoots = [], root = projectRoot, spawnImpl = spawn })
{
  let sessionId = '', token = '', stopped = false, worker = null, loopPromise = null;
  let activeAction = null;
  return {
    get sessionId() { return sessionId; },
    async open()
    {
      const tools = buildDesktopTools(allowDesktop, allowLocal);
      const res = await client.request('/desktop/sessions', { tools, ...(winTitle ? { windowTitle: winTitle } : {}) });
      sessionId = res.id; token = res.token;
      return { id: sessionId, token };
    },
    start(workspaceId = 'default')
    {
      if (!sessionId || stopped) return;
      const args = ['--target', String(allowDesktop ? hwnd : 0)];
      if (allowLocal)
      {
        const ws = workdir ? resolve(workdir) : join(root, 'local-workspaces', workspaceId);
        mkdirSync(ws, { recursive: true });
        args.push('--root', ws);
        for (const r of readRoots) args.push('--read-root', resolve(r));
      }
      worker = createWorkerCaller(resolveExe(root), args, {
        root, spawnImpl,
        onProgress: output => {
          if (!stopped && activeAction)
            client.request(`/desktop/${sessionId}/progress`, { token, actionId: activeAction, output }).catch(() => {});
        },
      });
      loopPromise = (async () => {
        while (!stopped)
        {
          try
          {
            const poll = await client.request(`/desktop/${sessionId}/poll`, { token }, { timeoutMs: 25000 });
            if (stopped) break;
            if (poll?.action)
            {
              const action = poll.action;
              activeAction = action.id;
              let result;
              try
              {
                const t = String(action.tool || '');
                if ((t.startsWith('local.') && !allowLocal) || (t.startsWith('desktop.') && !allowDesktop))
                  throw Error('このPC操作は許可されていません');
                result = await worker.perform(action);
              }
              catch (e) { result = { error: e.message }; }
              activeAction = null;
              if (!stopped) await client.request(`/desktop/${sessionId}/result`, { token, actionId: action.id, result });
            }
            else await sleep(60);
          }
          catch { if (!stopped) await sleep(150); }
        }
      })();
    },
    async stop()
    {
      stopped = true;
      worker?.kill(); worker = null;
      await loopPromise?.catch(() => {});
      if (sessionId)
      {
        await client.request(`/desktop/${sessionId}/close`, { token }, { timeoutMs: 2000 }).catch(() => {});
        sessionId = ''; token = '';
      }
    },
  };
}

export async function runStandby({ client, query, allowLocal = true, workdir = '', readRoots = [], root = projectRoot, spawnImpl = spawn, output = process.stdout })
{
  const matched = await resolveTargetWindow(query, { root, spawnImpl });
  const bridge = createDesktopBridge({ client, hwnd: matched.id, winTitle: matched.title, allowDesktop: true, allowLocal, workdir, readRoots, root, spawnImpl });
  const { id, token } = await bridge.open();
  await client.request('/desktop/shared', { enabled: true, id, token, windowTitle: matched.title, computerName: hostname() });
  output.write(JSON.stringify({ type: 'standby', status: 'listening', id, windowTitle: matched.title, hwnd: matched.id, localCliEnabled: allowLocal }) + '\n');
  bridge.start('shared');
  await new Promise(resolveStop => {
    const stop = () => { process.off('SIGINT', stop); resolveStop(); };
    process.on('SIGINT', stop);
  });
  await client.request('/desktop/shared', { enabled: false, id, token }, { timeoutMs: 2000 }).catch(() => {});
  await bridge.stop();
  output.write(JSON.stringify({ type: 'standby', status: 'stopped', id }) + '\n');
}
