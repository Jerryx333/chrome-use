import './test-i18n.js';
import assert from 'node:assert/strict';
import test from 'node:test';
import { parseResumeCommand, projectName, importedView, createResumePicker, groupSessions, PROJECT_PREVIEW, RESUME_TEXT } from './resume-picker.js';

class Node {
  constructor(tag = 'div') { this.tag = tag; this.children = []; this.listeners = {}; this.textContent = ''; this.hidden = false; this.value = ''; this.attributes = {}; }
  append(...nodes) { for (const node of nodes) { node.parent = this; this.children.push(node); } }
  replaceChildren(...nodes) { this.children = []; this.append(...nodes); }
  addEventListener(type, listener) { this.listeners[type] = listener; }
  setAttribute(name, value) { this.attributes[name] = value; }
  fire(type, details = {}) { this.listeners[type]?.({ preventDefault() {}, ...details }); }
}
const document = { createElement: tag => new Node(tag) };
const text = node => [node.textContent, ...node.children.map(text)].filter(Boolean).join(' ');
const all = (node, predicate) => [...(predicate(node) ? [node] : []), ...node.children.flatMap(child => all(child, predicate))];
const settle = async () => { for (let i = 0; i < 4; i++) await new Promise(resolve => setImmediate(resolve)); };

test('the command is /resume with an optional search', () => {
  assert.deepEqual(parseResumeCommand('/resume'), { query: '' });
  assert.deepEqual(parseResumeCommand('  /resume   fix login  '), { query: 'fix login' });
  for (const other of ['/resumes', '/resume-x', 'resume', 'please /resume', '/review']) assert.equal(parseResumeCommand(other), null, other);
  assert.equal(projectName('/Users/me/work/app/'), 'app');
  assert.equal(projectName('C:\\work\\app'), 'app');
  assert.equal(projectName(null), '');
});

test('imported history says what was not recorded', () => {
  assert.equal(importedView(document, null), null);
  const complete = importedView(document, { messages: [{ role: 'user', text: 'hi' }], end: { agentId: 'pi', cwd: '/p', omitted: 0, unparsed: 0, partial: false } }, { agentName: () => 'Pi' });
  assert.doesNotMatch(text(complete), /not recorded/);
  assert.match(text(complete), /Continued from a Pi session in \/p\. Earlier in this session · 1 message You hi/);
  const partial = importedView(document, { messages: [], end: { agentId: 'pi', partial: true, omitted: 0 } });
  assert.match(text(partial), new RegExp(RESUME_TEXT.notRecorded.replace('.', '\\.')));
  // No closing event: the import itself did not finish.
  assert.match(text(importedView(document, { messages: [{ role: 'tool', text: 'Read' }], end: null })), /not recorded/);
});

