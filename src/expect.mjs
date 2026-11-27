// v63 (COLLAB 10-28 23:17-23:20): WorkspaceShell.Expect / LocalShell.Expect. Each case's command runs through the existing
// workspace.run / local.run path (sandbox, PowerShell guards, time limit and the PC broker unchanged) and its output is compared
// here, so shell quoting, "\n" inside quotes, $(...) dropping the final newline and the order of && and $? can no longer break a
// check (v52 comparison 10-28: every new false finish was a hand-written shell check that failed three times).
// A mismatch is a normal result ({pass:false} with the first differing line): it is evidence about the program, never a tool
// failure, so it does not feed the circuit breaker. Only a case whose command could not be started is an error.
import {createHash} from 'node:crypto';
import {missingCommand} from './image-commands.mjs';
export const expectLimits=Object.freeze({cases:8,command:1800,stdin:1200,expected:4000,why:200});
export const matchModes=['trimEnd','exact','contains'];
// v63.4 (COLLAB 10-30 18:50): the 27B's cases may also use numeric (the 8B's own Expect keeps the three modes above)
export const plannedMatchModes=[...matchModes,'numeric'];
const clip=(value,size)=>{const text=String(value??'');return text.length>size?text.slice(0,size)+'…[抜粋]':text;};
// review t1-1: the number of failing tests a runner reports (node "ℹ fail N", TAP "# fail N", pytest "N failed", go's "--- FAIL" lines), so the
// guard's (iv-b) sees progress inside T1 (null: the output does not say)
export function failingTestCount(stdout,stderr){const text=String(stdout??'')+'\n'+String(stderr??'');
 const m=text.match(/^\s*ℹ fail (\d+)/m)||text.match(/^# fail (\d+)/m)||text.match(/\b(\d+) failed\b/);if(m)return Number(m[1]);
 const go=(text.match(/^\s*--- FAIL:/gm)||[]).length;return go||null;}
// v63.6-3 (COLLAB 10-31 08:55): under each stack frame of a failed test that points into the workspace (node "at fn (file.mjs:22:43)"), the
// line it points at ("→ file.mjs:22: code"): the 8B read the program and the test file in turn to find the failing assert. Right under the
// frame, so the summary that keeps the head of failedTests keeps them too (one step later only the summary is left). read(path) -> the file
// text (a path that cannot be read is left out); relative paths inside the workspace only; a location once; at most max lines.
export async function withSourceLines(text,read,{max=6,width=160}={}){
 const files=new Map(),seen=new Set(),out=[];let shown=0;
 for(const line of String(text??'').split('\n')){out.push(line);if(shown>=max||!/^\s*at\s/.test(line))continue;
  const m=line.match(/([\w.\/-]+\.(?:m?js|cjs|mts|ts)):(\d+):\d+/);if(!m)continue;const path=m[1],no=+m[2],key=path+':'+no;
  if(seen.has(key)||path.startsWith('/')||path.split('/').includes('..'))continue;seen.add(key);
  if(!files.has(path))files.set(path,await Promise.resolve().then(()=>read(path)).catch(()=>null));
  const content=files.get(path),source=typeof content==='string'?content.split('\n')[no-1]:undefined;if(source===undefined)continue;
  out.push(`${line.match(/^\s*/)[0]}→ ${path}:${no}: ${source.trim().slice(0,width)}`);shown++;}
 return out.join('\n');
}
// v63.5-1 (COLLAB 10-31 03:15): the failing tests of a test runner's output: node's spec reporter (after "✖ failing tests:"), TAP ("not ok"
// with its YAML up to the stack), pytest (FAILED and E lines) and go test (--- FAIL and file:line lines). Stack frames of node itself and
// of node_modules are dropped; file:// paths are cut to the file name and line.
export function testFailureLines(stdout,stderr,limit=900){
 const lines=(String(stdout??'')+'\n'+String(stderr??'')).replace(/\r\n/g,'\n').split('\n'),picked=[];
 const spec=lines.findIndex(line=>/^\s*✖ failing tests:/.test(line));
 if(spec>=0)picked.push(...lines.slice(spec+1));
 else if(lines.some(line=>/^\s*not ok\b/.test(line))){let inside=false;for(const line of lines){if(/^\s*not ok\b/.test(line)){inside=true;picked.push(line);continue;}if(inside&&/^\s*(stack:|\.\.\.$|ok\b|# )/.test(line)){inside=false;continue;}if(inside)picked.push(line);}}
 else picked.push(...lines.filter(line=>/^(FAILED\b|E\s|_+ .+ _+$)|^\s*--- FAIL|_test\.go:\d+|AssertionError|^\s*(expected|actual|operator):/.test(line)));
 const out=picked.filter(line=>line.trim()&&!(/^\s*at\s/.test(line)&&/\(?node:|node_modules|internal\//.test(line))).map(line=>line.replace(/(file:\/\/)?\/workspace\/([^\s)]+:\d+:\d+)/g,'$2').replace(/file:\/\/\S*\/([^\/\s)]+:\d+:\d+)/g,'$1').replace(/\s+$/,''));   // v63.6 review texts-1: a frame in the workspace keeps its path there (test/add.test.mjs), as withSourceLines reads it
 const text=out.join('\n');return text.length>limit?text.slice(0,limit)+'…':text;
}
const crlf=text=>String(text??'').replace(/\r\n/g,'\n');
// trimEnd: CRLF as LF, and the whitespace and newlines at the very end of the whole output dropped
const trimmed=text=>crlf(text).replace(/\s+$/,'');
const shown=(lines,index)=>index<lines.length?clip(lines[index],200):null;
// numeric (v63.4, COLLAB 10-30 18:50): both outputs split on whitespace; the same number of words; a word that reads as a number is compared
// as a number (21 = 21.0), any other word as text; no rounding tolerance ("ratio: 21" = "ratio: 21.0", "21" != "ratio: 21")
const numberWord=word=>/^[+-]?(\d+(\.\d*)?|\.\d+)([eE][+-]?\d+)?$/.test(word)?Number(word):null;
const sameWord=(a,b)=>{const x=numberWord(a),y=numberWord(b);return x!==null&&y!==null?x===y:a===b;};
export function compareOutput(expected,actual,match='trimEnd'){
 if(match==='numeric'){const words=text=>trimmed(text).split(/\s+/).filter(Boolean),a=words(expected),b=words(actual);let at=0;while(at<a.length&&at<b.length&&sameWord(a[at],b[at]))at++;
  if(at===a.length&&at===b.length)return {pass:true};
  return {pass:false,diff:{word:at+1,expected:a[at]??null,actual:b[at]??null,note:(a.length!==b.length?'語の数が違います。':'')+'数は数として、ほかの語は文字として、空白で分けた語ごとに比べました'}};}
 if(match==='contains'){const pass=crlf(actual).includes(crlf(expected));return pass?{pass}:{pass,expectedText:clip(expected,300),actualHead:clip(actual,300)};}
 const [want,got]=match==='exact'?[String(expected??''),String(actual??'')]:[trimmed(expected),trimmed(actual)];
 if(want===got)return {pass:true};
 const a=want.split('\n'),b=got.split('\n');let line=0;while(line<a.length&&line<b.length&&a[line]===b[line])line++;
 const diff={line:line+1,expected:shown(a,line),actual:shown(b,line)};
 if(diff.expected===null)diff.note='実際の出力の方が長い（期待はここで終わり）';else if(diff.actual===null)diff.note='実際の出力の方が短い';
 if(match==='exact'&&trimmed(expected)===trimmed(actual))diff.note='末尾の空白・改行、または CRLF だけが違います（match=trimEnd なら一致）';
 return {pass:false,diff};
}
// v63.2 (F1, Windows 14:55): the words for a case that differs only at the very end (the Expect note, nextDecision, the guard, finish)
const endOnlyText='中身が合っています（末尾の空白・改行、または CRLF だけが違います）。依頼が末尾の空白・改行を決めていなければ、match=trimEnd で同じ case をやり直します。プログラムも期待値も変えません。';
export const endOnlyAdvice='合わない case は'+endOnlyText;
// stdin goes to the command exactly (bash: the bytes through base64, no quoting; the subshell keeps a multi-line command together)
export const bashWithStdin=(command,stdin)=>stdin===undefined?command:`{ printf %s '${Buffer.from(stdin,'utf8').toString('base64')}' | base64 -d; } 2>/dev/null | (\n${command}\n)`;
// Windows PowerShell 5.1 sends a piped string to the FIRST program of the command as UTF-8 (with $OutputEncoding) and ends it with CRLF
export const powershellWithStdin=(command,stdin)=>stdin===undefined?command:`$OutputEncoding=New-Object System.Text.UTF8Encoding $false\n[System.Text.Encoding]::UTF8.GetString([System.Convert]::FromBase64String('${Buffer.from(stdin,'utf8').toString('base64')}')) | ${command}`;
// v63.4 (COLLAB 10-30 18:50, design §2.2): the 27B's files for a case are written by the harness through base64 before the command (no
// quoting, no printf format characters); bash only (v63.4 cases are for target=workspace)
export const safeFilePath=path=>typeof path==='string'&&/^[A-Za-z0-9._\/-]{1,80}$/.test(path)&&!path.startsWith('/')&&!path.split('/').includes('..');
// COLLAB 10-30 19:55: in production the case runs in the user's workspace, so a file the case writes must not destroy the user's file of
// the same path: each path is backed up first (or marked new) under .qwen-files/<key>/, and put back right after the command (the backup
// restored, a new file and the folders made for it removed), keeping the command's exit code. restoreFiles is the same step alone: the
// harness runs it again after every case (idempotent), so a killed or timed-out command is still undone.
// COLLAB 10-30 22:45 (1): each backup names its own path (<index>.path), so the restore needs no case list; a backup folder left by an earlier
// run (a restore that could not run, e.g. after a cancel) is put back before anything else, and a case does not run while it cannot be.
const quote=text=>`'${String(text).replace(/'/g,`'\\''`)}'`;
const filesDir=key=>`.qwen-files/${String(key).replace(/[^A-Za-z0-9_-]/g,'_')}`;
export const restoreFiles=(_files=[],key='case')=>{const d=filesDir(key);return `if [ -d ${quote(d)} ]; then __ok=0; for __p in ${quote(d)}/*.path; do [ -e "$__p" ] || continue; __i="\${__p%.path}"; __f="$(cat "$__p")"; if [ -e "$__i" ]; then cp -p "$__i" "$__f" || __ok=1; elif [ -e "$__i.new" ]; then rm -f "$__f" || __ok=1; fi; done; `+`if [ -f ${quote(d+'/dirs')} ]; then tac ${quote(d+'/dirs')} | while IFS= read -r __d; do rmdir "$__d" 2>/dev/null; done; fi; [ $__ok = 0 ] && rm -rf ${quote(d)}; exit $__ok; fi`;};
export const withFiles=(command,files=[],key='case')=>{if(!files?.length)return command;const d=filesDir(key);
 const leftover=`if [ -d ${quote(d)} ]; then ( ${restoreFiles([],key)} ) || { echo ${quote('qwen-files: 前の回の files を戻せていません（'+d+'）。case は流していません')} >&2; exit 97; }; fi`;
 const backup=`${leftover}; mkdir -p ${quote(d)} && `+files.map((file,index)=>`{ printf %s ${quote(file.path)} > ${quote(d+'/'+index+'.path')} && if [ -e ${quote(file.path)} ]; then cp -p ${quote(file.path)} ${quote(d+'/'+index)}; else : > ${quote(d+'/'+index+'.new')}; fi; }`).join(' && ');
 const write=files.map(file=>{const dir=file.path.includes('/')?file.path.slice(0,file.path.lastIndexOf('/')):'';return (dir?`{ [ -d ${quote(dir)} ] || { mkdir -p ${quote(dir)} && echo ${quote(dir)} >> ${quote(d+'/dirs')}; }; } && `:'')+`printf %s '${Buffer.from(String(file.content??''),'utf8').toString('base64')}' | base64 -d > ${quote(file.path)}`;}).join(' && ');
 return `${backup} && ${write} && (\n${command}\n); __s=$?; ( ${restoreFiles(files,key)} ) || __s=$?; exit $__s`;};
// The stdouts this task has already seen: from its runs and from earlier Expect cases (agent-guard.mjs keeps both in evidence.commands).
export function observedStdouts(evidence={}){
 return (evidence.commands||[]).flatMap(row=>row.expect?(row.expect.cases||[]).filter(item=>!item.stdoutMatched).map(item=>item.stdout):[row.stdout]).filter(text=>typeof text==='string'&&trimmed(text)!=='');
}
// v63.5-1: the request's test (T1, requestTest) comes on top of the 27B's 8 cases; only CheckCases (planned) carries it
export function checkCases(cases,wrap=bashWithStdin,{planned=false}={}){
 if(!Array.isArray(cases)||!cases.length||(planned?cases.filter(item=>!item?.requestTest):cases).length>expectLimits.cases)return `cases は 1〜${expectLimits.cases} 件です`;
 for(const [index,item] of cases.entries()){
  const at=`cases[${index}]`;
  if(typeof item?.command!=='string'||!item.command.trim()||item.command.length>expectLimits.command)return `${at}.command は 1〜${expectLimits.command} 文字です`;
  if(typeof item.why!=='string'||!item.why.trim()||item.why.length>expectLimits.why)return `${at}.why（依頼のどの条件・境目か）は 1〜${expectLimits.why} 文字です`;
  if(item.stdin!==undefined&&(typeof item.stdin!=='string'||item.stdin.length>expectLimits.stdin))return `${at}.stdin は ${expectLimits.stdin} 文字までです`;
  for(const key of ['expectedStdout','expectedStderr'])if(item[key]!==undefined&&(typeof item[key]!=='string'||item[key].length>expectLimits.expected))return `${at}.${key} は ${expectLimits.expected} 文字までです`;
  if(item.expectedExitCode!==undefined&&!Number.isInteger(item.expectedExitCode))return `${at}.expectedExitCode は整数です`;
  if(wrap(item.command,item.stdin).length>4000)return `${at}: command と stdin を合わせると長すぎます（stdin は UTF-8 と base64 で約 1.4〜4 倍になり、全体で 4000 文字まで）。stdin を短くするか、入力はファイルで渡してください`;
 }
 return null;
}
// run(command) -> {exitCode, stdout, stderr} of one command (workspace.run or the broker's local.run); wrap adds stdin.
// A program that ran into the output or time limit ran: that is a failed case (often the defect itself), not a tool error.
const limitOf=e=>{const text=String(e?.cause?.message??'')+' '+String(e?.message??'');return /output limit exceeded|コマンド出力が1MiBを超え/.test(text)?'output':e?.toolResult?.timedOut===true||/time limit exceeded|設定時間を超え/i.test(text)?'time':null;};
// v63.2 (COLLAB 10-29 14:43 F4, Windows 14:55): the previous Expect's cases (agent-guard.mjs lastExpect()), to tell when a case that did not
// match there now expects the output it got there (the case was rewritten to the program's output, defective or not; only 27B can judge)
const sameCase=(a,b)=>a.command===b.command&&(a.stdin??'')===(b.stdin??'');
// Windows 15:10: whole outputs are compared by their digest (the shown stdout is cut at 400 characters, so a fix after character 400 looked
// like a copy); an output the runner cut is never compared
const digest=text=>createHash('sha256').update(trimmed(text)).digest('hex');
// planned (v63.4): the 27B's cases: each case's own match (numeric allowed), its files written first, stderr compared as a substring (a
// thrown error carries a stack), no exit-code-only note (the 8B cannot change these cases); rows carry the case id
// the command words of a shell line (the first word of each simple command, after VAR=value assignments)
const commandWords=command=>String(command??'').split(/&&|\|\||[;\n|(`]|\$\(/).map(part=>part.trim().split(/\s+/).filter(word=>!/^[A-Za-z_][A-Za-z0-9_]*=/.test(word))[0]).filter(Boolean).map(word=>word.replace(/^.*\//,''));
export async function runExpect({cases,match='trimEnd',run,restoreRun=run,wrap,observed=[],previous=[],signal,planned=false}){
 const original=cases,filesOf=new Map();if(planned)cases=(cases||[]).map((item,index)=>{if(!item?.files?.length)return item;const key=item.id??'case'+(index+1);filesOf.set(index,{files:item.files,key});return {...item,command:withFiles(item.command,item.files,key)};});
 const restoreFailed=[];
 const invalid=checkCases(original,wrap,{planned});if(invalid)return {error:invalid,executed:false,code:'EXPECT_INVALID'};
 if(!matchModes.includes(match))return {error:'match は trimEnd・exact・contains のどれかです',executed:false,code:'EXPECT_INVALID'};
 const seen=observed.map(trimmed),rows=[];
 const undoFiles=async index=>{const {files,key}=filesOf.get(index);let ok=false;try{const r=await restoreRun(restoreFiles(files,key));ok=r?.exitCode===0;}catch{}if(!ok)restoreFailed.push(...files.map(file=>file.path));};
 let storage;
 for(const [index,item] of cases.entries()){
  const base={case:index+1,...(item.id?{id:item.id}:{}),why:item.why,command:clip(planned?original[index].command:item.command,300),...(planned?{source:'planned'}:{}),...(planned&&original[index]?.requestTest?{requestTest:true}:{})},caseMatch=planned&&plannedMatchModes.includes(item.match)?item.match:match;
  let result;try{result=await run(wrap(item.command,item.stdin));}catch(e){
   if(filesOf.has(index))await undoFiles(index);
   // a cancel or a user interruption ends the whole call, exactly as it ends workspace.run; files not put back go with it
   if(signal?.aborted){if(restoreFailed.length&&e&&typeof e==='object')Object.assign(e,{filesNotRestored:[...new Set(restoreFailed)],filesCode:'FILES_NOT_RESTORED'});throw e;}
   const limit=limitOf(e);
   if(limit){const partial=e?.toolResult||{};rows.push({...base,ran:true,pass:false,exitCode:typeof partial.exitCode==='number'?partial.exitCode:null,expectedExitCode:item.expectedExitCode??0,mismatch:{limit:{kind:limit,message:limit==='output'?'出力が上限（1 MiB）を超えて止められました':'時間の上限で止められました'}},stdout:clip(partial.stdout,400),stderr:clip(partial.stderr,200),checkedStdout:item.expectedStdout!==undefined,stdoutMatched:false,copiedFromObserved:false});continue;}
   result={error:e?.message};
  }
  if(filesOf.has(index)&&!result?.restored)await undoFiles(index);
  if(result?.storage)storage=result.storage;
  if(typeof result?.exitCode!=='number'){rows.push({...base,ran:false,pass:false,error:clip(result?.error??'コマンドを実行できませんでした',500)});continue;}
  // v64-1 (COLLAB 11-02 03:12 §4): a command was not found (e.g. `file` in cf23549b, in a pipeline too). The worker's own case whose own
  // command is missing (exit 127) is not run rather than failed: a missing tool, not a mismatch of the program. A 27B case stays a failed
  // case (so Task.DisputeCase can return it, as in production) and carries the fixed answer and a note.
  const missing=item.expectedExitCode!==127?missingCommand(result):null;
  if(!planned&&missing&&result.exitCode===127&&commandWords(item.command).includes(missing.name)){rows.push({...base,ran:false,pass:false,missingCommand:missing,error:clip(missing.instead,500)});continue;}
  const expectedExitCode=item.expectedExitCode??0,checks={exitCode:{pass:result.exitCode===expectedExitCode,expected:expectedExitCode,actual:result.exitCode}};
  if(item.expectedStdout!==undefined)checks.stdout=compareOutput(item.expectedStdout,result.stdout,caseMatch);
  if(item.expectedStderr!==undefined)checks.stderr=compareOutput(item.expectedStderr,result.stderr,planned?'contains':match);
  // copied from an earlier output of this task (COLLAB 23:20): shown, never refused, so the planner can tell such a check apart
  const copiedFromObserved=item.expectedStdout!==undefined&&trimmed(item.expectedStdout)!==''&&seen.includes(trimmed(item.expectedStdout));
  const before=item.expectedStdout===undefined||trimmed(item.expectedStdout)===''?null:previous.find(row=>row.stdoutFailed&&row.actualDigest&&sameCase(row,item)),want=before?digest(item.expectedStdout):null;
  const rewrittenToFailedOutput=!!want&&want===before.actualDigest&&want!==before.expectedDigest;
  const pass=Object.values(checks).every(check=>check.pass);
  // v63.2 (F1): the exit code is as expected and every output that did not match differs only at the very end (exact match). A case also
  // rewritten to the earlier failed output gets only that note (self-review 22:4x: "change neither" contradicted it)
  const endOnly=!rewrittenToFailedOutput&&!pass&&checks.exitCode.pass&&[checks.stdout,checks.stderr].filter(check=>check&&!check.pass).every(check=>/末尾の空白・改行/.test(check.diff?.note??''));
  const plannedCase=planned&&!pass?(({stdin,files,expectedStdout,expectedStderr,match:caseMode,quote})=>({...(stdin!==undefined?{stdin:clip(stdin,300)}:{}),...(files?.length?{files:files.map(file=>({path:file.path,content:clip(file.content,200)}))}:{}),...(expectedStdout!==undefined?{expectedStdout:clip(expectedStdout,300)}:{}),...(expectedStderr!==undefined?{expectedStderr:clip(expectedStderr,200)}:{}),match:caseMode,...(quote?{quote:clip(quote,120)}:{})}))(original[index]):{};
  const caseRow={...base,...plannedCase,ran:true,pass,exitCode:result.exitCode,expectedExitCode,...(pass?{}:{mismatch:Object.fromEntries(Object.entries(checks).filter(([,check])=>!check.pass).map(([key,check])=>[key,key==='exitCode'?{expected:check.expected,actual:check.actual}:check.diff??{expectedText:check.expectedText,actualHead:check.actualHead}]))}),
   stdout:clip(result.stdout,400),stderr:clip(result.stderr,200),...(base.requestTest&&!pass?{failedTests:testFailureLines(result.stdout,result.stderr),failedCount:failingTestCount(result.stdout,result.stderr)}:{}),...(result.truncated?{outputTruncated:true}:{}),checkedStdout:item.expectedStdout!==undefined,stdoutMatched:item.expectedStdout!==undefined&&!!checks.stdout?.pass,copiedFromObserved,...(endOnly?{endOnly}:{}),...(rewrittenToFailedOutput?{rewrittenToFailedOutput}:{})};
  // the whole stdout's digest for the next call (expectCasesForNext); not enumerable, so neither the model nor the logs see it
  if(missing&&!pass)caseRow.missingCommand=missing;
  Object.defineProperty(caseRow,'stdoutDigest',{value:result.truncated?null:digest(result.stdout)});rows.push(caseRow);
 }
 const notRun=rows.filter(row=>!row.ran).length,failed=rows.filter(row=>row.ran&&!row.pass).length;
 // v63.1 (COLLAB 10-29 12:05): notes, never refusals. A case that compared only the exit code passes, but says so (117 of 324 cases
 // in the v63 comparison had no expected output; log-window's "both ends" cases passed with the end line missing). A copied value is
 // fine when the request states it, so the note says how to tell.
 const exitOnly=planned?[]:cases.map((item,index)=>item.expectedStdout===undefined&&item.expectedStderr===undefined&&rows[index]?.ran&&!rows[index]?.mismatch?.limit?index+1:0).filter(Boolean),notes=[];
 const endOnly=rows.filter(row=>row.endOnly).map(row=>row.case),rewritten=rows.filter(row=>row.rewrittenToFailedOutput).map(row=>row.case);
 if(endOnly.length)notes.push(`case ${endOnly.join('・')} は${endOnlyText}`);
 if(rewritten.length)notes.push(`case ${rewritten.join('・')} の期待値は、前の Expect で合わなかった時の実際の出力に書き換えられています。依頼の文の値・規則で言える値でなければ、照合になっていません（欠陥を見落とします）。`);
 const missingRows=rows.filter(row=>row.ran&&row.missingCommand);
 if(missingRows.length)notes.push(`case ${missingRows.map(row=>row.id??row.case).join('・')} では、コマンド（${[...new Set(missingRows.map(row=>row.missingCommand.name))].join('・')}）が見つかりませんでした（missingCommand.instead）。`+(planned?'case の側がこのイメージに無い道具を使っているなら、プログラムの不具合ではないので Task.DisputeCase（kind other）で返します。':''));
 if(exitOnly.length)notes.push(`case ${exitOnly.join('・')} は終了コードだけを比べ、出力は比べていません。依頼が出力を決めているなら expectedStdout（または expectedStderr）を書きます。`);
 // v63.2 (F4): the warning first again, as in v63; a rewritten case has its own note above
 if(rows.some(row=>row.copiedFromObserved&&!row.rewrittenToFailedOutput))notes.push('copiedFromObserved=true の case は、期待値が前に見た実行結果と同じです。出力を見てから書いた値なら照合になっていません。依頼の文に書かれた値・規則から言える期待値なら有効な照合です。');
 // a file that could not be put back is a failure of the tool, named for the planner (COLLAB 19:55)
 if(restoreFailed.length)return {error:`files の戻しに失敗: ${[...new Set(restoreFailed)].join(', ')}`,code:'FILES_NOT_RESTORED',executed:true,pass:false,passed:rows.filter(row=>row.pass).length,failed,notRun,match,cases:rows,restoreFailed:[...new Set(restoreFailed)]};
 return {pass:!notRun&&!failed,passed:rows.filter(row=>row.pass).length,failed,notRun,match,cases:rows,executed:rows.some(row=>row.ran),...(storage?{storage}:{}),
  ...(notRun?rows.filter(row=>!row.ran).every(row=>row.missingCommand)?{error:`${notRun} 件の case が、このイメージに無いコマンド（${[...new Set(rows.filter(row=>row.missingCommand).map(row=>row.missingCommand.name))].join('・')}）を使っています（照合の不一致ではありません）。`,code:'EXPECT_TOOL_MISSING'}
   :{error:`${notRun} 件のコマンドを実行できませんでした（照合の不一致ではありません）。path とコマンドを直してください`,code:'EXPECT_NOT_RUN'}:{}),
  ...(notes.length?{note:notes.join(' ')}:{})};
}
// The first difference of a case in a few words (the resume fingerprint reads this summary, so each kind of difference shows)
const lineText=(name,diff)=>diff?.word?` ${name} ${diff.word} 語目 期待 ${JSON.stringify(clip(diff.expected,100))} 実際 ${JSON.stringify(clip(diff.actual,100))}`:diff?.line?` ${name} ${diff.line} 行目 期待 ${JSON.stringify(clip(diff.expected,100))} 実際 ${JSON.stringify(clip(diff.actual,100))}`:diff?` ${name} に ${JSON.stringify(clip(diff.expectedText,60))} が無い（実際 ${JSON.stringify(clip(diff.actualHead,100))}）`:'';
// v63.5-1: a failed T1 says which tests failed (the 400 characters of stdout rarely reach them)
// v63.6-3: 700 (was 400) so the first failing test keeps the source lines under its frames
const failedTestsText=row=>row.failedTests?` 落ちた試験: ${clip(row.failedTests,700)}`:'';
const mismatchText=row=>row.mismatch?.limit?` ${row.mismatch.limit.message}`:row.mismatch?(lineText('stdout',row.mismatch.stdout)+lineText('stderr',row.mismatch.stderr)+(row.mismatch.exitCode?` exit 期待 ${row.mismatch.exitCode.expected} 実際 ${row.mismatch.exitCode.actual}`:'')):row.error?' 実行できず':'';
// v63.1 (Windows 12:45): what the planner is shown for an Expect evidence row instead of its exitCode. The row's exitCode is 0/1 for
// "every case matched / some did not", and the planner read 1 as the program crashing (v63-1-discussion-1 P4); each case keeps the
// program's own exit code in its case row.
// v63.2 (Windows 14:55): with the cases rewritten to an earlier failed output, which 27B can check against the request
export function expectLabel(row){const e=row?.expect||{},total=(e.cases||[]).length||(e.passed||0)+(e.failed||0)+(e.notRun||0),bad=(e.failed||0)+(e.notRun||0);
 const rewritten=e.rewrittenToFailedOutput?.length?`。case ${e.rewrittenToFailedOutput.join('・')} の期待値は、前に合わなかった時の実際の出力に書き換えられています（依頼の値か確かめてください）`:'';
 return (bad?`合わない case ${bad} 件（Expect ${e.passed||0}/${total} 一致）`:`全 case 一致（Expect ${e.passed||0}/${total} 一致）`)+rewritten;}
// v63.2: every case that did not match differs only at the very end
// v63.3 (iv-a) (COLLAB 10-30 13:20): a case that fails again after a line-changing write is re-derived from the rules before the next edit
// (self-review 10-30: a task without rules is told to use the request only; the one line goes in the next write's reason, where the guard reads it)
export const reDeriveAdvice=cases=>`case ${cases.join('・')} は、行を変える書き換えの後にも、また合いませんでした。同じ Expect をやり直す前に、その case の期待値を、依頼の文から導き直します。導けなければ case を直すか外します。導けたなら、プログラムのどこが依頼に反するかを、次の書き換えの reason に 1 行で書いてから直します。`;
// v63.3 B (COLLAB 10-30 13:55, 12:50): the newest Expect decides. Returns the newest Expect row when it did not match, else null
// (a later Run, such as a build, is not a check).
export function newestExpectFailed(commands=[]){const row=(commands||[]).findLast(item=>item?.expect);return row&&row.exitCode!==0?row:null;}
export function onlyEndDiffers(row){const e=row?.expect,bad=(e?.cases||[]).filter(item=>!item.pass);return !!e&&!e.notRun&&bad.length>0&&bad.every(item=>item.ran&&item.endOnly);}
// v63.2: what the guard keeps of one Expect call for the next one (runExpect previous): the case's command and stdin, and the digests of its
// expected stdout and of the whole stdout it got (none when the runner cut it, or for a case that hit a limit)
export function expectCasesForNext(action,result){
 return (action.cases||[]).map((item,index)=>{const row=result.cases?.[index];return {command:item.command,stdin:item.stdin,expectedDigest:item.expectedStdout===undefined?null:digest(item.expectedStdout),actualDigest:row?.stdoutDigest??null,stdoutFailed:!!row?.ran&&!!row.checkedStdout&&!row.stdoutMatched};});}
// One evidence row per Expect call (agent-guard.mjs): exitCode 0 only when every case passed, so the existing finish and readiness
// checks ("the last command must have succeeded") treat a failed check like a failed command.
export function expectEvidence(tool,action,result){
 // review t1-4: a failed T1 comes first (its failing tests would otherwise fall behind three failed cases)
 const cases=result.cases||[],failedRows=cases.filter(row=>!row.pass),bad=[...failedRows.filter(row=>row.requestTest),...failedRows.filter(row=>!row.requestTest)],rewritten=cases.filter(row=>row.rewrittenToFailedOutput).map(row=>row.case);
 const summary=`Expect ${result.passed}/${cases.length} 一致`+(bad.length?'。合わない case: '+bad.slice(0,3).map(row=>`${row.id??row.case}（${clip(row.why,60)}）`+mismatchText(row)+failedTestsText(row)).join('、'):'');
 return {tool,command:clip((action.cases||[]).map(item=>item.command).join(' ; '),1200),exitCode:result.pass?0:1,stdout:clip(summary,1200),stderr:'',
  expect:{passed:result.passed,failed:result.failed,notRun:result.notRun,...(rewritten.length?{rewrittenToFailedOutput:rewritten}:{}),
   cases:cases.map(row=>({case:row.case,...(row.id?{id:row.id}:{}),why:clip(row.why,100),pass:row.pass,ran:row.ran,exitCode:row.exitCode,expectedExitCode:row.expectedExitCode,stdout:row.stdout,stdoutMatched:!!row.stdoutMatched,copiedFromObserved:row.copiedFromObserved,...(row.endOnly?{endOnly:true}:{})}))}};
}
