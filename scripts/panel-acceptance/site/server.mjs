// Fixture site for the verifiable-panel-turns live checks (plan 14.2). One
// Node server, no dependencies. Every request is logged so a check can prove a
// tool never reached the site (check 20). Usage: node server.mjs <port> <log>
import http from 'node:http';
import fs from 'node:fs';
import { createHash } from 'node:crypto';

const port = Number(process.argv[2] || 8910);
const logPath = process.argv[3] || '/tmp/panel-site.log';
const sleep = ms => new Promise(r => setTimeout(r, ms));
const log = entry => fs.appendFileSync(logPath, JSON.stringify({ at: Date.now(), ...entry }) + '\n');

// Deterministic file bodies; the driver recomputes digests from the same rule.
export function pdfBody(variant) {
  const text = `%PDF-1.4\n% panel acceptance report variant ${variant}\n1 0 obj << /Type /Catalog >> endobj\n` + 'x'.repeat(2000 + Number(variant || 0) * 37) + '\n%%EOF\n';
  return Buffer.from(text);
}
export const sha256 = bytes => 'sha256:' + createHash('sha256').update(bytes).digest('hex');

const secrets = new Map();
const page = (title, body, head = '') => `<!doctype html><html><head><meta charset="utf-8"><title>${title}</title>${head}<style>body{font:15px system-ui;margin:24px}#toast{position:fixed;bottom:20px;right:20px;background:#222;color:#fff;padding:8px 12px}</style></head><body>${body}</body></html>`;

const routes = {
  '/': () => page('Panel fixture home', '<h1>Panel fixture</h1><a href="/a">A</a> <a href="/b">B</a> <a href="/c">C</a>'),
  '/a': () => page('Page A', '<h1 id="title">Page A</h1><p>Sent from here.</p><a id="to-b" href="/b">Go to B</a>'),
  '/b': q => page('Page B', `<h1 id="title">Page B</h1><p id="where">${q.get('landed') ? 'landed by redirect' : q.get('popup') ? 'opened as popup' : 'direct'}</p><button id="act" onclick="document.getElementById('done').textContent='Acted on B'">Act</button><p id="done"></p><a id="redirect" href="/redirect">Redirecting link</a> <a id="popup" href="/b?popup=1" target="_blank">Popup link</a>`),
  '/c': () => page('Page C', '<h1 id="title">Page C</h1><p>The user switched here.</p>'),
  // A form whose save the server rejects; the page reports what the server said.
  '/form': () => page('Order form', `<h1>Order form</h1><label>Name <input id="name" name="name"></label>
<button id="save" onclick="save()">Save</button><button id="save-ok" onclick="saveOk()">Save draft</button><p id="status"></p>
<script>
async function save(){const r=await fetch('/api/save',{method:'POST',body:document.getElementById('name').value});document.getElementById('status').textContent=r.ok?'Saved':'Save failed: '+(await r.text());}
async function saveOk(){const r=await fetch('/api/draft',{method:'POST',body:document.getElementById('name').value});document.getElementById('status').textContent=r.ok?'Draft saved':'Draft failed';}
</script>`),
  '/list': q => {
    const n = Math.max(1, Math.min(6, Number(q.get('page') || 1)));
    const items = Array.from({ length: 5 }, (_, i) => `<li>Item ${(n - 1) * 5 + i + 1}</li>`).join('');
    const next = n < 6 ? `<a id="next" href="/list?page=${n + 1}">next</a>` : '<span id="end">end</span>';
    return page(`List page ${n}`, `<h1>List page ${n}</h1><ul>${items}</ul>${next}`);
  },
  '/static': () => page('Unchanged page', '<h1>Unchanged</h1><p>This page never changes.</p>'),
  '/login': () => page('Sign in', `<h1>Sign in</h1><form id="f" method="post" action="/api/login"><label>User <input id="user" name="user" value="alice"></label>
<label>Password <input id="password" type="password" name="password"></label><button id="submit" type="submit">Sign in</button></form>`),
  // Secrets come from the driver (POST /__secrets), never from this page's
  // own URL, so only the parameters the plan names carry them.
  '/cookie': q => page('Cookie page', `<h1>Cookie page</h1><a id="tok" href="/b?access_token=${encodeURIComponent(secrets.get(q.get('id'))?.tok || 'none')}&amp;x=1">Continue</a>`),
  '/toast': () => page('Toast', `<h1>Toast</h1><button id="go" onclick="t()">Go</button><script>function t(){const d=document.createElement('div');d.id='toast';d.textContent='Order placed';document.body.append(d);setTimeout(()=>d.remove(),1500);}</script>`),
  // Records every DOM change and reports it to the server; a stealthy agent
  // that only reads and clicks leaves attribute/child counts it can explain.
  '/stealth': () => page('Stealth probe', `<h1 id="title">Stealth probe</h1><button id="act" onclick="this.dataset.clicked='1'">Act</button>
<script>
const seen=[];new MutationObserver(list=>{for(const m of list){if(m.type==='attributes'&&m.target.id==='act'&&m.attributeName==='data-clicked')continue;seen.push({type:m.type,target:m.target.nodeName+(m.target.id?'#'+m.target.id:''),attr:m.attributeName,added:[...m.addedNodes].map(n=>n.nodeName),removed:[...m.removedNodes].map(n=>n.nodeName)});}}).observe(document,{subtree:true,childList:true,attributes:true,characterData:true});
setInterval(()=>{const rootAttrs=[...document.documentElement.attributes].map(a=>a.name);navigator.sendBeacon('/log/stealth',JSON.stringify({mutations:seen,rootAttrs,nodes:document.getElementsByTagName('*').length}));},1000);
</script>`),
  // 1500 requests in bursts of 25, for the request-log bound (check 16).
  '/burst': () => page('Burst', `<h1>Burst</h1><button id="act" onclick="this.dataset.clicked='1'">Act</button><button id="go" onclick="go()">Go</button><p id="sent">0</p>
<script>async function go(){let n=0;for(let i=0;i<60;i++){await Promise.all(Array.from({length:25},()=>fetch('/ping?n='+(n++)).catch(()=>{})));document.getElementById('sent').textContent=n;}}</script>`),
  // Inspect & Ask → panel (inspect-panel-live): a plain button, one in an
  // open shadow root, one in a same-origin iframe.
  '/inspect': () => page('Inspect fixture', '<h1 id="title">Inspect fixture</h1><p><button id="save" style="background:#bbb;color:#555">Save order</button></p><x-card id="card"></x-card><p><iframe id="pay" src="/inspect-pay" style="width:360px;height:110px;border:1px solid #ccc"></iframe></p><a id="away" href="/a">Away</a>',
    '<script>customElements.define("x-card", class extends HTMLElement { constructor() { super(); this.attachShadow({ mode: "open" }).innerHTML = \'<div class="body"><button class="inner">Inner action</button></div>\'; } });</script>'),
  '/inspect-pay': () => page('Pay frame', '<button id="pay-now">Pay now</button>'),
  '/popup-page': () => page('Popup opener', '<a id="popup" href="/b?popup=1" target="_blank">Open popup</a>'),
};

