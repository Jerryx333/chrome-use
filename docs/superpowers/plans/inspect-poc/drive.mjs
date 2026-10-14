import {spawn} from 'node:child_process';import http from 'node:http';import fs from 'node:fs';import os from 'node:os';import path from 'node:path';
const dir=path.dirname(new URL(import.meta.url).pathname);
const CHROME=process.env.CHROME;const PORT=9333;const FIX='http://127.0.0.1:8765/fix.html';
http.createServer((q,r)=>{r.setHeader('content-type','text/html');r.end(fs.readFileSync(path.join(dir,'fix.html')));}).listen(8765);
const ud=fs.mkdtempSync(path.join(os.tmpdir(),'poc-'));
const ch=spawn(CHROME,['--headless=new',`--remote-debugging-port=${PORT}`,`--user-data-dir=${ud}`,`--load-extension=${dir}/ext`,`--disable-extensions-except=${dir}/ext`,'--no-first-run','about:blank'],{stdio:'ignore'});
const sleep=ms=>new Promise(r=>setTimeout(r,ms));
let ver;for(let i=0;i<50;i++){try{ver=await (await fetch(`http://127.0.0.1:${PORT}/json/version`)).json();break;}catch{await sleep(200);}}
const ws=new WebSocket(ver.webSocketDebuggerUrl);await new Promise(r=>ws.onopen=r);
let id=0;const pend=new Map();ws.onmessage=e=>{const m=JSON.parse(e.data);if(m.id&&pend.has(m.id)){pend.get(m.id)(m);pend.delete(m.id);}};
const cdp=(method,params={},sessionId)=>new Promise(r=>{const i=++id;pend.set(i,r);ws.send(JSON.stringify({id:i,method,params,sessionId}));});
async function attach(pred){for(let k=0;k<50;k++){const {result}=await cdp('Target.getTargets');const t=result.targetInfos.find(pred);if(t){const a=await cdp('Target.attachToTarget',{targetId:t.targetId,flatten:true});return {sid:a.result.sessionId,t};}await sleep(200);}throw new Error('no target');}
const ev=async(sid,expr)=>{const r=await cdp('Runtime.evaluate',{expression:expr,awaitPromise:true,returnByValue:true},sid);if(r.result?.exceptionDetails)return {error:r.result.exceptionDetails.exception?.description};return r.result?.result?.value;};
const out={};
const tgt=await cdp('Target.createTarget',{url:FIX});await sleep(1000);
let page=await attach(t=>t.type==='page'&&t.url===FIX);
let sw=await attach(t=>t.type==='service_worker'&&t.url.endsWith('/bg.js'));
const frameURL=await ev(sw.sid,'__poc.getURL()');out.frameURL=frameURL;
// S1: attacker copy frame exists BEFORE the real picker, with a fake secret pushed into it
await ev(page.sid,`(()=>{const f=document.createElement('iframe');f.id='copy';f.src=${JSON.stringify(frameURL)};document.body.append(f);
 f.onload=()=>{const c=new MessageChannel();f.contentWindow.postMessage({type:'init',tag:'copy-fake'},'*',[c.port2]);c.port1.postMessage({secret:'guess'});};return 1})()`);
await sleep(800);
// S2: attacker floods every frame with fake inits right as the real picker starts
await ev(page.sid,`(()=>{window.__flood=setInterval(()=>{for(let i=0;i<frames.length;i++){const c=new MessageChannel();try{frames[i].postMessage({type:'init',tag:'flood'},'*',[c.port2]);c.port1.postMessage({secret:'guess2'});}catch(e){}}},20);return 1})()`);
out.start=await ev(sw.sid,`__poc.start(${JSON.stringify(FIX)})`);
await sleep(1500);

// copy created AFTER authorization, given a fake secret
await ev(page.sid,`(()=>{const f=document.createElement('iframe');f.src=${JSON.stringify(frameURL)};document.body.append(f);f.onload=()=>{const c=new MessageChannel();f.contentWindow.postMessage({type:'init',tag:'copy-late'},'*',[c.port2]);c.port1.postMessage({secret:'guess3'});};return 1})()`);
await sleep(800);
out.afterStart=await ev(sw.sid,'JSON.stringify(__poc.log.filter(x=>!(x.tag==="flood"&&x.ev==="frame-status")).map(x=>[x.ev,x.ok,x.frameId,x.tag,x.status].join("|")))');out.floodAuthOk=await ev(sw.sid,'__poc.log.filter(x=>x.tag==="flood"&&x.ok).length');
// Visibility checks against the authorized frame
{const {result:tl}=await cdp('Target.getTargets');let fsid=null;
 for(const t of tl.targetInfos.filter(t=>t.type==='iframe'&&t.url.includes('frame.html'))){const a=await cdp('Target.attachToTarget',{targetId:t.targetId,flatten:true});if(await ev(a.result.sessionId,'!!window.__mp'))fsid=a.result.sessionId;}
 const V=()=>ev(fsid,'window.__vis().then(x=>JSON.stringify(x))');
 out.vis_immediate=await V();await sleep(10000);out.vis_after10s=await V();
 await ev(page.sid,`(()=>{window.__host=[...document.documentElement.children].find(e=>e.tagName==='DIV');const d=document.createElement('div');d.id='ov';d.style.cssText='position:fixed;inset:0;background:rgba(255,255,255,0.01);z-index:2147483647';document.documentElement.append(d);return !!window.__host})()`);
 out.vis_overlay=await V();
 await ev(page.sid,`document.getElementById('ov').remove();window.__host.style.opacity='0.5';1`);out.vis_opacity=await V();
 await ev(page.sid,`window.__host.style.opacity='';1`);out.vis_restored=await V();
 await ev(page.sid,`window.__host.style.transform='translateX(100px)';1`);out.vis_moved=await V();
 await ev(page.sid,`window.__host.style.transform='';1`);}
// S3: reload revokes
await cdp('Page.reload',{},page.sid);await sleep(1500);
out.afterReload=await ev(sw.sid,'JSON.stringify(__poc.log.slice(-4).map(x=>[x.ev,x.ok,x.frameId,x.tag,x.status].join("|")))');
// S4: worker restart drops instance; start new instance then kill worker
page=await attach(t=>t.type==='page'&&t.url===FIX);
await ev(sw.sid,`__poc.log.length=0`);
out.start2=await ev(sw.sid,`__poc.start(${JSON.stringify(FIX)})`);await sleep(1200);
out.beforeKill=await ev(sw.sid,'JSON.stringify(__poc.log)');
await cdp('Target.closeTarget',{targetId:sw.t.targetId});await sleep(1500);
// wake worker by sending a hello-like message is not possible from page; poke via frames: page cannot. Re-attach if worker restarted.
try{const sw2=await attach(t=>t.type==='service_worker'&&t.url.endsWith('/bg.js'));out.afterKill=await ev(sw2.sid,'JSON.stringify(__poc.log)');}catch(e){out.afterKill='worker not running: '+e.message;}
console.log(JSON.stringify(out,null,1));ch.kill();process.exit(0);
