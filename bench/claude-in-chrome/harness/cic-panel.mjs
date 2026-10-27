// node harness/cic-panel.mjs <cdp-port> <outfile> "<prompt>"  -- type a prompt into the Claude in Chrome side panel over CDP and wait until it finishes.
// The rig Chrome must be started with --remote-debugging-port=<cdp-port> and the CiC side panel open. Times are +-3 s (2 s polling).
import fs from 'node:fs';
const [port, out, prompt] = process.argv.slice(2);
async function target(){ const l = await (await fetch(`http://127.0.0.1:${port}/json`)).json(); return l.find(x=>x.url.includes('claude.ai/cic')); }
let t = await target();
const ws = new WebSocket(t.webSocketDebuggerUrl);
let id=0; const pend={};
const send=(m,p={})=>new Promise(r=>{const i=++id;pend[i]=r;ws.send(JSON.stringify({id:i,method:m,params:p}))});
ws.onmessage=e=>{const m=JSON.parse(e.data); if(m.id&&pend[m.id])pend[m.id](m)};
await new Promise(r=>ws.onopen=r);
const ev = async (x)=> (await send('Runtime.evaluate',{expression:x,returnByValue:true,awaitPromise:true})).result?.result?.value;
const xy = await ev(`(()=>{const e=document.querySelector('.ProseMirror');const b=e.getBoundingClientRect();return [b.x+20,b.y+b.height/2]})()`);
for (const type of ['mouseMoved','mousePressed','mouseReleased']) await send('Input.dispatchMouseEvent',{type,x:xy[0],y:xy[1],button:'left',clickCount:1});
await send('Input.insertText',{text:prompt});
const t0 = Date.now();
await send('Input.dispatchKeyEvent',{type:'keyDown',key:'Enter',code:'Enter',windowsVirtualKeyCode:13,text:'\r'});
await send('Input.dispatchKeyEvent',{type:'keyUp',key:'Enter',code:'Enter',windowsVirtualKeyCode:13});
let seenStop=false, quiet=0, prompts=[];
while (Date.now()-t0 < 900000) {
  await new Promise(r=>setTimeout(r,2000));
  const s = await ev(`(()=>{const bs=[...document.querySelectorAll('button')].map(b=>(b.getAttribute('aria-label')||b.innerText||'').trim());return {stop:bs.some(x=>/^stop/i.test(x)), allow:bs.filter(x=>/^(allow|approve|always allow|continue)/i.test(x))}})()`);
  if (s?.allow?.length) prompts.push({t:(Date.now()-t0)/1000, b:s.allow});
  if (s?.stop) { seenStop=true; quiet=0; } else if (seenStop || Date.now()-t0>20000) { quiet++; if (quiet>=3) break; }
}
const secs = (Date.now()-t0)/1000 - 6;
const text = await ev(`document.body.innerText`);
fs.writeFileSync(out, JSON.stringify({prompt, secs, prompts, text}, null, 1));
console.log('done secs≈', secs.toFixed(1), 'prompts', JSON.stringify(prompts).slice(0,300));
ws.close();
