// v64-1 (COLLAB 11-02 03:12 §4): fixed answers about the workspace image's commands.
// (1) A command that is not found: the 8B and the consultant kept trying file / xxd that the image lacks (cf23549b: 31 of 35 steps, the
//     consultant advised xxd 4 times, often inside pipelines that still exit 0 or 1). Each "X: command not found" in stderr gets one fixed
//     answer naming what to use instead; a name that is not a known image tool may be the task's own program called without ./.
// (2) mingw -static: a dynamic x86_64-w64-mingw32 link needs libstdc++-6.dll / libgcc_s_seh-1.dll on the PC (cf23549b step 13 overwrote a
//     working static snake.exe with a 179 KB dynamic one). -static-libgcc -static-libstdc++ is not enough (libwinpthread-1.dll stays).
const NOT_FOUND=/(?:^|\n)[^\n]*?: ([A-Za-z0-9_.+-]+): (?:command )?not found/g;
const INSTEAD=[
 // rootfs-v2 (COLLAB 11-05 01:0x): file and xxd are in the image now; hexdump and hd (bsdextrautils) are still not
 [/^(?:hexdump|hd)$/,'網も無いので入れられません。中身の頭は xxd -l 64 <ファイル>（または od -An -tx1 -N64 <ファイル>）で見ます。exe・dll の種類は file <ファイル>、読み込む DLL は x86_64-w64-mingw32-objdump -p <ファイル> | grep "DLL Name" で見ます。'],
 [/^(?:sudo|su|doas)$/,'コマンドは管理者として動くので、sudo は要りません。パッケージは網が無いので入れられません。'],
 [/^(?:apt|apt-get|aptitude|dpkg|dnf|yum|apk|snap|brew|pip|pip3|conda|npm|yarn|pnpm|cargo|go|rustc|java|javac|ruby|php|perl6)$/,'網が無いので、入れることも取ってくることもできません。今ある道具（gcc/g++、clang、cmake、python3、node）で進めてください。'],
 [/^(?:python|py)$/,'python3 を使います。'],
 [/^jq$/,'網も無いので入れられません。JSON は python3 -c "import json,sys; …" か node -e で読みます。'],
 [/^(?:open|xdg-open|start|explorer|explorer\.exe|cmd|cmd\.exe|powershell|pwsh|wine|wine64)$/,'このイメージには画面も Windows もありません。PC で動かすのが要るなら、PC に写してから LocalShell.Run で動かします。'],
];
// The first command not found in a result's stderr (any exit code: in a pipeline the exit code is the last command's).
export function missingCommand(output){
 if(typeof output?.exitCode!=='number')return null;
 NOT_FOUND.lastIndex=0;const match=NOT_FOUND.exec(String(output.stderr??''));if(!match)return null;
 const name=match[1],known=INSTEAD.find(([pattern])=>pattern.test(name));
 return known?{name,inImage:false,instead:name+' はこのイメージにありません。'+known[1]}
  :{name,found:false,instead:name+' が見つかりませんでした。作ったプログラムなら ./'+name+' と書きます（今のフォルダーは PATH に入っていません）。道具の名前なら、このイメージには無く、網も無いので入れられません。'};
}
// A copy of a shell line for scanning, the same length (offsets stay valid): backslash-newline continuations become spaces and heredoc
// bodies are blanked, so neither splits a command nor counts as one.
function scanText(command){
 let text=String(command??'').replace(/\\\n/g,'  ');
 const heredoc=/<<-?\s*(['"]?)(\w+)\1[^\n]*\n/g;let match;
 while((match=heredoc.exec(text))){
  const start=match.index+match[0].length,close=new RegExp('^\\s*'+match[2]+'\\s*$','m').exec(text.slice(start));
  const stop=close?start+close.index+close[0].length:text.length;
  text=text.slice(0,start)+' '.repeat(stop-start)+text.slice(stop);heredoc.lastIndex=stop;
 }
 return text;
}
const segments=text=>{const out=[];let start=0;for(const sep of text.matchAll(/&&|\|\||[;\n|]/g)){out.push([start,sep.index]);start=sep.index+sep[0].length;}out.push([start,text.length]);return out;};
// the driver must be the command word (after VAR=value, env, time, timeout N, nice -n N, exec, command), not an argument of echo or grep
const MINGW=/^\s*(?:\(\s*)?(?:(?:[A-Za-z_][A-Za-z0-9_]*=\S*|env|time|exec|command|timeout\s+\S+|nice(?:\s+-n\s*-?\d+)?)\s+)*((?:\S*\/)?x86_64-w64-mingw32-(?:g\+\+|gcc|c\+\+)(?:-posix|-win32)?(?:-\d+(?:\.\d+)*)?)(?=\s|$)/;
const NO_LINK=word=>['-c','-S','-E','-M','-MM','-fsyntax-only','-###','--version','-v','-dumpversion','-dumpmachine','--help'].includes(word)||word.startsWith('-print-');
// Each mingw driver call of a line that links: {compiler, end (offset after the compiler word in the original line), words}
function mingwLinks(command){
 const text=scanText(command),links=[];
 for(const [start,stop] of segments(text)){
  const part=text.slice(start,stop),match=MINGW.exec(part);if(!match)continue;
  const words=part.slice(match.index+match[0].length).trim().split(/\s+/).filter(Boolean);
  if(words.some(NO_LINK))continue;
  links.push({compiler:match[1],end:start+match.index+match[0].length,words});
 }
 return links;
}
// The .exe a mingw link writes (its -o, or a.exe), relative to the working folder; null when the line has no mingw link.
export function mingwOutput(command){
 const link=mingwLinks(command)[0];if(!link)return null;
 const at=link.words.findIndex(word=>word==='-o'||(word.startsWith('-o')&&word.length>2));
 const out=at<0?'a.exe':link.words[at]==='-o'?link.words[at+1]:link.words[at].slice(2);
 return out?out.replace(/^['"]|['"]$/g,''):null;
}
// The first link of the line without -static, refused before it runs; fixedCommand is the whole line with -static added to that call.
export function mingwStaticViolation(command){
 const link=mingwLinks(command).find(item=>!item.words.includes('-static'));if(!link)return null;
 const original=String(command);
 return {code:'MINGW_STATIC_REQUIRED',executed:false,compiler:link.compiler,
  error:'x86_64-w64-mingw32 でリンクする時は -static を付けます（付けないと libstdc++-6.dll・libgcc_s_seh-1.dll が要り、MinGW の無い PC では起動しません。-static-libgcc -static-libstdc++ では libwinpthread-1.dll が残ります）。',
  fixedCommand:original.slice(0,link.end)+' -static'+original.slice(link.end)};
}
