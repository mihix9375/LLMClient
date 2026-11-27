// C (COLLAB 11-04 21:35; verification/c-shortpath/design.md, c1-safety.md section 7): Planner.DirectWork, the path where the restricted 27B does a
// very small job itself. The harness bounds it: an envelope declared first (kind, files, characters), checked before anything runs; a child loop on
// the worker verification path with the planner's own model, a per-kind tool list, and limits on operations, seconds, wasted steps, paths and
// characters; for code the cases are made before the first write and run by the harness before the finish counts. Any failed or unknown check
// fails closed to a hand-off (the planner continues, the files stay). This module is pure text and arithmetic (no model call, no HTTP): agent.mjs
// wires it. Off unless config.directPath.enabled (default false; the master decides after the measurements); also off with config.app (s-v71).
import {dataExts,normalPath} from './case-plan.mjs';
import {safeFilePath} from './expect.mjs';
import {readdir,readFile,stat} from 'node:fs/promises';
import {join} from 'node:path';
import {createHash} from 'node:crypto';
export const directKinds=Object.freeze(['inspect','search','copy','create','edit']);
export const directTitle='直接作業（27B）';
// the one constants table (COLLAB 11-04 21:35): the envelope as the Windows side answered it. ops = tool operations a child may run (a refusal is not
// one; CheckCases is); seconds = summed step seconds from acceptance (planCases included); wasted = refusals, duplicates, truncations.
export const directLimits=Object.freeze({files:2,writeFile:1500,writeTotal:3000,command:400,runSeconds:60,checkSeconds:120,wasted:2,abortGraceSeconds:60,
 workspaceFiles:6,workspaceChars:3000,diskFiles:8,diskChars:6000,handoffChars:1500,refusals:2,
 kinds:Object.freeze({inspect:Object.freeze({ops:4,seconds:150}),search:Object.freeze({ops:4,seconds:150}),copy:Object.freeze({ops:4,seconds:150}),create:Object.freeze({ops:6,seconds:240}),edit:Object.freeze({ops:6,seconds:240})})});
const scalarKeys=Object.keys(directLimits).filter(key=>key!=='kinds');
// config.directPath: {enabled:false (default), forced:false (measurement only: with the branch offered the schema is the branch and Task.Finish, see
// agent.mjs), limits:{key:number,...,kinds:{create:{ops,seconds}}}}; a limit may be set from 1 up to 4 times its default
export function directPathConfig(value){
 if(value===undefined||value===null)return {enabled:false};
 if(typeof value!=='object'||Array.isArray(value)||Object.keys(value).some(key=>!['enabled','forced','limits'].includes(key)))throw Error('Invalid directPath');
 if(value.enabled!==undefined&&typeof value.enabled!=='boolean'||value.forced!==undefined&&typeof value.forced!=='boolean')throw Error('Invalid directPath');
 const out={enabled:value.enabled===true,...(value.forced===true?{forced:true}:{})},bound=(number,base)=>Number.isInteger(number)&&number>=1&&number<=base*4;
 if(value.limits!==undefined){const l=value.limits;if(!l||typeof l!=='object'||Array.isArray(l))throw Error('Invalid directPath.limits');const kept={};
  for(const [key,item] of Object.entries(l)){
   if(key==='kinds'){if(!item||typeof item!=='object'||Array.isArray(item))throw Error('Invalid directPath.limits.kinds');kept.kinds={};
    for(const [kind,row] of Object.entries(item)){if(!directKinds.includes(kind)||!row||typeof row!=='object'||Object.keys(row).some(name=>!['ops','seconds'].includes(name))||Object.entries(row).some(([name,number])=>!bound(number,directLimits.kinds[kind][name])))throw Error('Invalid directPath.limits.kinds.'+kind);kept.kinds[kind]=row;}}
   else if(scalarKeys.includes(key)&&bound(item,directLimits[key]))kept[key]=item;else throw Error('Invalid directPath.limits.'+key);}
  out.limits=kept;}
 return out;
}
export const directLimitsOf=config=>{const l=config?.directPath?.limits??{};return {...directLimits,...l,kinds:Object.fromEntries(directKinds.map(kind=>[kind,{...directLimits.kinds[kind],...(l.kinds?.[kind]??{})}]))};};
// the three conditions that do not change inside a job (so the system text stays one text per job): the flag, a restricted planner, no app
export const directConfigured=(config,restricted)=>!!restricted&&config?.directPath?.enabled===true&&!config.app;
// the per-call condition for the schema branch and the directPath input key: nothing delegated, no tiny or assist, no earlier session, fewer than
// 2 refused declarations, not closed. The branch leaves the schema when any of this changes; the system text does not change.
export const directBranch=({config,restricted,state})=>directConfigured(config,restricted)&&config.modelRole!=='worker'&&!state.lastDelegation&&!state.plannerDelegation&&!state.delegationResume&&!state.tinyUsed&&!state.assistedDelegation&&!state.plannerGrant&&!state.direct&&!state.directClosed&&(state.directRefused||0)<directLimitsOf(config).refusals;
// ---- the codes (v65 shape {error(string), code, rule, fix(<=120), next(<=40), executed:false}) ----
export const directRules=Object.freeze({D1:'D1 申告が枠内',D2:'D2 作業場が小さい',D3:'D3 依頼から確認 case が作れる',D4:'D4 操作・秒・書込字数・申告したパス',D5:'D5 直接作業を引き継ぐ'});
export const directFixes=Object.freeze({
 DIRECT_REFUSED:'枠の外の大きさ。申告を枠内に直して 1 回だけやり直すか、2 つ以上の component に分けて Worker.Delegate する。',
 DIRECT_UNVERIFIABLE:'確認 case が作れず何も書いていない。分けて委譲するか、未達を報告する。',
 DIRECT_LIMIT:'この操作は枠の外で実行していない。申告したパスと上限の中でやり直すか、報告して終える。',
 DIRECT_HANDOFF:'枠の中で終わらなかった。書いた物は作業場に残っている。残りを分けて渡すか、未達を報告する。'});
