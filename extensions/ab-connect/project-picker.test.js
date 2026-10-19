import './test-i18n.js';
import assert from 'node:assert/strict';
import test from 'node:test';
import { createProjectChoice, folderLabel, siteOf, scopeName, rememberScopes, PRIVATE } from './project-picker.js';

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

function setup(answers = {}, matchesRepositories = false) {
  const calls = [], select = new Node('select'), host = new Node();
  const api = async (operation, params) => {
    calls.push([operation, params]);
    const answer = answers[operation];
    return typeof answer === 'function' ? answer(params) : answer ?? {};
  };
  const choice = createProjectChoice({ document, api, select, host, supported: () => true, matchesRepositories: () => matchesRepositories, onError: error => { throw error; } });
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
  await f.choice.created('/work/site', page);
  assert.equal(all(f.host, n => n.className === 'project-remember')[0].hidden, true, 'the remembered project is not asked about');
  await f.choice.created('/work/app', page);
  const box = all(f.host, n => n.className === 'project-remember')[0];
  assert.equal(box.hidden, false);
  assert.match(text(box), /Use this project for localhost:3000 from now on\?/);
  all(box, n => n.tag === 'button' && n.textContent === 'Remember')[0].fire('click'); await settle();
  assert.deepEqual(binds, [{ origin: 'http://localhost:3000', pathScope: '/', root: '/work/app', replaces: 'fp1' }]);
  assert.match(text(box), /now uses this project/);
  // Without a binding, nothing is replaced; a non-web page is never asked about.
  const g = setup({ conversationProjectList: list, inspectProject: { binding: null }, inspectProjectBind: params => { binds.push(params); return { binding: { origin: params.origin, root: params.root } }; } });
  await g.choice.refresh(page);
  await g.choice.created('/work/app', page);
  all(g.host, n => n.tag === 'button' && n.textContent === 'Remember')[0].fire('click'); await settle();
  assert.equal(binds.at(-1).replaces, null);
  await g.choice.created('/work/app', { url: 'chrome://newtab' });
  assert.equal(all(g.host, n => n.className === 'project-remember')[0].hidden, true);
});

test('scope names and the scopes the remember bar offers', () => {
  assert.equal(scopeName('https://github.com', '/o/r'), 'github.com/o/r');
  assert.equal(scopeName('http://localhost:3000', '/'), 'localhost:3000');
  const scopes = (url, info, bound) => rememberScopes(siteOf(url), info, bound).map(s => `${s.kind}:${s.pathScope}`);
  // A repository page: the repository first, never the bare host.
  assert.deepEqual(scopes('https://github.com/leeguooooo/image-use/pull/55', { repositoryScope: '/leeguooooo/image-use', codeHost: true }),
    ['repository:/leeguooooo/image-use', 'path:/leeguooooo/image-use/pull', 'site:/']);
  assert.deepEqual(scopes('https://gitlab.com/g/sub/app/-/issues/1', { repositoryScope: '/g/sub/app', codeHost: true }),
    ['repository:/g/sub/app', 'path:/g/sub/app/-/issues', 'site:/']);
  // A code host's other pages: a path first; its home page is not asked about.
  assert.deepEqual(scopes('https://github.com/settings/profile', { repositoryScope: null, codeHost: true }), ['path:/settings', 'site:/']);
  assert.deepEqual(scopes('https://github.com/', { repositoryScope: null, codeHost: true }), []);
  // Any other site: the whole site (or its current binding's scope) first.
  assert.deepEqual(scopes('http://localhost:3000/app/settings', { repositoryScope: null, codeHost: false }), ['site:/', 'path:/app']);
  assert.deepEqual(scopes('http://localhost:3000/admin/users/7', null, '/admin'), ['path:/admin', 'site:/', 'path:/admin/users']);
  // A host too old to say: known code hosts still default to the repository.
  assert.deepEqual(scopes('https://github.com/o/r/issues', null), ['repository:/o/r', 'site:/']);
});

const repoPage = { url: 'https://github.com/leeguooooo/image-use/pull/55' };
const repoList = {
  projects: [{ path: '/w/chatgpt-imagegen', sources: ['site'] }, { path: '/w/image-use', sources: ['claude'] }],
  page: {
    origin: 'https://github.com', codeHost: true, repositoryScope: '/leeguooooo/image-use',
    suggestion: { root: '/w/image-use', reason: 'repository', pathScope: '/leeguooooo/image-use', remote: 'origin' },
    bindings: [{ pathScope: '/', root: '/w/chatgpt-imagegen', exists: true, fingerprint: 'site-fp' }],
  },
};

