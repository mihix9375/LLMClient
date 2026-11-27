import { readFile } from 'node:fs/promises';
import { resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import {validateAdapters} from './adapters.mjs';
import {directPathConfig} from './direct-path.mjs';
export const projectRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
export const DEFAULT_SAMPLING = Object.freeze({
  temperature: 0.05,
  top_p: 0.95,
  top_k: 40,
  min_p: 0.05,
  repeat_penalty: 1.1,
  presence_penalty: 0,
  frequency_penalty: 0,
});
export function validateSampling(target, prefix = '') {
  if (!target || typeof target !== 'object') return;
  if (target.temperature !== undefined && (!Number.isFinite(target.temperature) || target.temperature < 0 || target.temperature > 2)) throw Error(`Invalid ${prefix}temperature`);
  if (target.top_p !== undefined && (!Number.isFinite(target.top_p) || target.top_p < 0 || target.top_p > 1)) throw Error(`Invalid ${prefix}top_p`);
  if (target.top_k !== undefined && (!Number.isInteger(target.top_k) || target.top_k < 1 || target.top_k > 100)) throw Error(`Invalid ${prefix}top_k`);
  if (target.min_p !== undefined && (!Number.isFinite(target.min_p) || target.min_p < 0 || target.min_p > 1)) throw Error(`Invalid ${prefix}min_p`);
  if (target.repeat_penalty !== undefined && (!Number.isFinite(target.repeat_penalty) || target.repeat_penalty < 1 || target.repeat_penalty > 2)) throw Error(`Invalid ${prefix}repeat_penalty`);
  if (target.presence_penalty !== undefined && (!Number.isFinite(target.presence_penalty) || target.presence_penalty < -2 || target.presence_penalty > 2)) throw Error(`Invalid ${prefix}presence_penalty`);
  if (target.frequency_penalty !== undefined && (!Number.isFinite(target.frequency_penalty) || target.frequency_penalty < -2 || target.frequency_penalty > 2)) throw Error(`Invalid ${prefix}frequency_penalty`);
}
export async function loadConfig(path = process.env.QWEN_CONFIG || resolve(projectRoot, 'config.local.json'), { useRuntime = true } = {}) {
  let local = {};
  try { local = JSON.parse(await readFile(path, 'utf8')); }
  catch (e) { if (e.code !== 'ENOENT') throw e; }
  const defaults = JSON.parse(await readFile(resolve(projectRoot, 'config.example.json'), 'utf8'));
  let runtime = {};
  try { if (useRuntime) runtime = JSON.parse(await readFile(resolve(projectRoot, 'runtime-settings.json'), 'utf8')); }
  catch (e) { if (e.code !== 'ENOENT') throw e; }
  const c = { ...DEFAULT_SAMPLING, ...defaults, ...local };
  c.modelProfiles={};
  try{const profiles=JSON.parse(await readFile(resolve(projectRoot,'model-profiles.json'),'utf8'));
    for(const role of ['classifier','worker'])if(profiles[role]?.enabled){const profile=profiles[role],url=new URL(profile.modelUrl);
      if(!['http:','https:'].includes(url.protocol) || !['127.0.0.1','localhost','[::1]'].includes(url.hostname) || url.username || url.password || url.search || url.hash)throw Error('Model profiles must use loopback URLs');
      for(const [key,min,max] of [['contextWindow',2048,32768],['maxTokens',128,8192],['timeoutMs',1000,3600000]])if(!Number.isInteger(profile[key]) || profile[key]<min || profile[key]>max)throw Error('Invalid model profile '+key);
      if(profile.maxTokens+256>=profile.contextWindow || typeof profile.model!=='string' || !profile.model || typeof profile.name!=='string' || !Number.isFinite(profile.temperature) || profile.temperature<0 || profile.temperature>2 || profile.cachePrompt!==undefined&&typeof profile.cachePrompt!=='boolean')throw Error('Invalid model profile');
      validateSampling(profile, 'model profile ');
      c.modelProfiles[role]={...profile,reasoningEffort:profile.reasoningEffort??'none',modelUrl:url.href.replace(/\/+$/,'').replace(/\/v1$/,'')};
      if(role==='worker')validateAdapters(c.modelProfiles[role]);
    }
  }catch(e){if(e.code!=='ENOENT')throw e;}
  c.workspaceRoot = resolve(process.env.QWEN_WORKSPACE_ROOT || c.workspaceRoot || resolve(projectRoot,'workspaces'));
  if(c.workspaceArchiveRoot)c.workspaceArchiveRoot=resolve(c.workspaceArchiveRoot);
  if(c.workspaceNvmeRoot)c.workspaceNvmeRoot=resolve(c.workspaceNvmeRoot);
  // v57: running jobs' records and workspaces in memory (ram-work.mjs). Off unless ramWorkRoot is set; the workspace part also needs workspaceArchiveRoot.
  if(c.ramWorkRoot!==undefined&&c.ramWorkRoot!==null){if(typeof c.ramWorkRoot!=='string'||!c.ramWorkRoot)throw Error('Invalid ramWorkRoot');c.ramWorkRoot=resolve(c.ramWorkRoot);}
  // production: ramWorkRoot /srv/qwen-ram (its own 14GB tmpfs, ramWorkMount true), commands in workSlice qwenwork.slice (MemoryMin)
  if(c.ramWorkMount!==undefined&&typeof c.ramWorkMount!=='boolean')throw Error('Invalid ramWorkMount');
  // v57.1: CPUs and quota of workspace commands (the API keeps its own); e.g. workCpus "12-15", workCpuQuota "400%"
  if(c.workCpus!==undefined&&(typeof c.workCpus!=='string'||!/^\d{1,3}(-\d{1,3})?(,\d{1,3}(-\d{1,3})?)*$/.test(c.workCpus)))throw Error('Invalid workCpus');
  if(c.workCpuQuota!==undefined&&(typeof c.workCpuQuota!=='string'||!/^[1-9]\d{1,3}%$/.test(c.workCpuQuota)))throw Error('Invalid workCpuQuota');
  if(c.workSlice!==undefined&&(typeof c.workSlice!=='string'||!/^[A-Za-z0-9_.@-]{1,64}\.slice$/.test(c.workSlice)))throw Error('Invalid workSlice');
  // v58: the Linux image for workspace.run (src/linux-rootfs.mjs). Off unless linuxRootfs is set: {archive: "<path>.tar.zst" (its .sha256
  // sits next to it), root: where scripts/extract-linux-rootfs.sh puts it, default /srv/qwen-ram/rootfs; the overlays go to ../overlay}.
  // overlayfs options are comma/colon separated, so the paths use plain characters only.
  if(c.linuxRootfs!==undefined&&c.linuxRootfs!==null){const r=c.linuxRootfs,plain=/^\/[A-Za-z0-9_.+\/-]+$/;
    if(typeof r!=='object'||Array.isArray(r)||Object.keys(r).some(key=>!['archive','root'].includes(key))||typeof r.archive!=='string'||!plain.test(r.archive)||!r.archive.endsWith('.tar.zst')||r.root!==undefined&&(typeof r.root!=='string'||!plain.test(r.root)))throw Error('Invalid linuxRootfs');
    c.linuxRootfs={archive:resolve(r.archive),root:resolve(r.root??'/srv/qwen-ram/rootfs')};if(dirname(c.linuxRootfs.root)===c.linuxRootfs.root)throw Error('Invalid linuxRootfs');}
  else delete c.linuxRootfs;
  for(const [key,min,max] of [['ramWorkFloorMB',16,4096],['ramSyncIntervalMs',60000,3600000],['persistIntervalMs',0,10000]])if(c[key]!==undefined&&(!Number.isFinite(c[key])||c[key]<min||c[key]>max))throw Error('Invalid '+key);
  for (const key of ['contextWindow', 'maxIterations', 'maxTokens','reasoningEffort','reasoningBudgetTokens', ...Object.keys(DEFAULT_SAMPLING)]) if (runtime[key] !== undefined) c[key] = runtime[key];
  c.maxIterations=0;
  c.requireTaskDecomposition??=true;
  if(typeof c.requireTaskDecomposition!=='boolean')throw Error('Invalid requireTaskDecomposition');
  if(!['worker-first','legacy','direct','unified'].includes(c.plannerWorkPolicy??'worker-first'))throw Error('Invalid plannerWorkPolicy');
  // C (COLLAB 11-04 21:35; src/direct-path.mjs): Planner.DirectWork. Off by default; the master turns it on after the measurements: {enabled:true, limits:{...}}
  c.directPath=directPathConfig(c.directPath);
  if(c.reasoningEffort&&!['none','low','medium','xhigh'].includes(c.reasoningEffort))throw Error('Invalid reasoningEffort');
  if(c.reasoningBudgetTokens!==undefined&&(!Number.isInteger(c.reasoningBudgetTokens)||c.reasoningBudgetTokens< -1||c.reasoningBudgetTokens>8192))throw Error('Invalid reasoningBudgetTokens');
  validateSampling(c);
  try { if (useRuntime) c.contextWindow = Number((await readFile(resolve(projectRoot, 'model-context.txt'), 'utf8')).trim()); }
  catch (e) { if (e.code !== 'ENOENT') throw e; }
  c.modelUrl = process.env.QWEN_MODEL_URL || c.modelUrl;
  c.model = process.env.QWEN_MODEL || c.model;
  c.dataDir = resolve(projectRoot, process.env.QWEN_DATA_DIR || c.dataDir);
  c.nativeBin = resolve(projectRoot, process.env.QWEN_NATIVE_BIN || c.nativeBin);
  if (process.platform === 'win32' && !c.nativeBin.endsWith('.exe')) {
    c.nativeBin += '.exe';
  }
  const url = new URL(c.modelUrl);
  if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password || url.search || url.hash)
    throw Error('modelUrl must be an HTTP(S) URL without credentials, query or fragment');
  c.modelUrl = url.href.replace(/\/+$/, '').replace(/\/v1$/, '');
  if (!['127.0.0.1', '::1', 'localhost'].includes(c.host)) throw Error('Framework API must bind to loopback for local HTTP (use https config for remote access)');
  if(c.https?.enabled){if(typeof c.https.host!=='string'||!c.https.host||!Number.isInteger(c.https.port)||c.https.port<1||c.https.port>65535||!c.https.cert||!c.https.key)throw Error('HTTPS requires host, port, certificate and key');c.https={...c.https,cert:resolve(projectRoot,c.https.cert),key:resolve(projectRoot,c.https.key)};if(!c.historyAccess?.enabled)throw Error('HTTPS requires IP-scoped conversation access');if(c.historyAccess.ownerOnly!==undefined&&typeof c.historyAccess.ownerOnly!=='boolean')throw Error('Invalid historyAccess.ownerOnly');if(c.historyAccess.ownerOnly===undefined)c.historyAccess={...c.historyAccess,ownerOnly:true};}
  for (const [key, min, max] of [['port', 1, 65535], ['maxTasks', 1, 4], ['maxTokens', 128, 8192],
    ['maxInputChars', 2000, 100000], ['timeoutMs', 1000, 3600000], ['maxQueue', 1, 100],
    ['contextWindow', 2048, 32768]]) {
    if (!Number.isInteger(c[key]) || c[key] < min || c[key] > max) throw Error(`Invalid ${key}`);
  }
  return c;
}
