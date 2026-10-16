import './test-i18n.js';
import assert from 'node:assert/strict';
import test from 'node:test';
import { createProjectChoice, folderLabel, siteOf, PRIVATE } from './project-picker.js';

class Node {
  constructor(tag = 'div') { this.tag = tag; this.children = []; this.listeners = {}; this.textContent = ''; this.hidden = false; this.value = ''; this.title = ''; this.disabled = false; this.dataset = {}; }
  append(...nodes) { for (const node of nodes) { node.parent = this; this.children.push(node); } }
  replaceChildren(...nodes) { this.children = []; this.append(...nodes); }
  addEventListener(type, listener) { this.listeners[type] = listener; }
  setAttribute() {}
  fire(type) { this.listeners[type]?.({ preventDefault() {} }); }
}
const document = { createElement: tag => new Node(tag) };
const text = node => [node.textContent, ...node.children.map(text)].filter(Boolean).join(' ');
const all = (node, predicate) => [...(predicate(node) ? [node] : []), ...node.children.flatMap(child => all(child, predicate))];
const settle = async () => { for (let i = 0; i < 4; i++) await new Promise(resolve => setImmediate(resolve)); };
const page = { url: 'http://localhost:3000/app/settings' };

function setup(answers = {}) {
  const calls = [], select = new Node('select'), host = new Node();
  const api = async (operation, params) => {
    calls.push([operation, params]);
    const answer = answers[operation];
    return typeof answer === 'function' ? answer(params) : answer ?? {};
  };
  const choice = createProjectChoice({ document, api, select, host, supported: () => true, onError: error => { throw error; } });
  return { choice, calls, select, host };
}
const list = { projects: [{ path: '/work/app', sources: ['claude'] }, { path: '/work/site', sources: ['site'] }] };
const bound = { binding: { origin: 'http://localhost:3000', pathScope: '/', root: '/work/site', exists: true, fingerprint: 'fp1' } };

test('labels and sites', () => {
  assert.equal(folderLabel('/Users/me/work/app'), 'app · me/work');
  assert.equal(folderLabel('C:\\work\\app'), 'app · C:/work');
  assert.deepEqual(siteOf('http://localhost:3000/a/b?q=1'), { origin: 'http://localhost:3000', pagePath: '/a/b' });
  assert.equal(siteOf('chrome://settings'), null);
});

test('a site\'s remembered project is preselected until the user picks another', async () => {
  const f = setup({ conversationProjectList: list, inspectProject: bound });
  await f.choice.refresh(page);
  assert.deepEqual(f.calls.find(([op]) => op === 'inspectProject')[1], { origin: 'http://localhost:3000', pagePath: '/app/settings' });
  assert.equal(f.choice.value(), '/work/site');
  assert.equal(f.select.children[0].value, PRIVATE, 'the private folder stays first');
  assert.match(text(f.select), /Choose a folder…/);
  assert.equal(f.select.dataset.chosen, '1', 'a project in use is shown as set');
  f.select.value = '/work/app'; f.select.fire('change');
  await f.choice.refresh(page);
  assert.equal(f.choice.value(), '/work/app', 'a refresh keeps the user\'s choice');
  // Pi cannot work in a project folder.
  f.choice.setAgent('pi');
  assert.equal(f.choice.value(), PRIVATE);
  assert.equal(f.select.disabled, true);
  assert.equal(f.select.dataset.chosen, '', 'Pi shows no project as set');
});

test('Other… opens the folder picker; a cancel keeps the previous choice; no picker asks for a path', async () => {
  let answer = { path: '/work/new' };
  const f = setup({ conversationProjectList: { projects: [] }, conversationProjectChoose: () => answer });
  await f.choice.refresh(null);
  assert.equal(f.choice.value(), PRIVATE);
  const other = f.select.children.at(-1).value;
  f.select.value = other; f.select.fire('change'); await settle();
  assert.equal(f.choice.value(), '/work/new');
  assert.ok(f.calls.some(([op, p]) => op === 'conversationProjectChoose' && typeof p.prompt === 'string'));
  answer = { cancelled: true };
  f.select.fire('focus'); f.select.value = other; f.select.fire('change'); await settle();
  assert.equal(f.choice.value(), '/work/new');
  answer = { unavailable: true };
  f.select.value = other; f.select.fire('change'); await settle();
  const input = all(f.host, n => n.tag === 'input')[0];
  assert.ok(input, 'a typed path is asked for');
  input.value = '/typed/folder';
  all(f.host, n => n.tag === 'button' && n.textContent === 'Use')[0].fire('click'); await settle();
  assert.equal(f.choice.value(), '/typed/folder');
});

test('after creating a conversation in another project, the site may remember it', async () => {
  const binds = [];
  const f = setup({ conversationProjectList: list, inspectProject: bound, inspectProjectBind: params => { binds.push(params); return { binding: { origin: params.origin, root: params.root, pathScope: '/', fingerprint: 'fp2' } }; } });
  await f.choice.refresh(page);
  f.choice.created('/work/site', page);
  assert.equal(all(f.host, n => n.className === 'project-remember')[0].hidden, true, 'the remembered project is not asked about');
  f.choice.created('/work/app', page);
  const box = all(f.host, n => n.className === 'project-remember')[0];
  assert.equal(box.hidden, false);
  assert.match(text(box), /Use this project for http:\/\/localhost:3000 from now on\?/);
  all(box, n => n.tag === 'button' && n.textContent === 'Remember')[0].fire('click'); await settle();
  assert.deepEqual(binds, [{ origin: 'http://localhost:3000', pathScope: '/', root: '/work/app', replaces: 'fp1' }]);
  assert.match(text(box), /now uses this project/);
  // Without a binding, nothing is replaced; a non-web page is never asked about.
  const g = setup({ conversationProjectList: list, inspectProject: { binding: null }, inspectProjectBind: params => { binds.push(params); return { binding: { origin: params.origin, root: params.root } }; } });
  await g.choice.refresh(page);
  g.choice.created('/work/app', page);
  all(g.host, n => n.tag === 'button' && n.textContent === 'Remember')[0].fire('click'); await settle();
  assert.equal(binds.at(-1).replaces, null);
  g.choice.created('/work/app', { url: 'chrome://newtab' });
  assert.equal(all(g.host, n => n.className === 'project-remember')[0].hidden, true);
});
