import { readFileSync, writeFileSync, renameSync, existsSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { projectRoot, validateSampling } from './config.mjs';

export const WORK_MODES = ['standard', 'chat', 'coding', 'pc_control'];
export const MODEL_ROLES = ['planner', 'worker'];
export const EFFORT_LEVELS = ['none', 'low', 'medium', 'xhigh'];
export const SAMPLING_KEYS = ['temperature', 'top_p', 'top_k', 'min_p', 'repeat_penalty', 'presence_penalty', 'frequency_penalty'];

export const SAMPLING_PRESETS = Object.freeze({
  code: { temperature: 0.05, top_p: 0.95, top_k: 40, min_p: 0.05, repeat_penalty: 1.10, presence_penalty: 0.0, frequency_penalty: 0.0 },
  balanced: { temperature: 0.20, top_p: 0.95, top_k: 40, min_p: 0.05, repeat_penalty: 1.10, presence_penalty: 0.0, frequency_penalty: 0.0 },
  creative: { temperature: 0.70, top_p: 0.95, top_k: 60, min_p: 0.03, repeat_penalty: 1.15, presence_penalty: 0.10, frequency_penalty: 0.10 },
});

export const DEFAULT_GUI_SETTINGS = Object.freeze({
  api: 'https://localhost:8443', autoConnect: true,
  workspaceEnabled: true, localCliEnabled: true, runOnPc: false, pcEnabled: false, pcStandby: false,
  pcWindowTitle: '', localWorkdir: '', readOnlyFolders: [], localWorkdirs: {},
  modelRole: 'planner', workMode: 'standard', planMode: false, samplingPreset: 'code',
  contextWindow: 16384, maxTokens: 1800, reasoningEffort: 'none', reasoningBudgetTokens: 2048,
  ...SAMPLING_PRESETS.code,
});

export function presetFromName(raw)
{
  const key = String(raw || '').trim().toLowerCase();
  if (key === '0' || key === 'code' || key === 'precise') return 'code';
  if (key === '1' || key === 'balanced' || key === 'standard') return 'balanced';
  if (key === '2' || key === 'creative') return 'creative';
  throw Error(`Unknown sampling preset: ${raw}`);
}

export function inferPresetName(values)
{
  const near = (a, b) => Math.abs(Number(a) - Number(b)) < 0.005;
  for (const [name, p] of Object.entries(SAMPLING_PRESETS))
  {
    const match = near(values.temperature, p.temperature) && near(values.top_p, p.top_p) &&
      Number(values.top_k) === p.top_k && near(values.min_p, p.min_p) &&
      near(values.repeat_penalty, p.repeat_penalty) && near(values.presence_penalty, p.presence_penalty) &&
      near(values.frequency_penalty, p.frequency_penalty);
    if (match) return name;
  }
  return 'custom';
}

export function loadGuiSettings(root = projectRoot)
{
  const out = { ...DEFAULT_GUI_SETTINGS, readOnlyFolders: [], localWorkdirs: {} };
  const file = join(root, 'gui-settings.json');
  if (!existsSync(file)) return out;
  try
  {
    const parsed = JSON.parse(readFileSync(file, 'utf8'));
    if (parsed && typeof parsed === 'object' && !Array.isArray(parsed)) Object.assign(out, parsed);
  }
  catch {}
  delete out.sshTarget;
  delete out.autoTunnel;
  return out;
}

export function saveGuiSettings(patch = {}, root = projectRoot)
{
  const merged = loadGuiSettings(root);
  Object.assign(merged, patch);
  delete merged.sshTarget;
  delete merged.autoTunnel;
  const file = join(root, 'gui-settings.json');
  const tmp = file + '.tmp';
  writeFileSync(tmp, JSON.stringify(merged, null, 2), 'utf8');
  try { renameSync(tmp, file); }
  catch { writeFileSync(file, JSON.stringify(merged, null, 2), 'utf8'); }
  return merged;
}

export function validateGuiAndServerPatch(merged)
{
  if (merged.workMode !== undefined && !WORK_MODES.includes(merged.workMode)) throw Error(`Invalid workMode: ${merged.workMode}`);
  if (merged.modelRole !== undefined && !MODEL_ROLES.includes(merged.modelRole)) throw Error(`Invalid modelRole: ${merged.modelRole}`);
  if (merged.reasoningEffort !== undefined && !EFFORT_LEVELS.includes(merged.reasoningEffort)) throw Error(`Invalid reasoningEffort: ${merged.reasoningEffort}`);
  if (merged.reasoningBudgetTokens !== undefined && (!Number.isInteger(merged.reasoningBudgetTokens) || merged.reasoningBudgetTokens < -1 || merged.reasoningBudgetTokens > 32768))
    throw Error('reasoningBudgetTokens must be -1 or 128-32768');
  const ctx = merged.contextWindow ?? 16384;
  const tok = merged.maxTokens ?? 1800;
  if (!Number.isInteger(ctx) || ctx < 2048 || ctx > 32768) throw Error('contextWindow must be 2048-32768');
  if (!Number.isInteger(tok) || tok < 128 || tok > 8192) throw Error('maxTokens must be 128-8192');
  if (tok + 256 >= ctx) throw Error('contextWindow must exceed maxTokens + 256');
  validateSampling(merged);
}

export function parsePatchFromFlags(values, currentGui = {})
{
  const num = (v, int = false) => { const n = Number(v); if (int ? !Number.isInteger(n) : !Number.isFinite(n)) throw Error(`Invalid number: ${v}`); return n; };
  const guiPatch = {};
  const serverPatch = {};
  if (values.tunnel) throw Error('SSH tunnel is disabled; configure direct HTTPS via --url');
  if (values['ssh-target']) throw Error('SSH is disabled; configure direct HTTPS via --url');
  if (values.url) guiPatch.api = values.url;
  for (const [on, off, key] of [['auto-connect', 'no-connect', 'autoConnect'], ['plan', 'no-plan', 'planMode'], ['workspace', 'no-workspace', 'workspaceEnabled'], ['local-cli', 'no-local-cli', 'localCliEnabled'], ['run-on-pc', 'server-windows', 'runOnPc'], ['pc', 'no-pc', 'pcEnabled'], ['standby', 'no-standby', 'pcStandby']])
    if (values[on]) guiPatch[key] = true; else if (values[off]) guiPatch[key] = false;
  if (values.direct) guiPatch.planMode = false;
  if (values['work-mode']) guiPatch.workMode = values['work-mode'];
  if (values.role || values['model-role']) guiPatch.modelRole = values.role || values['model-role'];
  if (values.window !== undefined) guiPatch.pcWindowTitle = values.window;
  if (values.workdir !== undefined) guiPatch.localWorkdir = !values.workdir || values.workdir === 'clear' ? '' : resolve(values.workdir);
  if (values['clear-read-dirs'] || values['read-dir'])
  {
    const base = values['clear-read-dirs'] ? [] : [...(currentGui.readOnlyFolders || [])];
    for (const dir of values['read-dir'] || []) { const r = resolve(dir); if (!base.includes(r)) base.push(r); }
    guiPatch.readOnlyFolders = base;
  }
  const setSrv = (k, v) => { serverPatch[k] = v; guiPatch[k] = v; };
  if (values.preset) { const name = presetFromName(values.preset); guiPatch.samplingPreset = name; for (const [k, v] of Object.entries(SAMPLING_PRESETS[name])) setSrv(k, v); }
  if (values.context !== undefined) setSrv('contextWindow', num(values.context, true));
  const tok = values.tokens ?? values['max-tokens']; if (tok !== undefined) setSrv('maxTokens', num(tok, true));
  const eff = values.effort ?? values['reasoning-effort']; if (eff !== undefined) setSrv('reasoningEffort', eff);
  const bdg = values.budget ?? values['reasoning-budget']; if (bdg !== undefined) setSrv('reasoningBudgetTokens', num(bdg, true));
  for (const [flag, key, isInt] of [['temp', 'temperature'], ['temperature', 'temperature'], ['top-p', 'top_p'], ['top-k', 'top_k', true], ['min-p', 'min_p'], ['repeat-penalty', 'repeat_penalty'], ['presence-penalty', 'presence_penalty'], ['frequency-penalty', 'frequency_penalty']])
    if (values[flag] !== undefined) setSrv(key, num(values[flag], !!isInt));
  return { guiPatch, serverPatch };
}

export async function applyCliSettings({ client, guiPatch = {}, serverPatch = {}, syncGui = true, localOnly = false, root = projectRoot })
{
  const current = loadGuiSettings(root);
  const preview = { ...current, ...guiPatch, ...serverPatch };
  validateGuiAndServerPatch(preview);
  if (!guiPatch.samplingPreset) guiPatch.samplingPreset = inferPresetName(preview);
  let server = null;
  if (client && !localOnly)
  {
    if (Object.keys(serverPatch).length > 0)
    {
      server = await client.configure(serverPatch);
      for (const k of ['contextWindow', 'maxTokens', 'reasoningEffort', 'reasoningBudgetTokens', ...SAMPLING_KEYS])
        if (server[k] !== undefined) guiPatch[k] = server[k];
      guiPatch.samplingPreset = inferPresetName({ ...preview, ...server });
    }
    else { try { server = await client.settings(); } catch {} }
  }
  const gui = syncGui && Object.keys(guiPatch).length > 0 ? saveGuiSettings(guiPatch, root) : { ...current, ...guiPatch };
  return { gui, server };
}

export function buildRunOptions({ gui, srv, health, values = {}, explicitServerKeys = new Set(), convId, parentId })
{
  const caps = health?.capabilities || {};
  const wsCapable = !!caps.workspace;
  const localCapable = !!caps.localCli;
  const serverWinAvail = !!caps.serverWindows?.available;
  const workMode = gui.workMode || 'standard';
  const modelRole = gui.modelRole || 'planner';
  const planMode = !!gui.planMode;
  const sharedDesktop = !!values['shared-desktop'];
  let useWorkspace = gui.workspaceEnabled !== false;
  if (!wsCapable && workMode !== 'pc_control' && !(values.attach?.length) && !values.workspace) useWorkspace = false;
  if (workMode === 'pc_control' || (values.attach?.length)) useWorkspace = true;
  if (useWorkspace && !wsCapable) throw Error('作業環境対応のAPIに接続し直してください');
  const desktopAllowed = values.pc ? true : values['no-pc'] ? false : !!gui.pcEnabled;
  const localAllowed = values['local-cli'] ? true : values['no-local-cli'] ? false : gui.localCliEnabled !== false;
  const runOnPc = !!gui.runOnPc;
  if (workMode === 'pc_control' && !desktopAllowed && !sharedDesktop) throw Error('pc_control mode requires --pc (with --window) or --shared-desktop');
  const options = { contextWindow: srv?.contextWindow ?? gui.contextWindow ?? 16384, maxIterations: 0, maxTokens: srv?.maxTokens ?? gui.maxTokens ?? 1800, modelRole };
  if (srv?.reasoningEffort) options.reasoningEffort = srv.reasoningEffort;
  if (srv?.reasoningBudgetTokens !== undefined) options.reasoningBudgetTokens = srv.reasoningBudgetTokens;
  for (const k of SAMPLING_KEYS) if (srv?.[k] !== undefined) options[k] = srv[k];
  if (workMode !== 'standard') options.workMode = workMode;
  if (workMode === 'chat' || workMode === 'pc_control')
  {
    if (!explicitServerKeys.has('maxTokens')) delete options.maxTokens;
    if (!explicitServerKeys.has('reasoningEffort')) delete options.reasoningEffort;
    if (!explicitServerKeys.has('reasoningBudgetTokens')) delete options.reasoningBudgetTokens;
    for (const k of SAMPLING_KEYS) if (!explicitServerKeys.has(k)) delete options[k];
    if (modelRole === 'planner') options.planMode = false;
  }
  else if (workMode === 'coding')
  {
    if (!explicitServerKeys.has('maxTokens')) delete options.maxTokens;
    if (!explicitServerKeys.has('temperature')) delete options.temperature;
    if (modelRole === 'planner') options.planMode = planMode;
  }
  else if (modelRole === 'planner') options.planMode = planMode;
  if (useWorkspace) options.execution = 'workspace';
  let machine = '';
  if (serverWinAvail && useWorkspace) machine = (runOnPc || desktopAllowed || sharedDesktop) ? 'user-pc' : 'server-windows';
  if (machine) options.machine = machine;
  const useLocal = (workMode !== 'pc_control') && (workMode !== 'chat') && useWorkspace && localAllowed && machine !== 'server-windows';
  if (useLocal && !localCapable) throw Error('PCのCLIに対応したAPIへ接続し直してください');
  if (sharedDesktop) options.useSharedDesktop = true;
  if (convId) options.conversationId = convId;
  else if (parentId) options.parentJobId = parentId;
  return { options, desktopAllowed, useLocal, sharedDesktop, useWorkspace };
}

export function findWindowMatch(windows, query)
{
  if (!Array.isArray(windows) || !query) return null;
  if (/^\d+$/.test(query)) { const id = Number(query); return windows.find(w => Number(w.id) === id) || null; }
  const needle = query.toLowerCase();
  return windows.find(w => String(w.title || '').toLowerCase() === needle) ||
    windows.find(w => String(w.title || '').toLowerCase().includes(needle)) || null;
}