const server = http.createServer(async (req, res) => {
  const url = new URL(req.url, 'http://x');
  const q = url.searchParams;
  let body = '';
  req.on('data', c => { body += c; });
  await new Promise(r => req.on('end', r));
  log({ method: req.method, path: url.pathname, query: url.search, auth: req.headers.authorization || null, body: body.slice(0, 500) });
  if (url.pathname === '/redirect') { res.writeHead(302, { location: '/b?landed=1' }); return res.end(); }
  if (url.pathname === '/slow') { await sleep(Number(q.get('ms') || 6000)); res.writeHead(200, { 'content-type': 'text/html' }); return res.end(page('Slow page', '<h1>Slow page loaded</h1>')); }
  if (url.pathname === '/api/save') { res.writeHead(422); return res.end('rejected by server'); }
  if (url.pathname === '/api/draft') { res.writeHead(200); return res.end('ok'); }
  if (url.pathname === '/api/login') { res.writeHead(200, { 'content-type': 'text/html' }); return res.end(page('Signed in', '<h1 id="title">Signed in</h1>')); }
  if (url.pathname === '/__secrets' && req.method === 'POST') { const v = JSON.parse(body); secrets.set(v.id, v); res.writeHead(204); return res.end(); }
  if (url.pathname === '/cookie' && secrets.has(q.get('id'))) res.setHeader('set-cookie', [`sid=${secrets.get(q.get('id')).cookie}; Path=/`, 'pref=compact; Path=/']);
  if (url.pathname === '/log/stealth' || url.pathname === '/ping') { res.writeHead(204); return res.end(); }
  if (url.pathname.startsWith('/download/')) {
    const name = url.pathname.slice('/download/'.length);
    const disposition = { 'content-disposition': `attachment; filename="${q.get('as') || name.replace(/^(slow|abort)-/, '')}"` };
    if (name === 'report.pdf') { const b = pdfBody(q.get('v') || '1'); res.writeHead(200, { 'content-type': 'application/pdf', 'content-length': b.length, ...disposition }); return res.end(b); }
    if (name === 'empty.pdf') { res.writeHead(200, { 'content-type': 'application/pdf', 'content-length': 0, ...disposition }); return res.end(); }
    if (name === 'fake.pdf') { const b = Buffer.from(page('Error', '<h1>Not a PDF</h1>')); res.writeHead(200, { 'content-type': 'application/pdf', 'content-length': b.length, ...disposition }); return res.end(b); }
    if (name === 'slow-report.pdf') {
      const b = pdfBody(q.get('v') || '7'); const ms = Number(q.get('ms') || 8000);
      res.writeHead(200, { 'content-type': 'application/pdf', 'content-length': b.length, ...disposition });
      const parts = 8;
      for (let i = 0; i < parts; i++) { res.write(b.subarray(Math.floor(i * b.length / parts), Math.floor((i + 1) * b.length / parts))); await sleep(ms / parts); }
      return res.end();
    }
    if (name === 'abort-report.pdf') {
      const b = pdfBody('9');
      res.writeHead(200, { 'content-type': 'application/pdf', 'content-length': b.length * 4, ...disposition });
      res.write(b);
      await sleep(1500);
      return req.socket.destroy();
    }
  }
  const route = routes[url.pathname];
  if (!route) { res.writeHead(404, { 'content-type': 'text/plain' }); return res.end('not found'); }
  res.writeHead(200, { 'content-type': 'text/html; charset=utf-8' });
  res.end(route(q));
});
if (process.argv[1] && process.argv[1].endsWith('server.mjs')) server.listen(port, '127.0.0.1', () => console.log(`panel fixture site on ${port}`));
