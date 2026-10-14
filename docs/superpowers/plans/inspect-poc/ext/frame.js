chrome.runtime.sendMessage({type:'frame-boot'});
let authed=false;
addEventListener('message',e=>{if(authed||e.source!==parent||!e.ports[0])return;const tag=e.data?.tag||'?';
 const mp=e.ports[0];window.__cand=mp;mp.onmessage=m=>{if(authed)return;const p=chrome.runtime.connect({name:'f'});
  p.onMessage.addListener(r=>{if(r.authorized){authed=true;window.__mp=mp;chrome.runtime.sendMessage({type:'frame-status',status:'authorized',tag});}
   if(r.denied)chrome.runtime.sendMessage({type:'frame-status',status:'denied',tag});
   if(r.revoked)chrome.runtime.sendMessage({type:'frame-status',status:'revoked',tag});});
  p.onDisconnect.addListener(()=>{if(authed)chrome.runtime.sendMessage({type:'frame-status',status:'disconnected',tag});});
  p.postMessage({auth:m.data?.secret,tag});};});

window.__vis=()=>new Promise(res=>{const n=Math.random();const h=e=>{if(e.data?.nonce===n){window.__mp.removeEventListener('message',h);res(e.data);}};window.__mp.addEventListener('message',h);window.__mp.start();window.__mp.postMessage({type:'vis',nonce:n});});
