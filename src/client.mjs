import { mkdir, lstat, writeFile } from 'node:fs/promises';
import { resolve, join, dirname, sep } from 'node:path';
import { setTimeout as sleep } from 'node:timers/promises';
import { projectRoot } from './config.mjs';
import { existsSync, readFileSync } from 'node:fs';
import { request as httpsRequest } from 'node:https';

function buildCandidates({ baseUrl, baseUrls, connection, gui })
{
  const list = [];
  const push = u =>
  {
    const clean = typeof u === 'string' ? u.trim().replace(/\/+$/, '') : '';
    if (clean && !list.includes(clean)) list.push(clean);
  };
  if (Array.isArray(baseUrls) && baseUrls.length)
  {
    for (const u of baseUrls) push(u);
    if (list.length) return list;
  }
  if (baseUrl) { push(baseUrl); return list; }
  push(process.env.QWEN_API_URL);
  const httpsUrls = [connection.url, ...(Array.isArray(connection.urls) ? connection.urls : [])].filter(u => typeof u === 'string' && u.trim());
  if (gui.api && !(httpsUrls.length && /^http:\/\/(127\.0\.0\.1|localhost):/i.test(gui.api))) push(gui.api);
  for (const u of httpsUrls) push(u);
  if (!list.length) push('https://localhost:8443');
  return list;
}

