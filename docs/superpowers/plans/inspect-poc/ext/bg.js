const log=[];const inst=new Map();
function L(x){log.push(x);}
globalThis.__poc={log,
 async start(url){const [tab]=await chrome.tabs.query({url});
  const frames=await chrome.webNavigation.getAllFrames({tabId:tab.id});const top=frames.find(f=>f.frameId===0);
  const i={tabId:tab.id,doc:top.documentId,secret:crypto.randomUUID(),used:false,port:null};inst.set(tab.id,i);
  await chrome.scripting.executeScript({target:{tabId:tab.id,frameIds:[0]},files:['cs.js']});return tab.id;},
 getURL:()=>chrome.runtime.getURL('frame.html')};
chrome.runtime.onMessage.addListener((m,s,reply)=>{
  if(m.type==='hello'){const i=inst.get(s.tab?.id);
    if(i&&s.frameId===0&&s.documentId===i.doc&&!i.handed){i.handed=true;reply({secret:i.secret});L({ev:'secret-handed'});}else{reply({});L({ev:'hello-refused'});}}
  if(m.type==='dbg')L({ev:'dbg',...m});if(m.type==='frame-boot')L({ev:'frame-boot',frameId:s.frameId,url:s.url});if(m.type==='frame-status')L({ev:'frame-status',frameId:s.frameId,status:m.status,tag:m.tag});
});
chrome.runtime.onConnect.addListener(p=>{
  p.onMessage.addListener(m=>{const s=p.sender;const i=inst.get(s.tab?.id);
    const ok=i&&!i.used&&m.auth===i.secret&&s.frameId!==0&&new URL(s.url).pathname==='/frame.html';
    L({ev:'auth',ok:!!ok,frameId:s.frameId,tag:m.tag});
    if(ok){i.used=true;i.port=p;i.frameId=s.frameId;i.frameDoc=s.documentId;p.postMessage({authorized:true});}
    else{p.postMessage({denied:true});p.disconnect();}});
  p.onDisconnect.addListener(()=>{for(const i of inst.values())if(i.port===p){i.port=null;L({ev:'port-gone'});}});
});
chrome.webNavigation.onCommitted.addListener(d=>{if(d.frameId!==0)return;const i=inst.get(d.tabId);
  if(i&&i.doc!==d.documentId){inst.delete(d.tabId);try{i.port?.postMessage({revoked:true});i.port?.disconnect();}catch{}L({ev:'revoked-nav'});}});
