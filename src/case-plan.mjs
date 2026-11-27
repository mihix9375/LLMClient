// v63.4 (COLLAB 10-30 18:10, 18:50; verification/v63-4-design.md §2): after the plan, the 27B writes the verify task's cases in a call of
// its own (planCases), so a thinking budget can be set for this call only and the measurement driver imports this same function.
// Each case: a rule with its quote from the request, a command in the request's own invocation (no pipe, no echo, no build), stdin, files
// written by the harness, the expected output and exit code, and its own match (numeric for numbers whose display the request leaves open).
// A lint that runs nothing (bash -n parses only) drops malformed cases before any worker sees them; more than half dropped, or no case
// comparing output, asks the 27B once more; still none means no cases (the worker's own path), reported, never silent.
import {spawnSync} from 'node:child_process';
import {bashWithStdin,withFiles,safeFilePath,expectLimits} from './expect.mjs';
// the 27B's match: no exact (gate 1, 10-30 19:24: 28 cases failed on the correct version only for the missing last newline)
export const caseMatchModes=['trimEnd','contains','numeric'];
export const caseLimits=Object.freeze({cases:8,rule:120,quote:120,command:400,stdin:600,files:3,path:80,content:600,expectedStdout:400,expectedStderr:200,build:300});
const text=(max,min=1)=>({type:'string',minLength:min,maxLength:max});
export const caseSchema={type:'object',properties:{rule:text(caseLimits.rule),quote:text(caseLimits.quote),command:text(caseLimits.command),stdin:{type:'string',maxLength:caseLimits.stdin},// required (gate 1: 31 cases left it out for a program that reads stdin)
 
 files:{type:'array',maxItems:caseLimits.files,items:{type:'object',properties:{path:text(caseLimits.path),content:{type:'string',maxLength:caseLimits.content}},required:['path','content'],additionalProperties:false}},
 expectedStdout:{type:'string',maxLength:caseLimits.expectedStdout},expectedStderr:{type:'string',maxLength:caseLimits.expectedStderr},expectedExitCode:{type:'integer'},match:{type:'string',enum:caseMatchModes}},
 required:['rule','quote','command','stdin','expectedExitCode','match'],additionalProperties:false};
