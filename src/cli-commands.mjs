import { readFile } from 'node:fs/promises';
import { basename } from 'node:path';
import { setTimeout as sleep } from 'node:timers/promises';
import { saveJob } from './client.mjs';
import { projectRoot } from './config.mjs';
import { applyCliSettings, buildRunOptions, findWindowMatch, inferPresetName, loadGuiSettings, presetFromName, SAMPLING_KEYS, SAMPLING_PRESETS, saveGuiSettings } from './cli-settings.mjs';
import { createDesktopBridge, listWindows, resolveTargetWindow, runStandby } from './cli-desktop.mjs';

export const HELP_TEXT = `Usage: node client.mjs [command] [args] [options]
Commands:
  chat (default), help, health, identity, commands, windows, gui-settings, settings, configure, preset,
  perf, models [status|start|stop|restart|perf], autotune, tune-status, tune-apply, standby,
  run [goal], status [id], fetch|save <id>, cancel <id>, answer <id> <qid> <text>,
  interrupt <id> <text>, conversations, conversation <id>, compact <convId> [focus],
  memory [list|add <text>|delete <id>], schedules [list|add <spec> <goal>|delete <id>],
  share <id> <ip>, files <id>, upload <id>, classify <text>
Options (synced to gui-settings.json by default):
  --url <url> --auto-connect/--no-connect
  --work-mode <standard|chat|coding|pc_control> --role <planner|worker> --plan/--no-plan/--direct
  --context <N> --tokens <N> --effort <none|low|medium|xhigh> --budget <N>
  --preset <code|balanced|creative> --temp --top-p --top-k --min-p --repeat-penalty --presence-penalty --frequency-penalty
  --workspace/--no-workspace --local-cli/--no-local-cli --run-on-pc/--server-windows --workdir <dir|clear>
  --read-dir <dir> --clear-read-dirs --pc/--no-pc --window <title|hwnd> --standby/--no-standby --shared-desktop
  --attach <file> --conv <id> --parent <id> --file <goal.txt> --output <dir> --local-only --no-sync-gui --json`;

export async function runLocalCommand(command, args, values, { guiPatch, serverPatch }, { root = projectRoot, desktopApi = { listWindows }, out = console } = {})
{
  if (command === 'windows')
  {
    const wins = await desktopApi.listWindows({ root });
    const q = values.window ?? args[0];
    if (q) { const m = findWindowMatch(wins, q); out.log(JSON.stringify({ windows: wins, matched: m }, null, 2)); return m ? 0 : 1; }
    out.log(JSON.stringify(wins, null, 2));
    return 0;
  }
  if (command === 'gui-settings')
  {
    const res = await applyCliSettings({ client: null, guiPatch, serverPatch, syncGui: !values['no-sync-gui'], localOnly: true, root });
    out.log(JSON.stringify(res.gui, null, 2));
    return 0;
  }
  return null;
}

