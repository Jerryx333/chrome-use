// Research probe only. Run on an isolated GUI test host, never the maintainer browser.
import {createServer} from 'node:http';
import {spawn,spawnSync} from 'node:child_process';
import {mkdtemp,readFile,writeFile,mkdir} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
const out=process.argv[2];
if (!out || process.env.CHROME_USE_ISOLATED_GUI_HOST !== '1') {
  console.error('Usage on an isolated GUI host: CHROME_USE_ISOLATED_GUI_HOST=1 node docs/evidence/position-indicator-20261026/probe.mjs <output-directory>');
  process.exit(2);
}
await mkdir(out,{recursive:true});
const profile=await mkdtemp(join(tmpdir(),'cu-position-profile-'));
const html=`<!doctype html><meta charset="utf-8"><title>chrome-use isolated position probe</title><link rel="icon" href="data:,"><style>html,body{margin:0;background:rgb(18,52,86);color:white}button{position:absolute;left:80px;top:80px;width:160px;height:80px;background:rgb(11,222,139);border:0}input{position:absolute;left:80px;top:200px}</style><button id="target">Fixture button</button><input aria-label="Fixture field"><script>window.fixture={mutations:0,events:[],requests:0};new MutationObserver(records=>fixture.mutations+=records.length).observe(document.documentElement,{subtree:true,attributes:true,childList:true,characterData:true});for(const name of ['pointerdown','pointerup','mousedown','mouseup','click','visibilitychange','focus','blur'])addEventListener(name,e=>fixture.events.push({type:e.type,trusted:e.isTrusted,target:e.target.id||e.target.nodeName}),true);</script>`;
let requests=0;const server=createServer((q,r)=>{requests++;r.setHeader('Content-Type','text/html');r.end(html)});await new Promise(r=>server.listen(0,'127.0.0.1',r));
const chrome=spawn('/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',[`--user-data-dir=${profile}`,'--remote-debugging-port=0','--no-first-run','--no-default-browser-check','--window-size=900,700','--window-position=60,60','--new-window','about:blank'],{stdio:['ignore','ignore','ignore']});
let ws,session;const pending=new Map();const calls=[];let seq=0;const sleep=ms=>new Promise(r=>setTimeout(r,ms));
try{
 let port;for(let i=0;i<100;i++){try{port=Number((await readFile(join(profile,'DevToolsActivePort'),'utf8')).split('\n')[0]);break}catch{}await sleep(100)}if(!port)throw Error('Chrome debugging endpoint unavailable');
 const info=await(await fetch(`http://127.0.0.1:${port}/json/version`)).json();ws=new WebSocket(info.webSocketDebuggerUrl);await new Promise((r,j)=>{ws.onopen=r;ws.onerror=j});
 ws.onmessage=e=>{const m=JSON.parse(e.data);if(m.id){const p=pending.get(m.id);if(p){pending.delete(m.id);m.error?p.reject(Error(JSON.stringify(m.error))):p.resolve(m.result)}}};
 const call=(method,params={},sid=session)=>new Promise((resolve,reject)=>{const id=++seq;calls.push({method,params,sessionId:sid});pending.set(id,{resolve,reject});ws.send(JSON.stringify({id,method,params,...(sid?{sessionId:sid}:{})}));setTimeout(()=>{if(pending.delete(id))reject(Error('Timed out: '+method))},15000).unref()});
 const {targetInfos}=await call('Target.getTargets',{},null);const target=targetInfos.find(t=>t.type==='page');session=(await call('Target.attachToTarget',{targetId:target.targetId,flatten:true},null)).sessionId;
 await call('Page.enable');await call('Runtime.enable');await call('Page.navigate',{url:`http://127.0.0.1:${server.address().port}/`});await call('Page.bringToFront');await sleep(1500);
 const evaluate=async expression=>(await call('Runtime.evaluate',{expression,returnByValue:true})).result.value;
 const state=()=>evaluate(`({globals:Object.getOwnPropertyNames(window).sort(),symbols:Object.getOwnPropertySymbols(window).map(String).sort(),nodes:document.querySelectorAll('*').length,html:document.documentElement.outerHTML,fixture:structuredClone(fixture),focus:document.activeElement.id||document.activeElement.nodeName,visibility:document.visibilityState,viewport:[innerWidth,innerHeight,devicePixelRatio,visualViewport.scale],screen:[screenX,screenY,outerWidth,outerHeight]})`);
 const desktop=async label=>{const g=await evaluate('({x:screenX+(outerWidth-innerWidth)/2,y:screenY+outerHeight-innerHeight,w:innerWidth,h:innerHeight})');const file=join(out,label+'-desktop.png');const r=spawnSync('/usr/sbin/screencapture',['-x',`-R${Math.round(g.x)},${Math.round(g.y)},${g.w},${g.h}`,file],{encoding:'utf8'});return {file,geometry:g,exit:r.status,error:r.stderr}};
 const capture=async label=>{const s=await call('Page.captureScreenshot',{format:'png'});await writeFile(join(out,label+'-page.png'),Buffer.from(s.data,'base64'));return desktop(label)};
 const baseline=await state();const baseImage=await capture('baseline');const requestsBefore=requests;
 let hingeReply,hingeError;try{hingeReply=await call('Overlay.setShowHinge',{hingeConfig:{rect:{x:100,y:100,width:24,height:24},contentColor:{r:255,g:0,b:255,a:1},outlineColor:{r:255,g:0,b:255,a:1}}})}catch(e){hingeError=String(e)}
 await sleep(250);const visible=await state();const visibleImage=await capture('visible');
 for(const type of ['mousePressed','mouseReleased'])await call('Input.dispatchMouseEvent',{type,x:110,y:110,button:'left',buttons:type==='mousePressed'?1:0,clickCount:1});
 const requestsAfterMarker=requests;
 const afterInput=await state();await call('Overlay.setShowHinge',{});await sleep(250);const hidden=await state();const hiddenImage=await capture('hidden');
 await call('Overlay.setShowHinge',{hingeConfig:{rect:{x:100,y:100,width:24,height:24},contentColor:{r:255,g:0,b:255,a:1}}});
 await call('Page.navigate',{url:`http://127.0.0.1:${server.address().port}/?navigation-control=1`});await sleep(350);
 const navigationImage=await capture('navigation');
 await call('Overlay.setShowHinge',{});await sleep(150);
 const navigationHiddenImage=await capture('navigation-hidden');
 const noInjection=JSON.stringify(baseline.globals)===JSON.stringify(visible.globals)&&JSON.stringify(baseline.symbols)===JSON.stringify(visible.symbols)&&baseline.nodes===visible.nodes&&baseline.html===visible.html&&visible.fixture.mutations===baseline.fixture.mutations;
 const evidence={browser:info,profile,baseline,visible,afterInput,hidden,hingeReply,hingeError,images:{baseline:baseImage,visible:visibleImage,hidden:hiddenImage,navigation:navigationImage,navigationHidden:navigationHiddenImage},checks:{noInjection,noExtraFixtureRequests:requestsAfterMarker===requestsBefore,viewportUnchanged:JSON.stringify(baseline.viewport)===JSON.stringify(visible.viewport),focusUnchanged:baseline.focus===visible.focus,visibilityUnchanged:baseline.visibility===visible.visibility,eventsBeforeInputUnchanged:JSON.stringify(baseline.fixture.events)===JSON.stringify(visible.fixture.events),trustedClickDelivered:afterInput.fixture.events.some(e=>e.type==='click'&&e.trusted&&e.target==='target')},calls};
 await writeFile(join(out,'evidence.json'),JSON.stringify(evidence,null,2));
 if (!Object.values(evidence.checks).every(Boolean) || Object.values(evidence.images).some(image => image.exit !== 0)) process.exitCode = 2;
 console.log(JSON.stringify({out,checks:evidence.checks,hingeReply,hingeError,images:evidence.images}));await call('Browser.close',{},null).catch(()=>{});
}finally{ws?.close();chrome.kill();server.close()}
