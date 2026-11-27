import { createInterface } from 'node:readline';
import { readFile } from 'node:fs/promises';
import { basename, resolve } from 'node:path';
import { saveJob } from './client.mjs';
import { projectRoot } from './config.mjs';
import { applyCliSettings, buildRunOptions, EFFORT_LEVELS, loadGuiSettings, MODEL_ROLES, presetFromName, SAMPLING_PRESETS, WORK_MODES } from './cli-settings.mjs';
import { createDesktopBridge, listWindows, resolveTargetWindow } from './cli-desktop.mjs';

const HELP = `依頼を入力すると達成ループを実行します（続きは直前の成果物を引き継ぎ）。
/new  /clear  /conv [ID]  /resume <ID>   会話・再開制御
/plan <task>  /goal <task>  /grill-me <topic>  /boost <task>  /teamwork <task>  /browser <task>
/compact [focus]  /learn [list|delete <id>|<note>]  /schedule [list|cancel <id>|<spec> <task>]
/mode <standard|chat|coding|pc_control>   /role <planner|worker>   /plan [on|off]
/preset <code|balanced|creative>   /effort <none|low|medium|xhigh>   /budget <N>
/context <N>   /tokens <N>   /temp /top-p /top-k /min-p /repeat-penalty /presence-penalty /frequency-penalty <値>
/workspace [on|off]  /local-cli [on|off]  /run-on-pc [on|off]  /workdir <dir|clear>  /read-dir <dir|clear>
/windows [q]  /window <name|hwnd|clear>  /pc [on|off]  /attach <file|clear>
/perf  /models [status|start|stop|restart|perf]  /tune-apply  /gui-settings  /settings
/status [ID]  /fetch <ID>  /cancel [ID]  /interrupt <id> <text>  /multi (/end送信)  /exit`;

const toggle = v => { const s = String(v ?? '').trim().toLowerCase(); if (['on', 'true', '1'].includes(s)) return true; if (['off', 'false', '0'].includes(s)) return false; throw Error('on または off を指定してください'); };
const TASK_SLASH = new Set(['/plan', '/goal', '/grill-me', '/boost', '/teamwork', '/browser']);
const isToggleText = v => ['on', 'off', 'true', 'false', '1', '0'].includes(String(v ?? '').trim().toLowerCase());