export const directNexts=Object.freeze({DIRECT_REFUSED:'申告を直す（1 回まで）か Worker.Delegate',DIRECT_UNVERIFIABLE:'Worker.Delegate か Task.Finish(false)',DIRECT_LIMIT:'枠内でやり直すか finish(achieved=false)',DIRECT_HANDOFF:'残りを分けて Worker.Delegate'});
const quoteOf=value=>{const text=typeof value==='string'?value:JSON.stringify(value)??String(value);return text.length>80?text.slice(0,80)+'…':text;};
export const directResult=(code,rule,why,{field,quote,extra}={})=>({error:({DIRECT_REFUSED:'直接作業は始めていません',DIRECT_UNVERIFIABLE:'直接作業は始めていません。何も書いていません',DIRECT_LIMIT:'この操作は実行していません',DIRECT_HANDOFF:'直接作業を引き継ぎます'})[code]+'（'+why+'）',code,rule,...(field?{field}:{}),...(quote!==undefined?{quote:quoteOf(quote)}:{}),fix:directFixes[code],next:directNexts[code],executed:false,...(extra??{})});
// ---- E2 the declaration, E3 the workspace ----
export const directCodeFile=path=>!new RegExp(`\\.(${dataExts})$`,'i').test(String(path));
const cleanPath=(path,normalize)=>{if(typeof path!=='string')return null;const p=normalPath(normalize(path.trim()));return p&&p!=='.'&&safeFilePath(p)?p:null;};
export const directWrites=kind=>kind==='create'||kind==='edit';
// -> {decl} or {refusal}; step = the Planner.DirectWork arguments; normalize = agent.mjs workspacePath (/workspace/x is x); capabilities = the PC's tools
export function directEntry({step,limits,normalize=path=>path,capabilities=[]}){
 const no=(why,field,value)=>({refusal:directResult('DIRECT_REFUSED',directRules.D1,why,{field,quote:value})});
 if(!directKinds.includes(step?.kind))return no('kind は inspect・search・copy・create・edit のどれか','kind',step?.kind);
 if(!Array.isArray(step.files)||step.files.some(file=>typeof file!=='string'))return no('files は書く・写すパスの配列','files',step.files);
 if(step.files.length>limits.files)return no('files は '+limits.files+' 個まで','files',step.files);
 const files=[...new Set(step.files.map(file=>cleanPath(file,normalize)))];
 if(files.includes(null))return no('files のパスは作業場からの相対パス（英数字と _ . / - だけ、80 字まで）','files',step.files);
 const kind=step.kind,why=typeof step.whySmall==='string'?step.whySmall.slice(0,200):'';
 if(directWrites(kind)){
  // C review wf_cacfe850-d72 S3: create and edit write the server's /workspace only; a declared target other than 'workspace' (or none) is refused, so a PC-destined request cannot end achieved with an empty PC
  if(step.target!=='workspace')return no(kind+' は鯖の作業場 /workspace にだけ書けます（target=workspace）。PC のファイルは copy（鯖で作った物を PC へ）か Worker.Delegate の target=local にします','target',step.target);
  if(!files.length)return no(kind+' は書くファイルを 1 つ以上 files に書く','files',step.files);
  if(!files.some(directCodeFile))return no('データだけの書込は対象外（コードのファイルが 1 つ以上要る）','files',files);
  if(!Number.isInteger(step.writeChars)||step.writeChars<0)return no('writeChars は書く字数の見積り（0 以上の整数）','writeChars',step.writeChars);
  if(step.writeChars>limits.writeTotal||step.writeChars>limits.writeFile*files.length)return no('書く字数は 1 ファイル '+limits.writeFile+' 字まで、全部で '+limits.writeTotal+' 字まで','writeChars',step.writeChars);
 }else if(kind==='copy'){
  if(!files.length)return no('copy は写すファイルのパスを files に書く','files',step.files);
  if(!capabilities.some(tool=>tool==='workspace.fetch'||tool==='local.fetch'))return no('いまは PC と鯖の間の複写が使えない','kind',kind);
 }
 return {decl:{kind,files,writeChars:directWrites(kind)?step.writeChars:0,whySmall:why}};
}
// E3: every regular file by content (a NUL byte = binary, counted as a file only); a big file counts by its size. C review wf_cacfe850-d72 S1: dot-files and
// node_modules count too (only the sandbox's own .home at the root does not), and the walk also returns `digest` (path -> hash of the content, 'big:<size>' over
// 64 KiB, 'link' for a symlink or other non-regular entry), so a run's changes outside the declared files can be seen. Bounded: 2,000 entries, depth 8
// (over either, truncated=true: E3 refuses the workspace and the finish check cannot pass).
export async function workspaceSize(root,{depth=8,limit=2000}={}){let files=0,chars=0,binary=0,seen=0,truncated=false;const digest=new Map();
 const walk=async(dir,level,prefix)=>{if(level>depth){truncated=true;return;}let entries=[];try{entries=await readdir(dir,{withFileTypes:true});}catch{return;}
  for(const entry of entries){if(level===0&&entry.name==='.home')continue;if(++seen>limit){truncated=true;return;}const full=join(dir,entry.name),rel=prefix+entry.name;
   if(entry.isDirectory()){await walk(full,level+1,rel+'/');continue;}
   files++;if(!entry.isFile()){digest.set(rel,'link');continue;}
   try{const info=await stat(full);if(info.size>65536){chars+=info.size;digest.set(rel,'big:'+info.size);continue;}const bytes=await readFile(full);digest.set(rel,createHash('sha256').update(bytes).digest('hex').slice(0,16));if(bytes.includes(0))binary++;else chars+=bytes.toString('utf8').length;}catch{digest.set(rel,'unreadable');}}};
 await walk(root,0,'');return {files,chars,binary,truncated,digest};}