export async function runRemoteCommand({ client, command, args, values, guiPatch, serverPatch, syncGui, signal, root = projectRoot })
{
  const out = value => console.log(JSON.stringify(value, null, 2));
  if (command === 'preset')
  {
    if (!args[0]) throw Error('Usage: preset <code|balanced|creative>');
    const name = presetFromName(args[0]);
    guiPatch.samplingPreset = name;
    for (const [k, v] of Object.entries(SAMPLING_PRESETS[name])) { guiPatch[k] = v; serverPatch[k] = v; }
  }
  if (['settings', 'configure', 'preset'].includes(command))
  { out(await applyCliSettings({ client, guiPatch, serverPatch, syncGui, localOnly: !!values['local-only'], root })); return 0; }
  if (command === 'health') { out(await client.health()); return 0; }
  if (command === 'commands') { out(await client.commands()); return 0; }
  if (command === 'identity') { out(await client.identity()); return 0; }
  if (command === 'profiles') { out(await client.profiles()); return 0; }
  if (command === 'adapters') { out(await client.adapters()); return 0; }
  if (command === 'adapter-profiles') { out(await client.adapterProfiles()); return 0; }
  if (command === 'perf') { out(await client.perf()); return 0; }
  if (command === 'tune-status') { out(await client.tuneStatus()); return 0; }
  if (command === 'tune-apply')
  {
    const res = await client.tuneApply();
    if (syncGui && res?.settings)
    {
      const patch = { samplingPreset: inferPresetName(res.settings) };
      for (const k of SAMPLING_KEYS) if (res.settings[k] !== undefined) patch[k] = res.settings[k];
      saveGuiSettings(patch, root);
    }
    out(res); return 0;
  }
  if (command === 'autotune')
  {
    await client.tuneAuto({ task: values.task || 'coding', mode: values['tune-mode'] || 'quick' });
    let st;
    for (let i = 0; i < 180; i++)
    {
      await sleep(1000, undefined, { signal });
      st = await client.tuneStatus();
      if (['completed', 'failed', 'cancelled'].includes(st.status)) break;
    }
    if (values.apply && st?.status === 'completed' && !st.alreadyOptimal) st.appliedResult = await client.tuneApply();
    out(st); return st?.status === 'completed' ? 0 : 2;
  }
  if (command === 'models')
  {
    const sub = args[0] || 'status';
    const fn = { status: () => client.modelsStatus(), perf: () => client.perf(), start: () => client.modelsStart(), stop: () => client.modelsStop(), restart: () => client.modelsRestart() }[sub];
    if (!fn) throw Error('Usage: models [status|start|stop|restart|perf]');
    out(await fn()); return 0;
  }
  if (command === 'compact') { out(await client.compact(args[0], { focus: args.slice(1).join(' ') })); return 0; }
  if (command === 'memory')
  {
    const sub = args[0] || 'list';
    if (sub === 'list') { out(await client.memoryList()); return 0; }
    if (sub === 'add') { out(await client.memoryAdd(args.slice(1).join(' '))); return 0; }
    if (sub === 'delete' || sub === 'remove') { out(await client.memoryDelete(args[1])); return 0; }
    throw Error('Usage: memory [list|add <text>|delete <id>]');
  }
  if (command === 'schedules')
  {
    const sub = args[0] || 'list';
    if (sub === 'list') { out(await client.schedulesList()); return 0; }
    if (sub === 'add')
    {
      const rest = args.slice(1).join(' ').trim();
      const m = rest.match(/^(in\s+\d+\s*[smh]|every\s+\d+\s*[mh])\s+([\s\S]+)$/i);
      if (!m) throw Error('Usage: schedules add "<in 10m|every 1h>" <goal>');
      out(await client.scheduleCreate({ spec: m[1], goal: m[2], ...(values.conv ? { conversationId: values.conv } : {}) })); return 0;
    }
    if (sub === 'delete' || sub === 'remove' || sub === 'cancel') { out(await client.scheduleDelete(args[1])); return 0; }
    throw Error('Usage: schedules [list|add <spec> <goal>|delete <id>]');
  }
  if (command === 'standby')
  {
    const { gui } = await applyCliSettings({ client: null, guiPatch, serverPatch: {}, syncGui, localOnly: true, root });
    await runStandby({ client, query: values.window || gui.pcWindowTitle, allowLocal: values['local-cli'] ? true : values['no-local-cli'] ? false : gui.localCliEnabled !== false, workdir: gui.localWorkdir || '', readRoots: gui.readOnlyFolders || [], root });
    return 0;
  }
  if (command === 'conversations') { out(await client.conversations()); return 0; }
  if (command === 'conversation') { out(await client.conversation(args[0])); return 0; }
  if (command === 'share') { out(await client.share(args[0], values.ip || args[1], values.mode || 'content', { remove: !!values.remove })); return 0; }
  if (command === 'files') { out(await client.files(args[0], values.path || args[1])); return 0; }
  if (command === 'upload')
  {
    const paths = [...args.slice(1), ...(values.attach || [])];
    const files = await Promise.all(paths.map(async p => ({ name: basename(p), dataBase64: (await readFile(p)).toString('base64') })));
    out(await client.upload(args[0], files)); return 0;
  }
  if (command === 'classify') { out(await client.classify(args.join(' '), {}, { signal })); return 0; }
  if (command === 'status') { out(args[0] ? await client.job(args[0]) : await client.list()); return 0; }
  if (command === 'cancel') { out(await client.cancel(args[0])); return 0; }
  if (command === 'answer') { out(await client.answer(args[0], values['question-id'] || args[1], values.answer || args.slice(2).join(' '))); return 0; }
  if (command === 'interrupt') { out(await client.interrupt(args[0], args.slice(1).join(' '))); return 0; }
  if (command === 'fetch' || command === 'save')
  {
    const job = await client.job(args[0]);
    const saved = await saveJob(job, values.output);
    out({ job, savedDir: saved });
    return 0;
  }
  if (command !== 'run') throw Error(`Unknown command: ${command}`);

  const merged = await applyCliSettings({ client, guiPatch, serverPatch, syncGui, localOnly: !!values['local-only'], root });
  const files = await Promise.all((values.attach || []).map(async p => ({ name: basename(p), dataBase64: (await readFile(p)).toString('base64') })));
  const rawGoal = values.file ? await readFile(values.file, 'utf8') : args.join(' ');
  const goal = rawGoal || (files.length ? '添付したファイルを確認してください。' : '');
  const extraOpts = values.options ? JSON.parse(await readFile(values.options, 'utf8')) : {};
  const health = await client.health();
  const explicitKeys = new Set(Object.keys(serverPatch));
  const built = buildRunOptions({ gui: merged.gui, srv: merged.server, health, values, explicitServerKeys: explicitKeys, convId: values.conv, parentId: values.parent });
  Object.assign(built.options, extraOpts);
  let bridge = null;
  if (!built.sharedDesktop && (built.desktopAllowed || built.useLocal))
  {
    const win = built.desktopAllowed ? await resolveTargetWindow(values.window || merged.gui.pcWindowTitle, { root }) : null;
    bridge = createDesktopBridge({ client, hwnd: win?.id || 0, winTitle: win?.title || '', allowDesktop: built.desktopAllowed, allowLocal: built.useLocal, workdir: merged.gui.localWorkdir || '', readRoots: merged.gui.readOnlyFolders || [], root });
    built.options.desktopSession = (await bridge.open()).id;
  }
  try
  {
    const modeTag = built.options.planMode ? '[Plan Mode]' : '[Direct 27B]';
    let job = await client.run(goal, built.options, {
      signal, files, onSubmitted: sub => bridge?.start(sub.workspaceId || sub.id),
      onProgress: j => { if (!values.json) console.log(`${modeTag} [${j.id}] ${j.stage} (${j.status})`); },
    });
    if (job.status === 'waiting_user' && (values['auto-answer'] || values.answer !== undefined))
    {
      await client.answer(job.id, job.question.id, values.answer || '');
      job = await client.wait(job.id, { signal, onProgress: j => { if (!values.json) console.log(`${modeTag} [${j.id}] ${j.stage} (${j.status})`); } });
    }
    if (job.status === 'waiting_user') { out({ question: job.question, job }); return 3; }
    const saved = await saveJob(job, values.output);
    if (values.json) out({ job, savedDir: saved });
    else
    {
      console.log(job.result?.review.final_answer || job.error || job.status);
      if (job.result) out({ achieved: job.result.achieved, iterations: job.result.iterations, stopReason: job.result.stopReason, testsExecuted: job.result.testsExecuted });
      console.log('Saved:', saved);
    }
    return job.status !== 'completed' ? 1 : job.result?.achieved === false ? 2 : 0;
  }
  finally { await bridge?.stop(); }
}