export async function runInteractive(client, { input = process.stdin, output = process.stdout, options = {}, save = saveJob, syncGui = false, root = projectRoot, desktopApi = { listWindows, resolveTargetWindow, createDesktopBridge } } = {})
{
  const rl = createInterface({ input, output, terminal: !!(input.isTTY && output.isTTY) });
  const lines = rl[Symbol.asyncIterator]();
  const write = v => output.write(String(v) + '\n');
  let parentJobId, convId, waitingJob, active, exiting = false, multiline = null;
  const attachments = [];
  const sessionOptions = { ...options };
  let guiState = syncGui ? loadGuiSettings(root) : {};
  const modeLabel = () => (syncGui ? guiState.planMode : sessionOptions.planMode) ? 'Plan (27B計画+8B委譲)' : 'Direct (27B直接)';
  const interrupt = () => { if (active) { active.abort(Error('ユーザーが中断しました')); write('中断を要求しています…'); } else { exiting = true; rl.close(); } };
  rl.on('SIGINT', interrupt); process.on('SIGINT', interrupt);
  const prompt = () => { if (!exiting) { rl.setPrompt(multiline ? '...> ' : 'あなた> '); rl.prompt(); } };

  const patchCfg = async (guiPatch, serverPatch = {}) => {
    if (syncGui)
    {
      const res = await applyCliSettings({ client, guiPatch, serverPatch, syncGui: true, root });
      guiState = res.gui;
      return res;
    }
    if (Object.keys(serverPatch).length) return client.configure(serverPatch);
    return guiPatch;
  };

  const runTaskText = async (taskText, extraOpts = {}) => {
    active = new AbortController();
    const usePlan = extraOpts.planMode !== undefined ? extraOpts.planMode : (syncGui ? guiState.planMode : sessionOptions.planMode);
    const modeTag = extraOpts.slashCommand ? `[/${extraOpts.slashCommand} Direct 27B]` : (usePlan ? '[Plan Mode]' : '[Direct 27B]');
    const waitOpts = { signal: active.signal, onProgress: job => write(`${modeTag} [${job.id}] ${job.stage} (${job.status})`) };
    let job, bridge = null;
    try
    {
      if (waitingJob)
      {
        const opts = waitingJob.question?.options || [];
        const idx = /^\d+$/.test(taskText) ? Number(taskText) - 1 : -1;
        const ans = (idx >= 0 && idx < opts.length) ? opts[idx] : taskText;
        await client.answer(waitingJob.id, waitingJob.question.id, ans);
        job = await client.wait(waitingJob.id, waitOpts);
      }
      else if (syncGui && typeof client.health === 'function')
      {
        const [health, srv] = await Promise.all([client.health(), client.settings()]);
        const built = buildRunOptions({ gui: guiState, srv, health, values: { attach: attachments }, convId, parentId: parentJobId });
        Object.assign(built.options, extraOpts);
        if (built.desktopAllowed || built.useLocal)
        {
          const win = built.desktopAllowed ? await desktopApi.resolveTargetWindow(guiState.pcWindowTitle, { root }) : null;
          bridge = desktopApi.createDesktopBridge({ client, hwnd: win?.id || 0, winTitle: win?.title || '', allowDesktop: built.desktopAllowed, allowLocal: built.useLocal, workdir: guiState.localWorkdir || '', readRoots: guiState.readOnlyFolders || [], root });
          built.options.desktopSession = (await bridge.open()).id;
        }
        job = await client.run(taskText, built.options, { ...waitOpts, files: [...attachments], onSubmitted: sub => bridge?.start(sub.workspaceId || sub.id) });
        attachments.length = 0;
      }
      else job = await client.run(taskText, { ...sessionOptions, ...extraOpts, ...(parentJobId ? { parentJobId } : {}) }, waitOpts);
    }
    finally { await bridge?.stop(); }
    if (job.status === 'waiting_user') { waitingJob = job; write('質問> ' + job.question.text + '\n' + (job.question.options || []).map((o, i) => `${i + 1}) ${o}`).join(' / ')); return; }
    waitingJob = undefined;
    if (job.conversationId) convId = job.conversationId;
    write('\n27B> ' + (job.result?.review.final_answer || job.error || job.status));
    if (job.result) { parentJobId = job.id; write(`達成: ${job.result.achieved} / 反復: ${job.result.iterations} / 終了理由: ${job.result.stopReason}`); }
    write('履歴は鯖に保存済みです。PCへ保存する場合は /fetch ' + job.id);
  };

  try
  {
    write('Qwen 27B — Direct HTTPS API');
    const settings = await client.settings();
    write(`context=${settings.contextWindow}  steps=上限なし（20手目から効率通知）  maxTokens=${sessionOptions.maxTokens ?? settings.maxTokens}  mode=${modeLabel()}`);
    write(HELP);
    prompt();
    for await (const raw of lines)
    {
      if (exiting) break;
      let text = raw.trim();
      if (multiline) { if (text !== '/end') { multiline.push(raw); prompt(); continue; } text = multiline.join('\n').trim(); multiline = null; }
      else if (text === '/multi') { multiline = []; prompt(); continue; }
      if (!text) { prompt(); continue; }
      try
      {
        if (text.startsWith('/'))
        {
          const sp = text.search(/\s/);
          const cmd = (sp === -1 ? text : text.slice(0, sp)).toLowerCase();
          const a = (sp === -1 ? '' : text.slice(sp + 1)).trim();
          if (['/exit', '/quit'].includes(cmd)) break;
          if (cmd === '/help') write(HELP);
          else if (cmd === '/new' || cmd === '/clear') { parentJobId = undefined; convId = undefined; waitingJob = undefined; write('新しい会話を開始します。'); }
          else if (cmd === '/conv') { convId = (!a || a === 'clear') ? undefined : a; write('conversationId=' + (convId || '')); }
          else if (cmd === '/gui-settings') write(JSON.stringify(loadGuiSettings(root), null, 2));
          else if (cmd === '/settings') write(JSON.stringify({ ...(syncGui ? { gui: guiState } : {}), server: await client.settings(), session: sessionOptions, previousJobId: parentJobId }, null, 2));
          else if (cmd === '/plan' && (!a || isToggleText(a)))
          {
            const next = a ? toggle(a) : !(syncGui ? guiState.planMode : sessionOptions.planMode);
            sessionOptions.planMode = next;
            await patchCfg({ planMode: next });
            write(`実行モード: ${modeLabel()}`);
          }
          else if (TASK_SLASH.has(cmd))
          {
            if (!a) throw Error(`${cmd} <指示内容> を指定してください`);
            await runTaskText(a, { slashCommand: cmd.slice(1), planMode: false });
          }
          else if (cmd === '/compact')
          {
            const targetConv = convId || (parentJobId ? (await client.job(parentJobId)).conversationId : '');
            if (!targetConv) throw Error('圧縮対象の会話がありません（先に会話を開始してください）');
            write(JSON.stringify(await client.compact(targetConv, a ? { focus: a } : {}), null, 2));
          }
          else if (cmd === '/learn')
          {
            if (!a || a === 'list') write(JSON.stringify(await client.memoryList(), null, 2));
            else if (/^(delete|remove|rm)\s+([A-Za-z0-9_-]{4,64})$/i.test(a))
              write(JSON.stringify(await client.memoryDelete(a.split(/\s+/)[1]), null, 2));
            else write(JSON.stringify(await client.memoryAdd(a), null, 2));
          }
          else if (cmd === '/schedule')
          {
            if (!a || a === 'list') write(JSON.stringify(await client.schedulesList(), null, 2));
            else if (/^(cancel|delete|remove|rm)\s+([A-Za-z0-9_-]{4,64})$/i.test(a))
              write(JSON.stringify(await client.scheduleDelete(a.split(/\s+/)[1]), null, 2));
            else
            {
              const m = a.match(/^(in\s+\d+\s*[smh]|every\s+\d+\s*[mh])\s+([\s\S]+)$/i);
              if (!m) throw Error('/schedule [list | cancel <id> | in 10m <task> | every 1h <task>]');
              write(JSON.stringify(await client.scheduleCreate({ spec: m[1], goal: m[2], ...(convId ? { conversationId: convId } : {}) }), null, 2));
            }
          }
          else if (cmd === '/mode') { if (!WORK_MODES.includes(a)) throw Error('mode: ' + WORK_MODES.join('|')); sessionOptions.workMode = a; write(JSON.stringify(await patchCfg({ workMode: a }), null, 2)); }
          else if (cmd === '/role') { if (!MODEL_ROLES.includes(a)) throw Error('role: ' + MODEL_ROLES.join('|')); sessionOptions.modelRole = a; write(JSON.stringify(await patchCfg({ modelRole: a }), null, 2)); }
          else if (cmd === '/preset') { const n = presetFromName(a); const p = SAMPLING_PRESETS[n]; write(JSON.stringify(await patchCfg({ samplingPreset: n, ...p }, p), null, 2)); }
          else if (cmd === '/effort') { if (!EFFORT_LEVELS.includes(a)) throw Error('effort: ' + EFFORT_LEVELS.join('|')); write(JSON.stringify(await patchCfg({ reasoningEffort: a }, { reasoningEffort: a }), null, 2)); }
          else if (cmd === '/budget') { const v = Number(a); write(JSON.stringify(await patchCfg({ reasoningBudgetTokens: v }, { reasoningBudgetTokens: v }), null, 2)); }
          else if (['/context', '/iterations', '/tokens'].includes(cmd))
          {
            if (!/^\d+$/.test(a)) throw Error('設定値を整数で指定してください。');
            const key = { '/context': 'contextWindow', '/iterations': 'maxIterations', '/tokens': 'maxTokens' }[cmd];
            const val = key === 'maxIterations' ? 0 : Number(a);
            const res = key === 'maxIterations' ? await client.configure({ maxIterations: Number(a) }) : await patchCfg({ [key]: val }, { [key]: val });
            sessionOptions[key] = val;
            write(JSON.stringify(res, null, 2));
          }
          else if (['/temp', '/top-p', '/top-k', '/min-p', '/repeat-penalty', '/presence-penalty', '/frequency-penalty'].includes(cmd))
          {
            const key = { '/temp': 'temperature', '/top-p': 'top_p', '/top-k': 'top_k', '/min-p': 'min_p', '/repeat-penalty': 'repeat_penalty', '/presence-penalty': 'presence_penalty', '/frequency-penalty': 'frequency_penalty' }[cmd];
            const val = Number(a);
            write(JSON.stringify(await patchCfg({ [key]: val }, { [key]: val }), null, 2));
          }
          else if (['/workspace', '/local-cli', '/run-on-pc', '/pc'].includes(cmd))
          {
            const key = { '/workspace': 'workspaceEnabled', '/local-cli': 'localCliEnabled', '/run-on-pc': 'runOnPc', '/pc': 'pcEnabled' }[cmd];
            write(JSON.stringify(await patchCfg({ [key]: toggle(a) }), null, 2));
          }
          else if (cmd === '/workdir') write(JSON.stringify(await patchCfg({ localWorkdir: (!a || a === 'clear') ? '' : resolve(a) }), null, 2));
          else if (cmd === '/read-dir')
          {
            const next = a === 'clear' ? [] : [...new Set([...(guiState.readOnlyFolders || []), resolve(a)])];
            write(JSON.stringify(await patchCfg({ readOnlyFolders: next }), null, 2));
          }
          else if (cmd === '/windows')
          {
            const wins = await desktopApi.listWindows({ root });
            write(JSON.stringify(a ? wins.filter(w => String(w.title || '').toLowerCase().includes(a.toLowerCase())) : wins, null, 2));
          }
          else if (cmd === '/window') write(JSON.stringify(await patchCfg({ pcWindowTitle: (!a || a === 'clear') ? '' : a }), null, 2));
          else if (cmd === '/attach') { if (!a || a === 'clear') attachments.length = 0; else attachments.push({ name: basename(a), dataBase64: (await readFile(a)).toString('base64') }); write(`添付: ${attachments.length}件`); }
          else if (cmd === '/perf') write(JSON.stringify(await client.perf(), null, 2));
          else if (cmd === '/models') { const fn = { status: () => client.modelsStatus(), start: () => client.modelsStart(), stop: () => client.modelsStop(), restart: () => client.modelsRestart(), perf: () => client.perf() }[a || 'status']; if (!fn) throw Error('/models [status|start|stop|restart|perf]'); write(JSON.stringify(await fn(), null, 2)); }
          else if (cmd === '/tune-apply') write(JSON.stringify(await client.tuneApply(), null, 2));
          else if (cmd === '/status') write(JSON.stringify(a || parentJobId ? await client.job(a || parentJobId) : await client.list(), null, 2));
          else if (cmd === '/cancel') write(JSON.stringify(await client.cancel(a || client.activeJobId || parentJobId), null, 2));
          else if (cmd === '/interrupt') { const [jid, msg = ''] = a.split(/\s+/, 2); write(JSON.stringify(await client.interrupt(jid, msg), null, 2)); }
          else if (cmd === '/fetch' || cmd === '/resume')
          {
            const job = await client.job(a);
            if (cmd === '/fetch') write('保存先: ' + await save(job));
            else { if (job.status !== 'completed' || !job.result) throw Error('完了済みの依頼を指定してください。'); parentJobId = job.id; write('この成果物から会話を再開します: ' + job.id); }
          }
          else throw Error('不明なコマンドです。/help で使い方を表示します。');
        }
        else await runTaskText(text);
      }
      catch (e) { write('エラー: ' + e.message); }
      finally { active = null; }
      prompt();
    }
  }
  finally { rl.close(); process.off('SIGINT', interrupt); }
  write('対話を終了しました。鯖のサービスは起動したままです。');
}