// S1: what a declared session may touch: its declared files (a copy may also name the destination by the basename)
export const directDeclared=child=>new Set(child.kind==='copy'?child.files.flatMap(file=>[file,file.split('/').at(-1)]):child.files);
// S1: one workspace.run: the digest before and after (both from workspaceSize); a path outside the declared files that is new, changed or gone is kept in
// state.directTrack as {path,before,now} (before = the hash when it was first seen changing, null = absent); a later run that puts it back clears it
export function directTrack(state,before,after,declared){const rows=state.directTrack??=[];
 for(const path of new Set([...before.digest.keys(),...after.digest.keys()])){if(declared.has(path))continue;const a=before.digest.get(path)??null,b=after.digest.get(path)??null;if(a===b)continue;
  const row=rows.find(item=>item.path===path);if(row)row.now=b;else if(rows.length<100)rows.push({path,before:a,now:b});else state.directTrackFull=true;}
 state.directTrackTruncated=!!after.truncated;return directUndeclared(state);}
// -> [{path,change:'new'|'changed'|'gone'}]: what is outside the declared files now (a walk that was cut short cannot say, so it counts as a change)
export const directUndeclared=state=>[...(state.directTrack??[]).filter(row=>row.before!==row.now).map(row=>({path:row.path,change:row.before===null?'new':row.now===null?'gone':'changed'})),...(state.directTrackTruncated||state.directTrackFull?[{path:'（作業場が大きく全部は数えられません）',change:'changed'}]:[])];
const changeText={new:'新規',changed:'変更',gone:'削除'};
export const directUndeclaredText=list=>list.slice(0,4).map(row=>row.path+'（'+changeText[row.change]+'）').join('・')+(list.length>4?' ほか '+(list.length-4)+' 件':'');
export const directWorkspaceRefusal=(size,limits)=>size.truncated||size.files>limits.workspaceFiles||size.chars>limits.workspaceChars?directResult('DIRECT_REFUSED',directRules.D2,'作業場が大きい（'+size.files+' ファイル・'+size.chars+' 字。上限は '+limits.workspaceFiles+' ファイル・'+limits.workspaceChars+' 字）',{field:'workspace',quote:size.files+' files / '+size.chars+' chars'}):null;
// ---- the child: tools, limits, gate ----
export const directKindTools=Object.freeze({inspect:['workspace.list','workspace.read','workspace.ocr','workspace.view_image','local.list','local.read','local.search'],search:['web.search'],copy:['workspace.fetch','local.fetch','workspace.list','local.list'],
 create:['workspace.list','workspace.read','workspace.write','workspace.run','workspace.check'],edit:['workspace.list','workspace.read','workspace.write','workspace.run','workspace.check']});