export const casesSchema={type:'object',properties:{build:{type:'string',maxLength:caseLimits.build},cases:{type:'array',maxItems:caseLimits.cases,items:caseSchema}},required:['cases'],additionalProperties:false};
export const casesInstruction=`あなたは 27B の計画役です。依頼の文と計画から、統合検証（verify）の担当が流す case を、最大 8 つ JSON で書きます。担当はこの case を変えられません。依頼どおりの正しいプログラムが、すべての case を通る必要があります。
各 case には、確かめる規則（rule、120 字以内）と、その規則が書かれた依頼の文の箇所（quote、依頼の文の写し）を付けます。
・command は、依頼の呼び出し（プログラム名・引数・オプション）のとおりに書きます。stdin で読むプログラムには、入力を必ず stdin の欄に書きます（ファイルや引数で読むプログラムでは、stdin は空文字です）。入力を pipe で渡す依頼の例も、入力は stdin の欄に移し、command には pipe も echo も書きません。ファイルで読むプログラムには、試しのファイルを files の欄に書きます（各行を改行で終えます）。ビルドが要る時は、build の欄に 1 回だけ書き、command には書きません。
・関数を export するモジュールは、node --input-type=module -e "import {f} from './m.mjs'; console.log(f(1))" のように値を出します。投げる例外は、try/catch で e.name を出して確かめます。
・期待値は、その command が出す出力の全体です（一部の行だけを期待値にしません。1 行だけを確かめたい時は、files で 1 行だけの入力を作ります。truncated のファイルを読む時だけは下のとおりです）。期待値の値は、依頼の文にある値か、依頼の規則から導いた値です。書式の名前や記号（#RRGGBB、N、<点数> など）は値ではありません。expectedExitCode はいつも書きます（エラーは 0 以外、それ以外は 0）。
・match は普通 trimEnd です（出力の最後の空白・改行は比べません）。依頼が数の表示の形（桁・小数点）を決めていない値は match=numeric にします。依頼が空白・区切り・大文字小文字を決めていない時は、出力の 1 行全体を trimEnd で比べられる値にするか、その case を書きません。contains は、長い語句が出ることを確かめる時だけ使います（数や 2 文字以下の語には使いません）。
・input.workspaceFiles のファイルは作業場にあります（頭 40 行と全体の行数 lines を見せています）。そのファイルを読む case は、依頼の呼び方のままそのファイルを読み、files には書きません。期待値は、見せた中身から導きます。truncated のファイル（全部は見せていない物）の出力は、全体を期待値にしません。見せた範囲で確かな 1 行を match=contains で確かめるか、終了コードで確かめます。head -n 40 などで入力を変えません。
・作業場に無いファイルを読む case では、files に新しい名前で中身を書きます（作業場にある名前には書きません）。引数を省くと既定のファイルを読むプログラムで、その既定のファイルが作業場に無い時も同じです。ファイルの行の形は、依頼の文の例の形か workspaceFiles の形をそのまま使い、例に無い記号（コロンなど）を足しません。
・依頼に写せる文が無い振る舞い（依頼に出てこない区切り記号・見出し行・入力の行の形）や、試験の実行（node --test など）は case にしません。入力の形は、依頼が例にしている形だけを使います。迷う case は書かず、8 つに満たなくて構いません。出力を比べる case（expectedStdout か expectedStderr がある物）を、1 つ以上入れます。
例 1（依頼「count.sh を作る。bash count.sh <ファイル> で、空行を除いた行の数を出す」、workspaceFiles に {"path":"data.txt","lines":3,"head":["ann 80","","bob 70"],"truncated":false} がある時）:
{"rule":"空行は数えない","quote":"空行を除いた行の数を出す","command":"bash count.sh data.txt","stdin":"","expectedStdout":"2","expectedExitCode":0,"match":"trimEnd"}
例 3（同じ依頼で、作業場に無い新しい入力を作る時）:
{"rule":"1 行だけのファイル","quote":"空行を除いた行の数を出す","command":"bash count.sh one.txt","stdin":"","files":[{"path":"one.txt","content":"cid 90\\n"}],"expectedStdout":"1","expectedExitCode":0,"match":"trimEnd"}
例 2（依頼「add.cpp を作る。g++ -O2 -o add add.cpp でビルドし、標準入力の 2 つの整数の和を出す」の時。build は "g++ -O2 -o add add.cpp"）:
{"rule":"2 つの整数の和","quote":"標準入力の 2 つの整数の和を出す","command":"./add","stdin":"3 4\\n","expectedStdout":"7","expectedExitCode":0,"match":"numeric"}`;
// C (COLLAB 11-04 21:35; c-shortpath/design.md section 4): the same call for the 27B's own 1-2 file program (src/direct-path.mjs): only the first
// two sentences differ (the cases are not for a verify worker, the program does not exist yet); everything else is casesInstruction, so a change there
// reaches both. A changed first paragraph throws at import.
const casesIntro='あなたは 27B の計画役です。依頼の文と計画から、統合検証（verify）の担当が流す case を、最大 8 つ JSON で書きます。担当はこの case を変えられません。依頼どおりの正しいプログラムが、すべての case を通る必要があります。';
if(!casesInstruction.startsWith(casesIntro+'\n'))throw Error('casesInstruction changed: directCasesInstruction must be derived again');
export const directCasesInstruction='あなたは 27B の計画役です。依頼の文から、これから 27B 自身が書く 1〜2 ファイルのプログラムを確かめる case を、最大 8 つ JSON で書きます。プログラムはまだ書かれていません。書いた後は 27B もこの case を変えられません。依頼どおりの正しいプログラムが、すべての case を通る必要があります。'+casesInstruction.slice(casesIntro.length);
// (2) (COLLAB 10-30 20:25, 20:50; design §2.5): the data files already in the workspace are shown to the 27B, the first 40 lines and the
// line count (200 characters a line, 6,000 in all), so its cases read them instead of guessing their contents. Program sources, hidden
// files and anything that may hold a secret (.env, keys, credentials, passwords) are never collected.
export const workspaceLimits=Object.freeze({lines:40,lineChars:200,totalChars:6000,fileBytes:65536});
const sourceExt=/\.(cpp|cc|cxx|c|h|hh|hpp|sh|bash|zsh|mjs|cjs|js|jsx|ts|tsx|py|rb|go|rs|java|kt|php|pl|o|obj|a|so|out|exe|class)$/i;
// v63.5-2 (COLLAB 10-31 03:15): one list for the files shown to the 27B and the files the lint looks for (the lint missed .ini and .conf)
// C (COLLAB 11-04 21:35): dataExts is exported so the direct path (src/direct-path.mjs) classifies a file as code by the same list
export const dataExts='txt|log|csv|tsv|dat|in|json|md|xml|yaml|yml|ini|conf|cfg';const dataExt=new RegExp(`\\.(${dataExts})$`,'i');
// every part of the path is checked (a file under secrets/ or .ssh/ is not collected either; review design-11)
export const secretName=name=>String(name).split('/').filter(Boolean).some(secretPart);
const secretPart=base=>{
 return base.startsWith('.')||/(^|[._-])(env|secrets?|credentials?|passwords?|passwd|htpasswd|netrc|npmrc|pypirc|git-credentials|private)([._-]|$)/i.test(base)
  ||/\.(pem|key|p12|pfx|kdbx|jks|keystore|crt|cer|der|gpg|asc|ppk)$/i.test(base)||/^id_(rsa|dsa|ecdsa|ed25519)/i.test(base)||/(api|access|auth|secret)[._-]?keys?/i.test(base);};