test('a repository page preselects its checkout and says why', async () => {
  const f = setup({ conversationProjectList: repoList }, true);
  await f.choice.refresh(repoPage);
  assert.deepEqual(f.calls, [['conversationProjectList', { origin: 'https://github.com', pagePath: '/leeguooooo/image-use/pull/55' }]], 'one call; no site lookup');
  assert.equal(f.choice.value(), '/w/image-use');
  assert.equal(f.choice.preferred, '/w/image-use');
  assert.equal(f.select.dataset.reason, 'repository');
  assert.equal(f.select.title, 'Matched by repository github.com/leeguooooo/image-use');
  const note = all(f.host, n => n.className === 'project-match hint')[0];
  assert.equal(note.hidden, false);
  assert.equal(note.textContent, 'Matched by repository github.com/leeguooooo/image-use');
  // Creating there asks nothing: the project was matched, not picked.
  await f.choice.created('/w/image-use', repoPage);
  assert.equal(all(f.host, n => n.className === 'project-remember')[0].hidden, true);
  assert.equal(note.hidden, true, 'the note leaves with the new conversation');
  // Another pick is the user's own; the note goes.
  await f.choice.refresh(repoPage);
  f.select.value = '/w/chatgpt-imagegen'; f.select.fire('change');
  assert.equal(note.hidden, true);
  assert.equal(f.select.dataset.reason, '');
});

test('remembering on a repository page defaults to the repository and can widen in place', async () => {
  const binds = [];
  const f = setup({ conversationProjectList: repoList, inspectProjectBind: params => { binds.push(params); return { binding: { origin: params.origin, root: params.root, pathScope: params.pathScope, fingerprint: 'new' } }; } }, true);
  await f.choice.refresh(repoPage);
  await f.choice.created('/w/chatgpt-imagegen', repoPage);
  const box = all(f.host, n => n.className === 'project-remember')[0];
  assert.equal(box.hidden, false);
  assert.match(text(box), /Use this project for github\.com\/leeguooooo\/image-use from now on\?/);
  const picker = all(box, n => n.tag === 'select')[0];
  assert.deepEqual(picker.children.map(o => [o.value, o.textContent]), [
    ['/leeguooooo/image-use', 'This repository · github.com/leeguooooo/image-use'],
    ['/leeguooooo/image-use/pull', 'This path · github.com/leeguooooo/image-use/pull'],
    ['/', 'Whole site · github.com'],
  ]);
  all(box, n => n.tag === 'button' && n.textContent === 'Remember')[0].fire('click'); await settle();
  assert.deepEqual(binds.at(-1), { origin: 'https://github.com', pathScope: '/leeguooooo/image-use', root: '/w/chatgpt-imagegen', replaces: null });
  assert.match(text(box), /github\.com\/leeguooooo\/image-use now uses this project/);
  // Widening to the whole site replaces the binding shown for it.
  await f.choice.created('/w/other', repoPage);
  const again = all(f.host, n => n.className === 'project-remember')[0];
  const widen = all(again, n => n.tag === 'select')[0];
  widen.value = '/'; widen.fire('change');
  assert.match(text(again), /Use this project for github\.com from now on\?/);
  all(again, n => n.tag === 'button' && n.textContent === 'Remember')[0].fire('click'); await settle();
  assert.deepEqual(binds.at(-1), { origin: 'https://github.com', pathScope: '/', root: '/w/other', replaces: 'site-fp' });
});

test('a conversation created on another page asks the host about that page', async () => {
  const f = setup({ conversationProjectList: params => params?.pagePath === '/leeguooooo/image-use/pull/55' ? repoList : { projects: [], page: { origin: 'https://github.com', codeHost: true, repositoryScope: '/o/r', suggestion: null, bindings: [] } } }, true);
  await f.choice.refresh(repoPage);
  await f.choice.created('/w/image-use', { url: 'https://github.com/o/r' });
  const box = all(f.host, n => n.className === 'project-remember')[0];
  assert.equal(box.hidden, false, '/w/image-use is not o/r\'s project');
  assert.match(text(box), /github\.com\/o\/r/);
  assert.deepEqual(f.calls.at(-1), ['conversationProjectList', { origin: 'https://github.com', pagePath: '/o/r' }]);
});