export const directChildTools=kind=>[...(directKindTools[kind]??[]),'finish'];
// -> null (inside the envelope) or {limit:'ops'|'seconds'|'wasted'}: from here the next call offers only Task.Finish
export function directStop({child,state}){const row=child.limits.kinds[child.kind];
 if((state.directOps||0)>=row.ops)return {limit:'ops'};if((state.directUsedMs||0)>=row.seconds*1000)return {limit:'seconds'};if((state.directWasted||0)>=child.limits.wasted)return {limit:'wasted'};return null;}
const limitText={ops:'操作の上限',seconds:'時間の上限',wasted:'無駄な手（断られた・重複・出力切れ）の上限'};
export const directAvailable=(available,child,state)=>directStop({child,state})?['finish']:[...new Set([...available.filter(tool=>child.tools.includes(tool)),'finish'])];
// L, before every operation: nothing is executed when it returns a result (the caller counts it as a wasted step)
export function directGate({child,state,step,normalize=path=>path}){
 if(step.tool==='finish')return null;
 const L=child.limits,limit=(why,extra)=>directResult('DIRECT_LIMIT',directRules.D4,why,extra),declared=new Set(child.files);
 if(!child.tools.includes(step.tool))return limit(child.kind+' では '+step.tool+' は使えません');
 const stop=directStop({child,state});if(stop)return limit(limitText[stop.limit]+'に達しました（Task.Finish だけが選べます）',{extra:{limit:stop.limit}});
 if(step.tool==='workspace.write'){const path=cleanPath(step.path,normalize);
  if(!path||!declared.has(path))return limit('書けるのは申告したパスだけです（'+child.files.join('・')+'）',{field:'path',quote:step.path});
  if(String(step.text??'').length>L.writeFile)return limit('1 回の書込は '+L.writeFile+' 字までです（'+String(step.text).length+' 字）',{field:'text',quote:String(step.text).length+' chars'});}
 if(step.tool==='workspace.fetch'||step.tool==='local.fetch'){
  if(step.overwrite===true)return limit('overwrite=true は使えません（PC のファイルは戻せません）',{field:'overwrite',quote:true});
  const allowed=new Set(child.files.flatMap(file=>[file,file.split('/').at(-1)])),path=cleanPath(step.path,normalize),to=step.to?cleanPath(step.to,normalize):'';
  if(!path||!allowed.has(path)||to===null||(to&&!allowed.has(to)))return limit('写せるのは申告したパスだけです（'+child.files.join('・')+'）',{field:'path',quote:step.path});}
 if(step.tool==='workspace.run'){const command=String(step.command??'');
  if(command.length>L.command||/[\r\n]/.test(command))return limit('コマンドは 1 行で '+L.command+' 字までです',{field:'command',quote:command.length+' chars'});}
 return null;
}
export const directOperationSeconds=(child,tool)=>tool==='workspace.check'?child.limits.checkSeconds:tool==='workspace.run'?child.limits.runSeconds:null;
// F: the checks the child's own finish adds on top of the worker's (cases, command, written files)
export function directFinishChecks({child,state,size,existing}){const L=child.limits,out=[];
 if(directWrites(child.kind)){
  for(const path of child.files)out.push({type:'file_exists',path,passed:existing.has(path),...(existing.has(path)?{}:{error:'申告した '+path+' がまだ作業場にありません。書いてから finish します。'})});
  const within=!size.truncated&&size.files<=L.diskFiles&&size.chars<=L.diskChars;
  // C review wf_cacfe850-d72 S1: a run that made, changed or deleted anything outside the declared files (dot-files and node_modules included) is not a pass
  const declaredNow=directDeclared(child),viaEntry=Object.entries(child.entry??{}).filter(([path,hash])=>!declaredNow.has(path)&&size.digest.get(path)!==hash).map(([path])=>({path,change:size.digest.has(path)?'changed':'gone'})),undeclared=[...directUndeclared(state),...viaEntry.filter(row=>!directUndeclared(state).some(x=>x.path===row.path))];out.push({type:'direct_workspace_unchanged',path:'申告したファイルの外の変更なし',passed:!undeclared.length,...(undeclared.length?{error:'申告したファイルの外が変わっています: '+directUndeclaredText(undeclared)+'。このコマンドで作った物は rm で消してから finish します。元に戻せない変更（削除・書き換えた既存ファイル）は achieved=false で何が変わったかを報告します。'}:{})});
  out.push({type:'direct_disk_size',path:'作業場 '+size.files+' ファイル・'+size.chars+' 字（上限 '+L.diskFiles+' ファイル・'+L.diskChars+' 字）',passed:within,...(within?{}:{error:'作業場が枠（'+L.diskFiles+' ファイル・'+L.diskChars+' 字）を超えています。余分なファイルを消すか、achieved=false で報告します。'})});}
 if(child.kind==='copy')out.push({type:'fetch_verified',path:child.files.join('・'),passed:state.directFetch?.verified===true,...(state.directFetch?.verified===true?{}:{error:'SHA-256 が合った複写（verified=true）がまだありません。写してから finish します。'})});
 return out;
}
// the parent: a direct session that did not end achieved (handoff, closed by an interruption, cancelled, limit) blocks achieved=true until a delegation succeeded
// that has an implement or verify task covering the session's declared files (C review wf_cacfe850-d72 S2: an inspection-only delegation does not lift it; a
// session that declared no file, inspect/search, is lifted by any successful delegation). 'running' and 'done' are not blocks (the parent's own check at the end of a passed child runs while it is 'running').
export const directUnfinished=state=>['handoff','closed','cancelled','limit'].includes(state.direct?.status);
// -> true when the delegation (its tasks, as the planner sent them) has an implement or verify task, a task without a phase counting as implement, that names a declared file (path or base name) in its title, instruction, acceptance or checks
export function directCovers(direct,tasks){const names=(direct?.files??[]).flatMap(file=>[file,file.split('/').at(-1)]);if(!names.length)return true;
 return (Array.isArray(tasks)?tasks:[]).some(task=>['implement','verify'].includes(task?.phase??'implement')&&names.some(name=>[task.title,task.instruction,task.acceptance,...(task.checks||[]).flatMap(check=>[check?.text,check?.path])].some(text=>typeof text==='string'&&text.includes(name))));}