export function candidateNames({goal='',design,task,topLevel=[]}={}){
 const text=[goal,design?.summary,...(design?.components||[]).map(row=>`${row.interface??''} ${row.responsibility??''}`),task?.instruction,task?.acceptance].map(value=>String(value??'')).join('\n');
 const named=[...text.matchAll(/[A-Za-z0-9._\/-]+\.[A-Za-z0-9]{1,5}(?![A-Za-z0-9])/g)].map(match=>match[0].replace(/^\.\//,''));
 return [...new Set([...named,...topLevel.filter(name=>dataExt.test(name))])].filter(name=>safeFilePath(name)&&!sourceExt.test(name)&&!secretName(name));
}
// read(path) -> {text, size} or null when the workspace has no such file
// what a credential looks like in the lines that would be shown: a private key, key/secret/password/token followed by : or =, a Bearer
// header, or a well-known key prefix. Such a file is named with its size only (review design-11)
export const secretContent=text=>/-----BEGIN [A-Z ]*PRIVATE KEY-----|\b(api[_-]?key|secret|passw(or)?d|passwd|token|auth[_-]?key|access[_-]?key|private[_-]?key|client[_-]?secret)\b\s*[:=]|\bBearer\s+[A-Za-z0-9._~+\/-]{8,}|\b(sk-[A-Za-z0-9]{8,}|AKIA[0-9A-Z]{12,}|gh[pousr]_[A-Za-z0-9]{20,}|xox[abpr]-[A-Za-z0-9-]{10,})/i.test(String(text));
// v63.6 (COLLAB 10-31 13:40, 13:45): the workspace files a judgement (judgeCase, reviewSameFlaw) shows the 27B: the planner's own view when it
// wrote the cases (the workspaceInputs output it got, secret-like and unshown files as there), each file read again with the same function;
// a file that changed since is shown as it is now and marked changedSinceCases (gone: no longer readable). read: as for workspaceInputs.
export async function judgeWorkspaceFiles(planned=[],read){
 if(typeof read!=='function'||!planned.length)return planned;
 const now=new Map((await workspaceInputs({candidates:planned.map(file=>file.path),read})).map(file=>[file.path,file]));
 return planned.map(file=>{const current=now.get(file.path);if(!current)return {path:file.path,gone:true,changedSinceCases:true};return JSON.stringify(current)===JSON.stringify(file)?file:{...current,changedSinceCases:true};});
}
export async function workspaceInputs({candidates=[],read}){
 const out=[];let total=0;
 for(const path of candidates){if(total>=workspaceLimits.totalChars)break;let file=null;try{file=await read(path);}catch{}if(!file)continue;
  if(file.size>workspaceLimits.fileBytes||/\u0000/.test(String(file.text))){out.push({path,size:file.size,shown:false});continue;}
  if(secretContent(String(file.text).split('\n').slice(0,workspaceLimits.lines).join('\n'))){out.push({path,size:file.size,shown:false,reason:'secret-like'});continue;}
  const all=String(file.text).replace(/\r\n/g,'\n').split('\n');if(all.at(-1)==='')all.pop();
  const head=[];let cutLine=false;for(const line of all.slice(0,workspaceLimits.lines)){const shown=line.length>workspaceLimits.lineChars?(cutLine=true,line.slice(0,workspaceLimits.lineChars)+'…'):line;if(total+shown.length>workspaceLimits.totalChars){cutLine=true;break;}head.push(shown);total+=shown.length;}
  out.push({path,lines:all.length,head,truncated:cutLine||head.length<all.length});}
 return out;
}
// the lint: why a case is dropped (null = kept). Nothing is run: bash -n only parses.
const testRun=/\bnode\s+--test\b|\bnpm\s+(run\s+)?test\b|\bpytest\b|\bgo\s+test\b/,pipeIntoCompiler=/\|\s*(g\+\+|gcc|clang\+?\+?|make)(?=\s|$)/,echoPipe=/(^|[;&|(]\s*)(echo|printf)\b[^|]*\|/;
const nodeNoPrint=command=>/\bnode\b[^|]*(\s-e\b|--eval|--input-type)/.test(command)&&!/console\.(log|error)|process\.std(out|err)/.test(command);
const parses=script=>spawnSync('bash',['-n'],{input:script,timeout:3000}).status===0;
const dataName=new RegExp(`(?:^|[\\s'"=<])([A-Za-z0-9._\\/-]+\\.(?:${dataExts}))(?=$|[\\s'";|&)>])`,'gi');
// v63.5-2: a case may read a missing file on purpose only when it expects an error and its rule or quote speaks of a missing file
// (v63.4 (c): "= でない行は" is not about a file; noeq.conf's INVALID and tzero.conf's ERROR came from contents the 27B never wrote)
// review lint-0/2: the word must follow ファイル/file within 3 characters ("設定ファイルの = でない行は" is not about a missing file)
export const missingFileWords=/(ファイル|file)[^。\n]{0,3}?(無い|無く|無けれ|無かっ|ない|なく|なけれ|なかっ|存在し|見つから|不存在|不在|not\s*found|missing|does\s*n[o']?t\s*exist|no\s*such)|(無い|存在しない|見つからない|missing|nonexistent|non-existent|no\s+such)[^。\n]{0,6}?(ファイル|file)/i;
const missingOnPurpose=item=>(item.expectedExitCode??0)!==0&&missingFileWords.test(`${item.rule??''}\n${item.quote??''}`);
export const normalPath=path=>String(path??'').replace(/\/+/g,'/').replace(/^(\.\/)+/,'').replace(/\/(\.\/)+/g,'/').replace(/\/$/,'');
// every file of a workspace folder as a relative path, for overwrites_existing (dot folders included; node_modules and .qwen-files skipped)
export async function workspacePaths(root,{limit=5000,depth=8}={}){const {readdir}=await import('node:fs/promises'),{join,relative,sep}=await import('node:path'),out=[];let seen=0;
 const walk=async(dir,level)=>{if(level>depth)return;let entries=[];try{entries=await readdir(dir,{withFileTypes:true});}catch{return;}
  for(const entry of entries){if(++seen>limit)return;if(entry.name==='node_modules'||entry.name==='.qwen-files'||entry.isSymbolicLink())continue;const full=join(dir,entry.name);if(entry.isDirectory())await walk(full,level+1);else out.push(relative(root,full).split(sep).join('/'));}};
 await walk(root,0);return out;}
export function caseProblem(item,{goal='',workspaceFiles=[],existing=[]}={}){
 const shown=new Map(workspaceFiles.map(file=>[file.path,file])),present=new Set([...existing,...shown.keys()].map(normalPath));
 if(!item||typeof item!=='object'||typeof item.command!=='string'||!item.command.trim()||typeof item.rule!=='string'||typeof item.quote!=='string'||!Number.isInteger(item.expectedExitCode))return 'schema';
 if(!caseMatchModes.includes(item.match))return 'match';
 if(item.files!==undefined&&(!Array.isArray(item.files)||item.files.length>caseLimits.files||item.files.some(file=>!safeFilePath(file?.path)||typeof file.content!=='string')))return 'file_path';
 // (2): a case never writes over a file the workspace has, at any depth, nor over a folder (the backup and restore stay as the safety net;
 // COLLAB 10-30 22:45 (1c): existing is every file of the workspace, not only the top level)
 if((item.files||[]).some(file=>{const path=normalPath(file.path);return present.has(path)||[...present].some(name=>name.startsWith(path+'/'));}))return 'overwrites_existing';
 if(testRun.test(item.command))return 'test_run';
 if(pipeIntoCompiler.test(item.command))return 'pipe_into_compiler';
 if(echoPipe.test(item.command))return 'echo_pipe';
 if(item.expectedStdout!==undefined&&nodeNoPrint(item.command))return 'node_without_print';
 if(!parses(item.command))return 'syntax';
 const provided=new Set((item.files||[]).map(file=>normalPath(file.path)));
 // a missing file on purpose is fine when the case expects the error and says why (gate (2): "bash grade.sh nosuchfile.txt" -> usage, exit 1)
 // review lint-1: a name is compared as a normalised path (./bad.conf is bad.conf), the redirect target too
 for(const [,written] of item.command.matchAll(dataName)){const name=normalPath(written);if(!missingOnPurpose(item)&&!provided.has(name)&&!present.has(name)&&!new RegExp(`>\\s*['"]?(\\./)*${name.replace(/[.*+?^${}()|[\]\\]/g,'\\$&')}`).test(item.command))return 'missing_file';}
 // (2): a file shown only in part gives no whole output: one line with contains, or the exit code
 if([...shown.values()].some(file=>file.truncated&&item.command.includes(file.path))&&item.expectedStdout!==undefined&&(item.match!=='contains'||String(item.expectedStdout).trim().includes('\n')))return 'truncated_whole';
 if(item.expectedStdout===undefined&&item.expectedStderr===undefined&&item.expectedExitCode===0)return 'checks_nothing';
 if(item.match==='contains'&&item.expectedStdout!==undefined&&(String(item.expectedStdout).trim().length<=2||/^[+-]?[\d.]+$/.test(String(item.expectedStdout).trim())))return 'weak_contains';
 if(bashWithStdin(withFiles(item.command,item.files),item.stdin).length>4000)return 'too_long';
 return null;
}
// v63.9 (COLLAB 11-03 12:15 (1), loop 9e85e642): a compiler call in a build names its input. 「x86_64-w64-mingw32-g++ で…」 gave a bare
// `g++` (the request pattern had no left boundary); every CheckCases stopped at "no input files" and the verify task went round 597 steps.
// A compiler call (g++, gcc, clang++, clang, c++, cc, with a target prefix, a version or a path) needs a word that is neither an option nor an
// option's value (the word after -o, -I, -x ...): a source, a header, a module, an object, $SRCS, @args or a glob. make and other commands are
// not looked at. Quoted text is one word and a backslash-newline continues the line (review wf_7aa6ee63-402)
const compilerCall=/^(?:[A-Za-z_][A-Za-z0-9_]*=\S*\s+)*(?:\S*\/)?(?:[\w.]+-)*(?:g\+\+|gcc|clang\+\+|clang|c\+\+|cc)(?:-\d+(?:\.\d+)*)?(?=\s|$)/;
const separateArg=new Set(['-o','--output','-I','-L','-l','-x','-include','-imacros','-isystem','-iquote','-idirafter','-MF','-MT','-MQ','-Xlinker','-Xpreprocessor','-Xassembler','-T','-u','-e']);
const commandParts=build=>String(build??'').replace(/\\\n/g,' ').replace(/"[^"]*"|'[^']*'/g,quoted=>quoted.replace(/[\s;&|]/g,'_')).split(/&&|\|\||[;\n|]/);
export const buildWithoutInput=build=>commandParts(build).some(part=>{const call=part.trim().match(compilerCall);if(!call)return false;
 const words=part.trim().slice(call[0].length).trim().split(/\s+/).filter(Boolean);return !words.some((word,index)=>(word==='-'||!word.startsWith('-'))&&!separateArg.has(words[index-1]));});
// the request's build: a whole command word (x86_64-w64-mingw32-g++, /usr/bin/g++), never the end of a longer word (cmake, mingw32-g++ alone).
// Every mention is tried in turn (「g++ は 13 以上。g++ -o add add.cpp でビルド」 takes the second); a compiler build ends before the first plain
// word that is not an option's value or a command after && ; | ("... add.cpp and verify that ./add" stops before "and"); make followed by
// more than two plain words is prose ("make sure that ..."), not a build
const requestBuild=/(?<![\w.+\/-])(?:[\w.\/-]*[\/-])?(?:g\+\+|gcc|clang\+\+|make)(?=\s)[\x20-\x5f\x61-\x7e]*/g;
const plainWord=/^[A-Za-z]+$/,operator=/^(?:&&|\|\||;|\|)$/;
function requestBuildText(run){
 const words=run.trim().split(/\s+/).filter(Boolean);if(!words.length)return null;
 if(/(?:^|\/)make$/.test(words[0]))return words.slice(1).filter(word=>plainWord.test(word)).length>2?null:words.join(' ');
 const kept=[words[0]];for(let i=1;i<words.length;i++){const word=words[i],prev=words[i-1];if(plainWord.test(word)&&!separateArg.has(prev)&&!operator.test(prev))break;kept.push(word);}
 while(kept.length>1&&operator.test(kept.at(-1)))kept.pop();
 return kept.join(' ').replace(/[.,:]$/,'');
}
export function requestBuildOf(goal){let skipped=null;
 for(const match of String(goal??'').matchAll(requestBuild)){const build=requestBuildText(match[0]);if(!build||!parses(build))continue;if(buildWithoutInput(build)){skipped??=build;continue;}return {build,skipped:null};}
 return {build:null,skipped};}
// v63.9 (COLLAB 11-03 12:15 (2)): a cases build that failed on its own line, not on the program. Only a build of compiler calls (with cd or
// mkdir), and only the compiler's and the linker's own lines (gcc quotes the program's source in a diagnostic; another command prints its
// own errors); any file:line: error is the program's. no input files only when the line itself names no input (clang says it after a
// missing source); command not found only for the compiler word with exit 127 (review wf_7aa6ee63-402)
const toolLine=/^(?:\S*\/)?(?:[\w.]+-)*(?:g\+\+|gcc|clang\+\+|clang|c\+\+|cc|cc1plus|cc1|collect2|ld(?:\.\w+)?)(?:-\d+(?:\.\d+)*)?: /;
export function buildLineFault(build){
 if(!build||build.exitCode===0)return null;
 const command=String(build.command??''),parts=commandParts(command).map(part=>part.trim()).filter(Boolean);
 if(!parts.length||!parts.every(part=>compilerCall.test(part)||/^(?:cd|mkdir)(?:\s|$)/.test(part)))return null;
 const lines=`${build.stderr??''}\n${build.stdout??''}`.split('\n').map(line=>line.trim());
 if(lines.some(line=>/:\d+(?::\d+)?: (?:fatal )?error\b/.test(line)))return null;
 const own=lines.filter(line=>toolLine.test(line));
 if(own.some(line=>/error: no input files\b/.test(line))&&buildWithoutInput(command))return 'no_input';
 if(own.some(line=>/error: (?:unrecogni[sz]ed (?:command[- ]line )?option|unknown argument)\b/.test(line)))return 'unknown_option';
 if(own.some(line=>/cannot find -l\S+/.test(line)))return 'library_not_found';
 const compilers=parts.map(part=>part.match(compilerCall)?.[0].trim().split(/\s+/).at(-1)).filter(Boolean);
 if(build.exitCode===127&&lines.some(line=>compilers.some(word=>line.endsWith(word+': command not found')||line.endsWith(word+': not found'))))return 'command_not_found';
 return null;
}
// quotes are compared without whitespace, backticks and quotation marks (the request writes its examples in backticks)
const normalized=text=>String(text??'').normalize('NFKC').replace(/[\s`'"「」『』]/g,'');
export function lintCases(reply,{goal='',workspaceFiles=[],existing=[]}={}){
 const raw=Array.isArray(reply?.cases)?reply.cases.slice(0,caseLimits.cases):[],dropped=[],kept=[],quoteMisses=[];
 let build=typeof reply?.build==='string'&&reply.build.trim()?reply.build.trim():undefined,buildDropped=null,buildFromRequest=false,requestBuildSkipped=null;
 if(build&&(!parses(build)||pipeIntoCompiler.test(build)||echoPipe.test(build)||buildWithoutInput(build))){buildDropped=build;build=undefined;}
 // gate v2 (10-30 19:50): a missing build (8 cases ran ./hexval that was never built) falls back to the request's own build command
 // v63.9: requestBuildOf (above); a compiler call without its input is not taken (recorded as requestBuildSkipped)
 if(!build){const fromRequest=requestBuildOf(goal);if(fromRequest.build){build=fromRequest.build;buildFromRequest=true;}else if(fromRequest.skipped)requestBuildSkipped=fromRequest.skipped;}
 raw.forEach((item,index)=>{const problem=caseProblem(item,{goal,workspaceFiles,existing});if(problem){dropped.push({index:index+1,reason:problem,case:item});return;}
  if(!normalized(goal).includes(normalized(item.quote)))quoteMisses.push(index+1);kept.push(item);});
 const cases=kept.map((item,index)=>({id:'C'+(index+1),...item})),comparing=cases.some(item=>item.expectedStdout!==undefined||item.expectedStderr!==undefined);
 return {build,buildDropped,buildFromRequest,...(requestBuildSkipped?{requestBuildSkipped}:{}),cases:comparing?cases:[],dropped,quoteMisses,raw:raw.length,retry:!raw.length||dropped.length*2>raw.length||!comparing,reason:!raw.length?'none':!comparing?'no_output_case':null};
}
// v63.5-1 (COLLAB 10-31 02:17 3.1, 03:15): the test the request names (node --test X, npm test, pytest, go test) is run by the harness as
// one more case of CheckCases, T1: the exit code only, never disputed, added only next to the 27B's cases and only when its files are in the
// workspace (a missing one is recorded). The 27B's own cases still never run a test (the test_run lint above). v63.4 ran it in 1 of 24 runs.
export const requestTestId='T1';
// a file argument ends where the path characters end (a backtick, a quote, Japanese text or a sentence's full stop may follow at once)
const requestTests=[
 {kind:'node',pattern:/\bnode\s+--test((?:\s+(?:--[\w-]+(?:=[^\s`'"]+)?|[\w.\/-]+\.(?:m?js|cjs|mts|ts)(?![\w\/-]|\.\w)))*)/},
 {kind:'npm',pattern:/\bnpm\s+(?:run\s+)?test\b()/},
 {kind:'pytest',pattern:/\b(?:python3?\s+-m\s+)?pytest((?:\s+(?:-[\w-]+|[\w.\/-]+\.py(?![\w\/-]|\.\w)|[\w.-]+\/(?:[\w.-]+\/)*(?![\w.\/-])))*)/},
 {kind:'go',pattern:/\bgo\s+test((?:\s+(?:-[\w-]+|\.\/\.\.\.(?![\w.\/-])|\.\/[\w.\/-]*(?![\w.\/-])))*)/}];
// COLLAB 10-31 04:20 (a): the files of T1 that a verify or review task may not change: the files the command names (under a named folder,
// its test files); when the command names none (npm test, node --test, go test ./...), the test files in the workspace at planCases
const testFilePattern=/(^|\/)(test\/.+\.[cm]?[jt]s|[^\/]*[._-]test\.[cm]?[jt]s|test-[^\/]*\.[cm]?js|test\.[cm]?js|test_[^\/]*\.py|[^\/]*_test\.py|[^\/]*_test\.go)$/;
const testFilesOf=(args,present)=>{const named=args.map(normalPath).filter(Boolean),all=[...present];
 return [...new Set(named.length?named.flatMap(path=>present.has(path)?[path]:all.filter(name=>name.startsWith(path+'/')&&testFilePattern.test(name))):all.filter(name=>testFilePattern.test(name)))].sort();};
// what the runner would pick up when the request names no file (node's own default patterns, pytest's and go's)
const defaultTests={node:/(^|\/)(test\/.+\.[cm]?js|[^\/]*[._-]test\.[cm]?js|test-[^\/]*\.[cm]?js|test\.[cm]?js)$/,pytest:/(^|\/)(test_[^\/]*|[^\/]*_test)\.py$/,go:/_test\.go$/};
export function requestTestCase({goal='',existing=[]}={}){
 const text=String(goal??''),present=new Set((existing||[]).map(normalPath));
 for(const {kind,pattern} of requestTests){const match=text.match(pattern);if(!match)continue;
  const command=match[0].replace(/\s+/g,' ').trim(),args=(match[1]||'').trim().split(/\s+/).filter(arg=>arg&&!arg.startsWith('-')&&arg!=='./...').map(arg=>arg.replace(/\/\.\.\.$/,''));
  const at=match.index,sentence=text.slice(Math.max(0,text.lastIndexOf('。',at)+1),(text.indexOf('。',at)+1||text.length+1)-1).trim(),quote=sentence.length<=120?sentence:command;
  // review t1-2: the match must be the whole command. Right after it: an ASCII character other than a closing mark (test:unit), a token that
  // is quoted or globbed, or a value after an option (pytest -k foo) mean it is longer than this reading; such a test is recorded, not run
  const rest=text.slice(at+match[0].length),lastToken=(match[1]||'').trim().split(/\s+/).at(-1)??'';
  if(/^[\x21-\x7e]/.test(rest)&&!/^[`'")\].,;:!?]\s|^[`'")\].,;!?]$|^[`'")\].,;]/.test(rest)||/^[ \t]+(?:["'*]|[^\s`'"]*[*?])/.test(rest)||lastToken.startsWith('-')&&/^[ \t]+[\x21-\x7e]/.test(rest)&&!/^[ \t]+[`'")\]]/.test(rest)){
   const whole=(text.slice(at).match(/^[\x20-\x7e]+/)||[command])[0].replace(/[`]+.*$/,'').trim();return {command:whole.slice(0,200),quote,unparsed:true};}
  // a mention the request turns down (npm test は実行しないで、pytest は使わず) is not a test to run
  if(/^[\s`'"」』）)]*[はをも]?[^。、]{0,8}?(使わ|しない|せず|しなく|不要|禁止)/.test(rest))return {command,quote,negated:true};
  const missing=kind==='npm'?(present.has('package.json')?[]:['package.json'])
   :args.length?args.filter(arg=>{const path=normalPath(arg);return !present.has(path)&&![...present].some(name=>name.startsWith(path+'/'));})
   :[...present].some(name=>defaultTests[kind]?.test(name))?[]:[kind==='node'?'*.test.js|*.test.mjs':kind==='pytest'?'test_*.py':'*_test.go'];
  if(missing.length)return {command,quote,missing};
  return {command,quote,testFiles:testFilesOf(kind==='npm'?[]:args,present),case:{id:requestTestId,rule:'依頼の試験が全部通る',quote,command,stdin:'',expectedExitCode:0,match:'trimEnd',requestTest:true}};}
 return null;
}
// the same for production (agent.mjs) and the driver (v51-ab.mjs): T1 joins the 27B's cases; the record says when the request's test is missing
export function withRequestTest(plan,{goal='',existing=[]}={}){
 const test=requestTestCase({goal,existing}),cases=plan?.cases||[];
 const requestTest=test?{command:test.command,added:!!test.case&&cases.length>0,...(test.case&&cases.length?{testFiles:test.testFiles}:{}),...(test.missing?{missing:test.missing,note:'依頼の試験が無い'}:test.unparsed?{note:'依頼の試験の命令を読み取れない'}:test.negated?{note:'依頼が試験を使わないと言っている'}:!cases.length?{note:'27B の case が無い'}:{})}:null;
 return {...plan,cases:requestTest?.added?[...cases,test.case]:cases,requestTest};
}
// COLLAB 10-31 04:20 (a): T1's files as they are at planCases (read(path) -> text or null; null: unreadable or over 64 KiB, so only refused,
// never put back). The same for production (agent.mjs) and the driver (v51-ab.mjs).
export async function testFileSnapshot(paths=[],read){const out=[];for(const path of paths){let content=null;try{content=await read(path);}catch{}out.push({path,content:typeof content==='string'?content:null});}return out;}
// the call: the request, the plan's design, the verify task and the implement tasks; its own thinking budget and output size
// maxTokens 6,144 (gate 1: thinking over 3,000 tokens cut the answer at 4,096)
// design §5 (10-30 22:36): the thinking cap of the case call. Arms on the 8 train tasks: 1,371 (the P50 of the 4,096 arm) and 2,238 (its P75)
// both passed the gate in every repetition and no task did worse than 4,096; the rule of §5.4 takes the smaller (v63-4-quality/summary.json)
export const casesReasoningBudget=1371;
// C: the instruction is a defaulted parameter; the default is casesInstruction, byte for byte (test/v63-4.test.mjs pins it); the direct path passes directCasesInstruction
export async function planCases({model,goal,design,task,implementTasks=[],workspaceFiles=[],existing=[],reasoningBudgetTokens=casesReasoningBudget,maxTokens=6144,signal,log,instruction=casesInstruction}){
 const input={request:String(goal??''),design:design?{summary:design.summary,components:(design.components||[]).map(({id,name,interface:api})=>({id,name,interface:api}))}:undefined,
  verify:{title:task?.title,instruction:task?.instruction,acceptance:task?.acceptance},implement:implementTasks.map(row=>({title:row.title,component:row.component})),workspaceFiles};
 const options={signal,log,stage:'planCases',maxTokens,...(reasoningBudgetTokens!==undefined?{reasoningBudgetTokens}:{})},calls=[];
 let reply=await model.chat(instruction,JSON.stringify(input),casesSchema,options),lint=lintCases(reply,{goal,workspaceFiles,existing});calls.push({reply,lint:{dropped:lint.dropped.map(({index,reason})=>({index,reason})),raw:lint.raw}});
 if(lint.retry){reply=await model.chat(instruction,JSON.stringify(input),casesSchema,options);lint=lintCases(reply,{goal,workspaceFiles,existing});calls.push({reply,lint:{dropped:lint.dropped.map(({index,reason})=>({index,reason})),raw:lint.raw}});}
 return {build:lint.build,buildDropped:lint.buildDropped,buildFromRequest:lint.buildFromRequest,...(lint.requestBuildSkipped?{requestBuildSkipped:lint.requestBuildSkipped}:{}),cases:lint.cases,dropped:lint.dropped,quoteMisses:lint.quoteMisses,casesMissing:lint.cases.length?null:lint.reason??'no_output_case',attempts:calls.length,calls};
}