export class QwenClient {
  constructor({ baseUrl, baseUrls, fetchImpl = fetch, caFile } = {})
  {
    let connection = {};
    try { connection = JSON.parse(readFileSync(join(projectRoot, 'https-server.json'), 'utf8')); }
    catch (e) { if (e.code !== 'ENOENT') throw e; }
    let gui = {};
    try { gui = JSON.parse(readFileSync(join(projectRoot, 'gui-settings.json'), 'utf8')); } catch {}
    this.fetch = fetchImpl;
    this.explicitCaFile = Boolean(caFile || process.env.QWEN_CA_FILE);
    this.certPath = caFile || process.env.QWEN_CA_FILE || (connection.certificate && resolve(projectRoot, connection.certificate));
    this.candidates = buildCandidates({ baseUrl, baseUrls, connection, gui });
    this._setBaseUrl(this.candidates[0]);
  }
  _setBaseUrl(url)
  {
    this.baseUrl = String(url).replace(/\/+$/, '');
    this.ca = (this.certPath && this.baseUrl.startsWith('https:') && this.fetch === fetch && (this.explicitCaFile || existsSync(this.certPath)))
      ? readFileSync(this.certPath)
      : undefined;
  }
  async request(path, body, { signal, timeoutMs = 15000, method } = {})
  {
    const httpMethod = method || (body === undefined ? 'GET' : 'POST');
    const timeout = AbortSignal.timeout(timeoutMs);
    const sig = signal ? AbortSignal.any([signal, timeout]) : timeout;
    if (this.ca)
    {
      const data = body === undefined ? undefined : JSON.stringify(body);
      return new Promise((yes, no) =>
      {
        const headers = { 'Content-Type': 'application/json' };
        if (data !== undefined) headers['Content-Length'] = Buffer.byteLength(data);
        const req = httpsRequest(this.baseUrl + path, { ca: this.ca, method: httpMethod, headers, signal: sig }, res =>
        {
          const chunks = [];
          let size = 0;
          res.on('data', chunk =>
          {
            size += chunk.length;
            if (size > 8 * 1024 * 1024) { res.destroy(Error('API response exceeds 8 MiB')); return; }
            chunks.push(chunk);
          });
          res.on('error', no);
          res.on('end', () =>
          {
            try
            {
              const raw = Buffer.concat(chunks).toString('utf8').trim();
              const value = raw ? JSON.parse(raw) : {};
              if (res.statusCode < 200 || res.statusCode >= 300)
                throw Object.assign(Error(value.error || `HTTP ${res.statusCode}`), { status: res.statusCode });
              yes(value);
            }
            catch (e) { no(e); }
          });
        });
        req.on('error', no);
        req.end(data);
      });
    }
    const res = await this.fetch(this.baseUrl + path, {
      method: httpMethod,
      headers: { 'Content-Type': 'application/json' },
      ...(body === undefined ? {} : { body: JSON.stringify(body) }),
      signal: sig,
    });
    const data = await res.json();
    if (!res.ok) throw Object.assign(Error(data.error || `HTTP ${res.status}`), { status: res.status });
    return data;
  }
  static async connect({ tunnel, sshHost, baseUrls, ...options } = {})
  {
    if (tunnel === true) throw Error('SSH tunnel is disabled; use direct HTTPS');
    if (sshHost) throw Error('SSH is disabled; use direct HTTPS');
    const client = new QwenClient({ ...options, ...(Array.isArray(baseUrls) ? { baseUrls } : {}) });
    let lastError = null;
    for (const candidate of client.candidates)
    {
      client._setBaseUrl(candidate);
      try
      {
        const value = await client.request('/health', undefined, { timeoutMs: 2500 });
        if (value?.service === 'qwen27-framework') return client;
        lastError = Error('Unexpected service from ' + candidate);
      }
      catch (e) { lastError = e; }
    }
    const suffix = lastError ? ` (${lastError.message})` : '';
    throw Error(`Framework API is not reachable via direct HTTPS; check server status and https-server.json${suffix}`);
  }
  health() { return this.request('/health'); }
  commands() { return this.request('/commands'); }
  settings() { return this.request('/settings'); }
  profiles() { return this.request('/model-profiles'); }
  adapters() { return this.request('/worker/adapters'); }
  adapterProfiles() { return this.request('/worker/adapter-profiles'); }
  classify(text, { context = '', labels } = {}, { signal } = {}) { return this.request('/classify', { text, context, ...(labels ? { labels } : {}) }, { signal, timeoutMs: 120000 }); }
  configure(settings) { return this.request('/settings', settings, { timeoutMs: 420000 }); }
  list() { return this.request('/jobs'); }
  conversations() { return this.request('/conversations'); }
  conversation(id) { return this.request('/conversations/' + this.validId(id)); }
  compact(id, body = {}) { return this.request('/conversations/' + this.validId(id) + '/compact', body, { timeoutMs: 30000 }); }
  identity() { return this.request('/identity'); }
  access(id) { return this.request('/conversations/' + this.validId(id) + '/access'); }
  share(id, ip, mode = 'content', { remove = false } = {}) { return this.request('/conversations/' + this.validId(id) + '/access', { ip, mode, remove }); }
  answer(id, questionId, answer, desktopSession) { return this.request('/jobs/' + this.validId(id) + '/answer', { questionId, answer, ...(desktopSession ? { desktopSession } : {}) }); }
  interrupt(id, text, requestId = crypto.randomUUID()) { return this.request('/jobs/' + this.validId(id) + '/interrupt', { text, requestId }); }
  files(id, path) { return this.request('/conversations/' + this.validId(id) + '/files' + (path ? '?path=' + encodeURIComponent(path) : '')); }
  upload(id, files) { return this.request('/conversations/' + this.validId(id) + '/files', { files }); }
  memoryList() { return this.request('/memory'); }
  memoryAdd(text, tags = []) { return this.request('/memory', { text, tags }); }
  memoryDelete(id) { return this.request('/memory/' + encodeURIComponent(String(id || '').trim()), undefined, { method: 'DELETE' }); }
  schedulesList() { return this.request('/schedules'); }
  scheduleCreate(body) { return this.request('/schedules', body); }
  scheduleDelete(id) { return this.request('/schedules/' + encodeURIComponent(String(id || '').trim()), undefined, { method: 'DELETE' }); }
  perf() { return this.request('/models/perf'); }
  modelsStatus() { return this.request('/models/status'); }
  modelsStart() { return this.request('/models/start', {}, { timeoutMs: 120000 }); }
  modelsStop() { return this.request('/models/stop', {}, { timeoutMs: 60000 }); }
  modelsRestart() { return this.request('/models/restart', {}, { timeoutMs: 120000 }); }
  tuneStatus() { return this.request('/tune/status'); }
  tuneAuto(body = { task: 'coding', mode: 'quick' }) { return this.request('/tune/auto', body, { timeoutMs: 120000 }); }
  tuneApply() { return this.request('/tune/apply', {}, { timeoutMs: 120000 }); }
  job(id) { return this.request('/jobs/' + this.validId(id)); }
  cancel(id) { return this.request('/jobs/' + this.validId(id) + '/cancel', {}); }
  validId(id)
  {
    if (!/^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/.test(id || '')) throw Error('Invalid job ID');
    return id;
  }
  async wait(id, { onProgress = () => {}, signal, pollMs = 1000 } = {})
  {
    let last;
    while (true)
    {
      signal?.throwIfAborted();
      const job = await this.request('/jobs/' + this.validId(id), undefined, { signal });
      const state = job.status + ':' + job.stage + ':' + (job.activityRevision || 0);
      if (last !== state) { await onProgress(job); last = state; }
      if (!['queued', 'running'].includes(job.status)) return job;
      await sleep(pollMs, undefined, { signal });
    }
  }
  async run(goal, options = {}, { signal, onProgress, pollMs, files, onSubmitted } = {})
  {
    const submitted = await this.request('/jobs', { goal, options, ...(files?.length ? { files } : {}) }, { signal });
    this.activeJobId = submitted.id;
    onSubmitted?.(submitted);
    try { return await this.wait(submitted.id, { signal, onProgress, pollMs }); }
    catch (e)
    {
      if (signal?.aborted) { await this.cancel(submitted.id).catch(() => {}); throw signal.reason || e; }
      throw e;
    }
    finally { this.activeJobId = null; }
  }
  async close() {}
}
async function noLinks(path)
{
  let current = resolve(path);
  while (true)
  {
    try { if ((await lstat(current)).isSymbolicLink()) throw Error('Refusing symlink/junction: ' + current); }
    catch (e) { if (e.code !== 'ENOENT') throw e; }
    const parent = dirname(current); if (parent === current) return; current = parent;
  }
}
export async function saveJob(job, outputDir = join(projectRoot, 'downloads'))
{
  QwenClient.prototype.validId(job.id);
  const root = resolve(outputDir, job.id);
  await noLinks(root); await mkdir(root, { recursive: true });
  const write = async (path, content) => { await noLinks(path); await mkdir(dirname(path), { recursive: true }); await writeFile(path, content, { mode: 0o600 }); };
  await write(join(root, 'job.json'), JSON.stringify(job, null, 2));
  for (const file of job.result?.artifacts || [])
  {
    if (typeof file.path !== 'string' || !/^[A-Za-z0-9_./-]{1,160}$/.test(file.path) || file.path.split('/').some(x =>
      !x || x.startsWith('.') || x.endsWith('.') || /^(CON|PRN|AUX|NUL|COM[1-9]|LPT[1-9])($|\.)/i.test(x))) throw Error('Unsafe artifact path');
    const artifactRoot = join(root, 'artifacts');
    const path = resolve(artifactRoot, file.path);
    if (!path.startsWith(artifactRoot + sep)) throw Error('Artifact path escapes output directory');
    await write(path, file.content);
  }
  if (job.result) await write(join(root, 'final.md'), job.result.review.final_answer);
  return root;
}