export const directHandoffCheck=(state,claim)=>claim!==false&&directUnfinished(state)?[{type:'direct_handoff_resolved',path:'直接作業の引き継ぎ（'+(state.direct.files?.length?state.direct.files.join('・')+' を実装または検証する task を含む委譲を Worker.Delegate で成功させる':'残りを Worker.Delegate で完了させる')+'）',passed:state.lastDelegation?.achieved===true&&(!state.direct.files?.length||state.lastDelegation.covers===true)}]:[];
// ---- what the user and the planner read ----
const cut=(text,max)=>{const value=String(text??'');return value.length>max?value.slice(0,max)+'…':value;};
export const sha12=text=>createHash('sha256').update(String(text)).digest('hex').slice(0,12);
// the first line where the case's expected output and the actual one differ
export function firstDiff(item,last){const expected=String(item?.expectedStdout??item?.expectedStderr??'exit '+(item?.expectedExitCode??0)),actual=String(item?.expectedStdout!==undefined?last?.stdout:item?.expectedStderr!==undefined?last?.stderr:'exit '+last?.exitCode);
 const a=expected.replace(/\r\n/g,'\n').split('\n'),b=actual.replace(/\r\n/g,'\n').split('\n'),at=a.findIndex((line,index)=>line.trimEnd()!==(b[index]??'').trimEnd());const k=at<0?Math.min(a.length,b.length):at;
 return '期待 '+cut(a[k]??'（なし）',60)+' / 実際 '+cut(b[k]??'（なし）',60);}
