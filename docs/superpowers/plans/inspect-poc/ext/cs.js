(async()=>{const {secret}=await chrome.runtime.sendMessage({type:'hello'});if(!secret)return;
 const host=document.createElement('div');const root=host.attachShadow({mode:'closed'});
 const f=document.createElement('iframe');
 f.addEventListener('load',()=>{chrome.runtime.sendMessage({type:'dbg',src:f.src,cw:!!f.contentWindow});const ch=new MessageChannel();
   ch.port1.onmessage=e=>{if(e.data?.type!=='vis')return;const n=e.data.nonce;let done=false;const io=new IntersectionObserver(es=>{if(done)return;done=true;io.disconnect();const x=es[es.length-1];ch.port1.postMessage({type:'vis',nonce:n,isVisible:x.isVisible,ratio:x.intersectionRatio,docVis:document.visibilityState});},{threshold:[1],trackVisibility:true,delay:100});io.observe(f);setTimeout(()=>{if(!done){done=true;io.disconnect();ch.port1.postMessage({type:'vis',nonce:n,timeout:true});}},1000);};f.contentWindow.postMessage({type:'init',tag:'real'},'chrome-extension://'+chrome.runtime.id,[ch.port2]);ch.port1.postMessage({secret});},{once:true});
 f.style.cssText='position:fixed;right:10px;bottom:10px;width:300px;height:200px;border:0;z-index:2147483647';f.src=chrome.runtime.getURL('frame.html');root.append(f);document.documentElement.append(host);
 window.__pocFrameSrcLeak=undefined;})();
