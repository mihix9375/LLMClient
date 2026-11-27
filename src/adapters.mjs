const validName=name=>typeof name==='string'&&/^[a-z][a-z0-9-]{0,47}$/.test(name);
export function validateAdapters(profile={}){
  const entries=profile.adapters??{};
  if(!entries||typeof entries!=='object'||Array.isArray(entries)||Object.keys(entries).length>64)throw Error('Invalid adapter profiles');
  for(const [name,row] of Object.entries(entries)){
    if(row?.toolProtocol!==undefined&&!['json','native'].includes(row.toolProtocol))throw Error('Invalid adapter tool protocol: '+name);
    if(!validName(name)||name==='none'||!row||typeof row!=='object'||!profile.baseModel||row.baseModel!==profile.baseModel||typeof row.path!=='string'||!row.path.startsWith('/')||!row.path.endsWith('.gguf')||!Number.isFinite(row.scale)||row.scale<0||row.scale>2)throw Error('Invalid or incompatible LoRA profile: '+name);
  }
  if(profile.roleAdapters!==undefined){
    if(!profile.roleAdapters||typeof profile.roleAdapters!=='object'||Array.isArray(profile.roleAdapters)||Object.keys(profile.roleAdapters).length>64)throw Error('Invalid role adapter policy');
    for(const [role,names] of Object.entries(profile.roleAdapters))if(!validName(role)||!Array.isArray(names)||!names.length||names.length>8||new Set(names).size!==names.length||names.some(name=>!validName(name)))throw Error('Invalid role adapter policy: '+role);
  }
  if(profile.phaseAdapters!==undefined){
    if(!profile.phaseAdapters||typeof profile.phaseAdapters!=='object'||Array.isArray(profile.phaseAdapters))throw Error('Invalid phase adapter policy');
    for(const [phase,names] of Object.entries(profile.phaseAdapters))if(!['investigate','implement','verify','review'].includes(phase)||!Array.isArray(names)||!names.length||names.length>8||new Set(names).size!==names.length||names.some(name=>!validName(name)))throw Error('Invalid phase adapter policy: '+phase);
  }
  checkAdapter(profile,profile.defaultAdapter??'none');
}
export function checkAdapter(profile,name){
  if(!validName(name)||name!=='none'&&!Object.hasOwn(profile?.adapters??{},name))throw Error('Unknown worker adapter: '+String(name));
  const row=profile?.adapters?.[name];
  if(row&&(!profile.baseModel||row.baseModel!==profile.baseModel))throw Error('Incompatible worker adapter: '+name);
  return name;
}
export async function resolveAdapter(profile,name,model){
  checkAdapter(profile,name);
  if(name==='none')return [];
  const row=profile.adapters[name],loaded=await model.loraAdapters();
  if(!Array.isArray(loaded))throw Error('Invalid loaded LoRA response');
  const matches=loaded.filter(item=>item.path===row.path);
  if(matches.length!==1||!Number.isSafeInteger(matches[0].id)||matches[0].id<0)throw Error('Worker adapter is not loaded uniquely: '+name);
  return [{id:matches[0].id,scale:row.scale}];
}
export function adapterProfiles(profile={},loaded=[]){
  validateAdapters(profile);
  return {baseModel:profile.baseModel??null,defaultAdapter:profile.defaultAdapter??'none',roleAdapters:profile.roleAdapters??{},phaseAdapters:profile.phaseAdapters??{},adapters:[{name:'none',description:'ベースモデルのみ',toolProtocol:'json',loaded:true},...Object.entries(profile.adapters??{}).map(([name,row])=>({name,description:row.description??'',scale:row.scale,toolProtocol:row.toolProtocol??'json',loaded:loaded.filter(item=>item.path===row.path).length===1}))]};
}