// the child's closing summary (agent.mjs puts it in the result): the stats and the failed cases
export function directSummary({child,state,guardState,cases=[]}){const last=guardState?.plannedLast||{};
 return {kind:child.kind,files:child.files,ops:state.directOps||0,usedMs:state.directUsedMs||0,wasted:state.directWasted||0,limit:directStop({child,state})?.limit??null,written:state.directFiles??{},undeclared:directUndeclared(state).slice(0,6),copiedBy:state.directFetch?.tool??null,
  caseCount:cases.length,passed:cases.filter(item=>last[item.id]?.pass).length,comparedPassed:cases.filter(item=>last[item.id]?.pass&&(item.expectedStdout!==undefined||item.expectedStderr!==undefined)).length,requestTest:cases.some(item=>item.requestTest),requestTestPassed:cases.some(item=>item.requestTest&&last[item.id]?.pass),
  failedCases:cases.filter(item=>last[item.id]&&!last[item.id].pass).slice(0,6).map(item=>({id:item.id,rule:cut(item.rule,60),firstDiff:firstDiff(item,last[item.id])}))};
}
// the observation the planner gets when the session did not end the job (<= handoffChars characters)
export function directHandoff({summary,written=[],failedChecks=[],stopReason,limits}){
 const body={written,failedCases:summary.failedCases,...(summary.undeclared?.length?{undeclared:summary.undeclared.slice(0,4)}:{}),...(failedChecks.length?{failedChecks:failedChecks.map(({type,path})=>({type,path:cut(path,80)}))}:{}),limit:summary.limit??(stopReason&&stopReason!=='not_achieved'?stopReason:null)};
 const make=()=>directResult('DIRECT_HANDOFF',directRules.D5,'操作 '+summary.ops+'・'+Math.round(summary.usedMs/1000)+' 秒で終わりませんでした',{extra:{...body,executed:true}});
 let out=make();while(JSON.stringify(out).length>limits.handoffChars&&(body.failedCases.length>1||body.written.length>1||body.failedChecks?.length>1)){if(body.failedCases.length>1)body.failedCases=body.failedCases.slice(0,-1);else if(body.written.length>1)body.written=body.written.slice(0,-1);else body.failedChecks=body.failedChecks.slice(0,-1);out=make();}
 if(JSON.stringify(out).length>limits.handoffChars){body.failedCases=body.failedCases.map(row=>({...row,firstDiff:cut(row.firstDiff,40)}));out=make();}
 return out;
}
// 「直接作業の記録: ...」 written by the harness, never by the 27B
export function directAnswerBlock({child,summary}){const files=Object.entries(summary.written??{}),chars=files.reduce((n,[,row])=>n+(row.chars||0),0);
 const checked=directWrites(child.kind)?`確認: case ${summary.caseCount} 件中 ${summary.passed} 件合格（出力比較 ${summary.comparedPassed} 件）。依頼の試験: ${summary.requestTest?(summary.requestTestPassed?'合格':'不合格'):'なし'}。未確認: 隠し試験なし（case は 27B が依頼から書いた物）`:child.kind==='copy'?'確認: ファイルの大きさと SHA-256 を照合した複写（verified）。置き場所: '+(summary.copiedBy==='local.fetch'?'PC':'鯖の作業場 /workspace'):'確認: 実行結果の読み取りのみ（case なし）';
 return `直接作業の記録: 27B が直接実行（操作 ${summary.ops}/${child.limits.kinds[child.kind].ops}・${Math.round(summary.usedMs/1000)}/${child.limits.kinds[child.kind].seconds} 秒${directWrites(child.kind)?`・鯖の作業場 /workspace に書込 ${files.length} ファイル ${chars} 字`:''}）。${checked}${directWrites(child.kind)?'。申告外の変更: '+(summary.undeclared?.length?directUndeclaredText(summary.undeclared):'なし'):''}`;
}
// ---- the texts the 27B reads ----
// the card appended to the restricted planner's system text (only with the flag on); the limits are in the input key directPath.limits
export const directCard='分けられないほど小さい作業は、委譲せずに自分でやる（Planner.DirectWork。何も実行・委譲していない間だけ、依頼に 1 回）。kind は inspect（読む・探す）/ search（Web）/ copy（1 ファイルを PC と鯖の間で写す）/ create・edit（鯖の /workspace にだけ書く。target=workspace。PC のファイルは copy か Worker.Delegate の target=local。1〜2 ファイルのコード）。files は書く・写すパス（2 つまで）、writeChars は書く字数の見積り。上限は directPath.limits。create・edit は、ハーネスが先に依頼から確認 case を作り、finish の前に流して全部合うまで達成にしない。枠に収まらない作業は Worker.Delegate（実装は 2 つ以上の component）。';
export const directPathInput=limits=>({limits:{...Object.fromEntries(directKinds.map(kind=>[kind,limits.kinds[kind].ops+'操作・'+limits.kinds[kind].seconds+'秒'])),files:limits.files,writeCharsPerFile:limits.writeFile,writeCharsTotal:limits.writeTotal}});
// the child's system text (agent-protocol.mjs adds the shared first line and the cards of its tools)
export function directInstructions(child){const L=child.limits,row=L.kinds[child.kind],write=directWrites(child.kind);
 return [`あなたは27Bです。依頼の中の「分けられないほど小さい作業」だけを、直接作業の枠の中で自分で実行します（kind=${child.kind}）。8Bへの委譲、27Bへの相談、ユーザーへの質問、case への異議はありません。`,
  `申告したパス: ${child.files.join('、')||'なし'}。上限は操作${row.ops}回・${row.seconds}秒・無駄な手${L.wasted}回です。上限に達するとTask.Finishだけが選べます。`,
  write?`書けるのは鯖の作業場 /workspace の申告したパスだけで、PC には書けません。全文を書きます（追記はありません。1回${L.writeFile}字まで）。コマンドは1行${L.command}字まで・${L.runSeconds}秒までで、申告したパスの外（作業場のほかのファイル・dot-file・node_modules）を作らず・変えず・消しません。コマンドで作った物は rm で消してから終えます。ハーネスが依頼から確認case（C1…）を先に作りました。WorkspaceShell.CheckCasesで流し、全部通してからachieved=trueで終えます。caseは変えられません。落ちたら期待と実際の最初の違いを読み、プログラムを直します。`
   :child.kind==='copy'?`写すのは申告したパスだけで、overwrite=trueは使えません。複写はSHA-256が合って初めて成功です。PCの相対パス「.」はlocalEnvironment.workspaceRootで、/workspaceは鯖だけです。`
   :child.kind==='search'?'見つけた事実には出典のURLを添えます。リンク先の本文を読んだとは書きません。':`PCの相対パス「.」はlocalEnvironment.workspaceRootで、/workspaceは鯖だけです。読んだ事実だけをanswerに書きます。`,
  '実行していないテストを合格と言いません。出力上限の警告が出たら1回のJSONを小さくします。枠に収まらない・直せない・分からない時は、無理に進めず、分かったことと残りをanswerに書いてachieved=falseで終えます（ハーネスが残りを分けて委譲します）。'].join('\n');}
