import { parseArgs } from 'node:util';
import { QwenClient } from './src/client.mjs';
import { runInteractive } from './src/interactive.mjs';
import { loadGuiSettings, parsePatchFromFlags } from './src/cli-settings.mjs';
import { HELP_TEXT, runLocalCommand, runRemoteCommand } from './src/cli-commands.mjs';

const BOOL_FLAGS = ['help', 'tunnel', 'no-tunnel', 'auto-connect', 'no-connect', 'plan', 'no-plan', 'direct',
  'workspace', 'no-workspace', 'local-cli', 'no-local-cli', 'run-on-pc', 'server-windows', 'pc', 'no-pc',
  'standby', 'no-standby', 'shared-desktop', 'clear-read-dirs', 'local-only', 'no-sync-gui', 'sync-gui',
  'json', 'apply', 'remove', 'auto-answer', 'full'];
const STR_FLAGS = ['url', 'api-url', 'ssh-target', 'work-mode', 'mode', 'role', 'model-role', 'context',
  'tokens', 'max-tokens', 'iterations', 'max-iterations', 'effort', 'reasoning-effort', 'budget',
  'reasoning-budget', 'preset', 'temp', 'temperature', 'top-p', 'top-k', 'min-p', 'repeat-penalty',
  'presence-penalty', 'frequency-penalty', 'workdir', 'window', 'conv', 'parent', 'file', 'options',
  'output', 'path', 'ip', 'task', 'tune-mode', 'answer', 'question-id', 'filter', 'query'];

const parseOptions = {};
for (const k of BOOL_FLAGS) parseOptions[k] = k === 'help' ? { type: 'boolean', short: 'h' } : { type: 'boolean' };
for (const k of STR_FLAGS) parseOptions[k] = { type: 'string' };
parseOptions['read-dir'] = { type: 'string', multiple: true };
parseOptions.attach = { type: 'string', multiple: true };

const { values, positionals } = parseArgs({ allowPositionals: true, options: parseOptions });
const [command = 'chat', ...args] = positionals;
const controller = new AbortController();
if (command !== 'chat') process.on('SIGINT', () => controller.abort(Error('Cancelled by user')));

let client;
try
{
  if (values.help || command === 'help') { console.log(HELP_TEXT); }
  else
  {
    if (values['api-url'] && !values.url) values.url = values['api-url'];
    const currentGui = loadGuiSettings();
    const patches = parsePatchFromFlags(values, currentGui);
    const localCode = await runLocalCommand(command, args, values, patches);
    if (localCode !== null) { process.exitCode = localCode; }
    else
    {
      client = await QwenClient.connect({
        ...(values.url ? { baseUrl: values.url } : {}),
      });
      const syncGui = !values['no-sync-gui'];
      if (command === 'chat')
      {
        if (Object.keys(patches.guiPatch).length || Object.keys(patches.serverPatch).length)
          await runRemoteCommand({ client, command: 'configure', args: [], values, ...patches, syncGui, signal: controller.signal });
        await runInteractive(client, { options: patches.serverPatch, syncGui });
      }
      else
      {
        process.exitCode = await runRemoteCommand({ client, command, args, values, ...patches, syncGui, signal: controller.signal });
      }
    }
  }
}
catch (e) { console.error(e.message); process.exitCode = controller.signal.aborted ? 130 : 1; }
finally { await client?.close(); }