test('the picker lists, searches and opens an already continued session without resuming it again', async () => {
  const calls = [], opened = [];
  let pending = null;
  const picker = createResumePicker({
    document,
    api: async (operation, params) => { calls.push([operation, params]); return { schemaVersion: 1, agentId: params.agentId, supported: params.agentId !== 'pi', sessions: [{ sessionId: 's1', agentId: params.agentId, title: null, cwd: '/w/app', lastActive: 0, running: false, conversationId: 'conv-1' }] }; },
    agents: () => [{ id: 'codex', name: 'Codex' }, { id: 'pi', name: 'Pi' }],
    preferredAgent: () => 'codex',
    executable: agentId => agentId === 'codex' ? '/opt/codex' : '',
    onResumed: async (conversation, existing) => opened.push([conversation.id, existing]),
    onError: error => { throw error; },
    schedule: callback => { pending = callback; return 1; },
    cancel: () => { pending = null; },
  });
  await picker.open('app');
  assert.equal(picker.isOpen, true);
  assert.deepEqual(calls[0], ['conversationSessionList', { agentId: 'codex', limit: 100, query: 'app', executable: '/opt/codex' }]);
  assert.match(text(picker.container), /▾ app 1 Untitled session Open in panel/);
  assert.match(text(picker.container), /Open in panel/);
  // Typing searches after a pause.
  const search = all(picker.container, n => n.tag === 'input')[0];
  search.value = 'other'; search.fire('input'); assert.ok(pending); pending(); await settle();
  assert.equal(calls[1][1].query, 'other');
  // Choosing a continued session opens it; nothing is resumed twice.
  all(picker.container, n => n.className?.split?.(' ').includes('resume-item'))[0].fire('click'); await settle();
  assert.deepEqual(opened, [['conv-1', true]]);
  assert.ok(!calls.some(([operation]) => operation === 'conversationResume'));
  assert.equal(picker.isOpen, false);
  // An agent whose sessions cannot be continued says so.
  await picker.open('', 'pi');
  assert.match(text(picker.container), /Pi sessions can't be continued here/);
});

test('sessions group by folder, the latest project first and folderless ones last', () => {
  const rows = [
    { sessionId: 'a1', cwd: '/w/app/', lastActive: 50 },
    { sessionId: 'n1', cwd: null, lastActive: 90 },
    { sessionId: 'b1', cwd: '/w/blog', lastActive: 80 },
    { sessionId: 'a2', cwd: '/w/app', lastActive: 70 },
  ];
  const ids = groups => groups.map(g => [g.key, g.sessions.map(s => s.sessionId)]);
  assert.deepEqual(ids(groupSessions(rows)), [['/w/blog', ['b1']], ['/w/app', ['a1', 'a2']], ['', ['n1']]]);
  // The site's project leads.
  assert.deepEqual(ids(groupSessions(rows, '/w/app/')).map(([key]) => key), ['/w/app', '/w/blog', '']);
  assert.deepEqual(ids(groupSessions(rows, '/w/none')).map(([key]) => key), ['/w/blog', '/w/app', '']);
});

test('the picker shows a few sessions per project, folds projects and moves with the arrow keys', async () => {
  const sessions = [
    ...Array.from({ length: 5 }, (_, i) => ({ sessionId: `a${i}`, agentId: 'codex', title: `app ${i}`, cwd: '/w/app', lastActive: 100 - i, running: false, conversationId: null })),
    { sessionId: 'b0', agentId: 'codex', title: 'blog 0', cwd: '/w/blog', lastActive: 200, running: false, conversationId: null },
    { sessionId: 'n0', agentId: 'codex', title: 'loose', cwd: null, lastActive: 300, running: false, conversationId: null },
  ];
  const calls = [];
  const picker = createResumePicker({
    document,
    api: async (operation, params) => {
      calls.push([operation, params]);
      if (operation === 'conversationResume') return { conversation: { id: 'c-new' } };
      const query = (params.query || '').toLowerCase();
      return { agentId: params.agentId, supported: true, sessions: sessions.filter(s => !query || `${s.title} ${s.cwd}`.toLowerCase().includes(query)) };
    },
    agents: () => [{ id: 'codex', name: 'Codex' }],
    preferredAgent: () => 'codex',
    preferredProject: () => '/w/app',
    onResumed: async () => {},
    onError: error => { throw error; },
    schedule: callback => { callback(); return 1; },
    cancel: () => {},
  });
  await picker.open();
  const items = () => all(picker.container, n => n.className?.split?.(' ').includes('resume-item'));
  const headers = () => all(picker.container, n => n.className === 'resume-project');
  const more = () => all(picker.container, n => n.className?.includes?.('resume-more'));
  // The site's project first, then the latest; folderless last.
  assert.deepEqual(headers().map(h => text(h)), ['▾ app This site 5', '▾ blog 1', `▾ ${RESUME_TEXT.other} 1`]);
  assert.equal(headers()[0].title, '/w/app');
  assert.deepEqual(items().map(i => text(i.children[0])), ['app 0', 'app 1', 'app 2', 'blog 0', 'loose']);
  assert.equal(PROJECT_PREVIEW, 3);
  assert.equal(text(more()[0]), 'Show 2 more');
  more()[0].fire('click'); await settle();
  assert.equal(items().length, 7);
  assert.equal(text(more()[0]), 'Show less');
  // Folding a project hides its sessions, and it stays folded.
  headers()[0].fire('click');
  assert.equal(headers()[0].attributes['aria-expanded'], 'false');
  assert.deepEqual(items().map(i => text(i.children[0])), ['blog 0', 'loose']);
  // Arrows walk the sessions on screen; Enter continues the highlighted one.
  const search = all(picker.container, n => n.tag === 'input')[0];
  search.fire('keydown', { key: 'ArrowDown' });
  search.fire('keydown', { key: 'ArrowDown' });
  assert.deepEqual(items().map(i => i.attributes['aria-selected']), ['false', 'true']);
  search.fire('keydown', { key: 'ArrowDown' });
  assert.equal(items()[0].attributes['aria-selected'], 'true', 'wraps around');
  search.fire('keydown', { key: 'ArrowUp' });
  search.fire('keydown', { key: 'Enter' }); await settle();
  assert.deepEqual(calls.at(-1), ['conversationResume', { agentId: 'codex', sessionId: 'n0' }]);
  assert.equal(picker.isOpen, false);
  // A search shows every match, folded or not, still by project.
  await picker.open('app');
  assert.deepEqual(headers().map(h => text(h)), ['▾ app This site 5']);
  assert.equal(items().length, 5);
  assert.equal(more().length, 0);
});

test('projects with the same folder name show their parents', async () => {
  const picker = createResumePicker({
    document,
    api: async (operation, params) => ({ agentId: params.agentId, supported: true, sessions: [
      { sessionId: 'x', agentId: 'claude', title: 'x', cwd: '/runs/r1/.protocol', lastActive: 2, running: true, conversationId: null },
      { sessionId: 'y', agentId: 'claude', title: 'y', cwd: '/runs/r2/.protocol', lastActive: 1, running: false, conversationId: null },
    ] }),
    agents: () => [{ id: 'claude', name: 'Claude Code' }], preferredAgent: () => 'claude',
    onResumed: async () => {}, onError: error => { throw error; },
  });
  await picker.open();
  assert.deepEqual(all(picker.container, n => n.className === 'resume-project').map(h => text(h)), ['▾ .protocol · runs/r1 1', '▾ .protocol · runs/r2 1']);
  assert.match(text(picker.container), /x Active elsewhere/);
});

function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

function selectionFixture(conversationId = null) {
  const requests = [], resumes = [], opened = [], errors = [];
  let scheduled = null;
  const picker = createResumePicker({
    document,
    api: async (operation, params) => {
      if (operation === 'conversationResume') {
        resumes.push(params);
        return { conversation: { id: 'resumed' } };
      }
      const request = { params, ...deferred() }; requests.push(request);
      return request.promise;
    },
    agents: () => [{ id: 'codex', name: 'Codex' }, { id: 'pi', name: 'Pi' }],
    preferredAgent: () => 'codex',
    onResumed: async conversation => opened.push(conversation.id),
    onError: error => errors.push(error),
    schedule: callback => { scheduled = callback; return 1; },
    cancel: () => { scheduled = null; },
  });
  const search = all(picker.container, node => node.tag === 'input')[0];
  const agent = all(picker.container, node => node.tag === 'select')[0];
  const row = (sessionId = 'old', agentId = 'codex') => ({ sessionId, agentId, title: sessionId, cwd: '/project', lastActive: 1, conversationId });
  return { picker, requests, resumes, opened, errors, search, agent, row,
    async start() { const loading = picker.open(); requests[0].resolve({ supported: true, sessions: [row()] }); await loading; },
    runSearch() { assert.ok(scheduled); const callback = scheduled; scheduled = null; callback(); },
    visible() { return all(picker.container, node => node.className?.split(' ').includes('resume-item')); },
  };
}

for (const conversationId of [null, 'existing-conversation']) {
  for (const outcome of ['pending', 'failed', 'unsupported']) {
    test(`agent change cannot select an old ${conversationId ? 'continued' : 'CLI'} session while ${outcome}`, async () => {
      const f = selectionFixture(conversationId); await f.start();
      f.search.fire('keydown', { key: 'ArrowDown' });
      f.agent.value = 'pi'; f.agent.fire('change');
      if (outcome === 'failed') f.requests[1].reject(new Error('Session listing unavailable'));
      if (outcome === 'unsupported') f.requests[1].resolve({ supported: false, sessions: [] });
      await settle();
      assert.equal(f.visible().length, 0);
      f.search.fire('keydown', { key: 'ArrowDown' });
      f.search.fire('keydown', { key: 'Enter' }); await settle();
      assert.deepEqual(f.resumes, []);
      assert.deepEqual(f.opened, []);
      assert.deepEqual(f.errors, []);
      assert.equal(f.picker.isOpen, true);
      if (outcome === 'pending') { f.requests[1].resolve({ supported: true, sessions: [] }); await settle(); }
    });
  }
}

test('editing a search invalidates selection and in-flight results before debounce fires', async () => {
  const f = selectionFixture(); await f.start();
  f.search.value = 'first'; f.search.fire('input');
  f.search.fire('keydown', { key: 'Enter' }); await settle();
  assert.deepEqual(f.resumes, []);
  assert.equal(f.visible().length, 0);
  f.runSearch();
  f.search.value = 'second'; f.search.fire('input');
  // The previous request finishes during the new query's debounce delay.
  f.requests[1].resolve({ supported: true, sessions: [f.row('first-result')] }); await settle();
  assert.equal(f.visible().length, 0);
  f.search.fire('keydown', { key: 'Enter' }); await settle();
  assert.deepEqual(f.resumes, []);
  f.runSearch();
  assert.equal(f.requests[2].params.query, 'second');
  f.requests[2].resolve({ supported: true, sessions: [f.row('second-result')] }); await settle();
  assert.equal(f.visible().length, 1);
  f.search.fire('keydown', { key: 'ArrowDown' });
  f.search.fire('keydown', { key: 'Enter' }); await settle();
  assert.deepEqual(f.resumes, [{ agentId: 'codex', sessionId: 'second-result' }]);
  assert.deepEqual(f.opened, ['resumed']);
  assert.deepEqual(f.errors, []);
});

test('a reopened picker ignores the old listing and resumes the newly loaded agent', async () => {
  const f = selectionFixture(); await f.start();
  const oldLoad = f.picker.open('old');
  f.picker.close();
  const newLoad = f.picker.open('', 'pi');
  f.search.fire('keydown', { key: 'Enter' }); await settle();
  assert.deepEqual(f.resumes, []);
  f.requests[2].resolve({ supported: true, sessions: [f.row('pi-session', 'pi')] }); await newLoad;
  f.requests[1].resolve({ supported: true, sessions: [f.row('stale-result')] }); await oldLoad;
  assert.equal(f.visible()[0].children[0].textContent, 'pi-session');
  f.search.fire('keydown', { key: 'Enter' }); await settle();
  assert.deepEqual(f.resumes, [{ agentId: 'pi', sessionId: 'pi-session' }]);
  assert.deepEqual(f.errors, []);
});

for (const outcome of ['success', 'running', 'failure']) {
  test(`changing agent ignores an outstanding resume ${outcome}`, async () => {
    const pending = deferred(), opened = [], errors = [];
    const picker = createResumePicker({ document,
      api: async operation => operation === 'conversationResume' ? pending.promise : { supported: true, sessions: [] },
      agents: () => [{ id: 'codex', name: 'Codex' }, { id: 'pi', name: 'Pi' }], preferredAgent: () => 'codex',
      onResumed: async conversation => opened.push(conversation.id), onError: error => errors.push(error),
    });
    await picker.open();
    const choosing = picker.choose({ agentId: 'codex', sessionId: 'old' });
    const agent = all(picker.container, node => node.tag === 'select')[0];
    agent.value = 'pi'; agent.fire('change'); await settle();
    if (outcome === 'failure') pending.reject(new Error('Old request failed'));
    else pending.resolve(outcome === 'running' ? { confirmRequired: 'running' } : { conversation: { id: 'old-conversation' } });
    await choosing;
    assert.deepEqual(opened, []);
    assert.deepEqual(errors, []);
    assert.equal(picker.isOpen, true);
    assert.equal(all(picker.container, node => node.className === 'delete-confirm')[0].hidden, true);
  });
}

for (const change of ['agent', 'search', 'close']) {
  test(`${change} invalidates the previous running-session confirmation`, async () => {
    const calls = [], opened = [];
    const picker = createResumePicker({ document,
      api: async (operation, params) => {
        calls.push([operation, params]);
        return operation === 'conversationResume' ? { confirmRequired: 'running' } : { supported: true, sessions: [] };
      },
      agents: () => [{ id: 'codex', name: 'Codex' }, { id: 'pi', name: 'Pi' }], preferredAgent: () => 'codex',
      onResumed: async conversation => opened.push(conversation.id), onError: error => { throw error; },
      schedule: () => 1, cancel: () => {},
    });
    await picker.open(); await picker.choose({ agentId: 'codex', sessionId: 'old' });
    const confirm = all(picker.container, node => node.className === 'delete-confirm')[0];
    assert.equal(confirm.hidden, false);
    // Button actions run in a microtask. Invalidate even an already queued confirmation.
    all(confirm, node => node.className === 'danger')[0].fire('click');
    if (change === 'close') picker.close();
    else {
      const control = all(picker.container, node => node.tag === (change === 'agent' ? 'select' : 'input'))[0];
      control.value = change === 'agent' ? 'pi' : 'new'; control.fire(change === 'agent' ? 'change' : 'input');
    }
    await settle();
    assert.equal(confirm.hidden, true);
    assert.equal(calls.filter(([operation]) => operation === 'conversationResume').length, 1);
    assert.deepEqual(opened, []);
  });
}

test('closing and reopening ignores the previous resume completion and permits a fresh choice', async () => {
  const pending = deferred(), opened = [];
  let resumes = 0;
  const picker = createResumePicker({ document,
    api: async operation => operation === 'conversationResume'
      ? ++resumes === 1 ? pending.promise : { conversation: { id: 'new-conversation' } }
      : { supported: true, sessions: [] },
    agents: () => [{ id: 'codex', name: 'Codex' }], preferredAgent: () => 'codex',
    onResumed: async conversation => opened.push(conversation.id), onError: error => { throw error; },
  });
  await picker.open(); const choosing = picker.choose({ agentId: 'codex', sessionId: 'old' });
  picker.close(); await picker.open();
  pending.resolve({ conversation: { id: 'old-conversation' } }); await choosing;
  assert.deepEqual(opened, []); assert.equal(picker.isOpen, true);
  await picker.choose({ agentId: 'codex', sessionId: 'new' });
  assert.deepEqual(opened, ['new-conversation']); assert.equal(picker.isOpen, false);
});

for (const failure of ['resume', 'open']) {
  test(`a current ${failure} failure is still reported`, async () => {
    const picker = createResumePicker({ document,
      api: async operation => {
        if (operation !== 'conversationResume') return { supported: true, sessions: [] };
        if (failure === 'resume') throw new Error('Current resume failed');
        return { conversation: { id: 'current' } };
      },
      agents: () => [{ id: 'codex', name: 'Codex' }], preferredAgent: () => 'codex',
      onResumed: async () => { throw new Error('Current open failed'); }, onError: error => { throw error; },
    });
    await picker.open();
    await assert.rejects(picker.choose({ agentId: 'codex', sessionId: 'current' }), new RegExp(`Current ${failure} failed`));
  });
}

test('the current running-session confirmation still resumes the chosen session', async () => {
  const calls = [], opened = [];
  const picker = createResumePicker({ document,
    api: async (operation, params) => {
      if (operation !== 'conversationResume') return { supported: true, sessions: [] };
      calls.push(params);
      return params.confirmRunning ? { conversation: { id: 'current' } } : { confirmRequired: 'running' };
    },
    agents: () => [{ id: 'codex', name: 'Codex' }], preferredAgent: () => 'codex',
    onResumed: async conversation => opened.push(conversation.id), onError: error => { throw error; },
  });
  await picker.open(); await picker.choose({ agentId: 'codex', sessionId: 'current' });
  all(picker.container, node => node.className === 'danger')[0].fire('click'); await settle();
  assert.deepEqual(calls, [{ agentId: 'codex', sessionId: 'current' }, { agentId: 'codex', sessionId: 'current', confirmRunning: true }]);
  assert.deepEqual(opened, ['current']); assert.equal(picker.isOpen, false);
});