// the child's first message (input.goal)
export function directGoal({goal,child,cases=[]}){
 return `直接作業（27B）: ${child.whySmall||'小さな作業'}\n依頼: ${goal}\n種類: ${child.kind}\n申告したパス: ${child.files.join('、')||'なし'}${directWrites(child.kind)?'（鯖の作業場 /workspace）':''}${cases.length?'\n確認case: '+cases.map(item=>item.id+' '+item.rule).join(' / ')+'\nWorkspaceShell.CheckCasesで流し、全部通ってからfinishします。caseは変えられません。':''}\nどうしても分からない点は、分かったことと残りをfinishのanswerに書き、achieved=falseで終えます。`;
}
// ---- the scrub: the child has no Task.DisputeCase and no 8B to hand a case back to ----
// whole sentences that recommend the dispute are cut (plannedText, the CheckCases card and recipe say it in one clause or one sentence), and
// 「同じ8Bで」 is removed from the finish push-back (the repair instruction stays)
const disputeClauses=[[/, or use Task\.DisputeCase if the case contradicts the request/g,''],[/, or returned with Task\.DisputeCase when it contradicts the request/g,''],[/同じ8Bで/g,'']];
const disputeWord=/Task\.DisputeCase|task\.dispute|DisputeCase|同じ8B/;
// C review wf_cacfe850-d72 n5: two more things the child cannot do (it has no WorkspaceShell.Expect, and the gate refuses overwrite=true): they are cut from the cards, recipes and fix texts it reads
const childOnly=[[/ Add your own WorkspaceShell\.Expect only for what the 27B cases do not check\./g,''],[/, or set overwrite=true only if replacing it is part of the task/g,'']];
export function scrubForDirect(text){
 if(typeof text!=='string')return text;
 let out=text;for(const [pattern,by] of childOnly)out=out.replace(pattern,by);
 if(!disputeWord.test(out))return out;
 for(const [pattern,by] of disputeClauses)out=out.replace(pattern,by);
 if(disputeWord.test(out))out=out.split(/(?<=。)|(?<=\. )/).filter(sentence=>!disputeWord.test(sentence)).join('');
 return out;
}
export const scrubValue=value=>typeof value==='string'?scrubForDirect(value):Array.isArray(value)?value.map(scrubValue):value&&typeof value==='object'?Object.fromEntries(Object.entries(value).map(([key,item])=>[key,scrubValue(item)])):value;
// C review wf_cacfe850-d72 F1: the scrub touches only what the harness wrote (the system text, the cards and recipes, plannedText, the case index line: the keys below
// of the child's input), never the user's goal, a file the child reads (content), command output (stdout, stderr), the file listing or a web page. A key name decides, not
// the text, so a file that mentions Task.DisputeCase reaches the child as it is.
export const harnessKeys=Object.freeze(['error','note','message','fix','next','warning','instruction','nextDecision','completionReadiness','decisionSupport','circuitBreaker','failedChecks','options','guidance','hint','repairInstruction']);
export const scrubHarness=value=>Array.isArray(value)?value.map(scrubHarness):value&&typeof value==='object'?Object.fromEntries(Object.entries(value).map(([key,item])=>[key,harnessKeys.includes(key)?scrubValue(item):scrubHarness(item)])):value;
