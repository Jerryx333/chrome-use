// Fake chrome.* and agent host for the store screenshots. Loaded before
// sidepanel.js / options.js by scripts/store-screenshots.sh; never shipped.
const q = new URLSearchParams(location.search), s = q.get('s') || 'chat'
const now = Date.now()
let seq = 0
const ev = (kind, data) => ({ sequence: ++seq, kind, data, at: now })
const page = { title: 'Pricing — Acme Cloud', url: 'https://acme.example/pricing' }
const answer = 'Acme Cloud has **3 plans** (yearly billing shown):\n\n| Plan | Price | Storage | Seats |\n|---|---|---|---|\n| Starter | $0 | 5 GB | 1 |\n| Pro | $12/mo | 100 GB | 5 |\n| Team | $30/mo | 1 TB | Unlimited |\n\n- Yearly billing saves `20%`\n- Only Team includes SSO and audit logs\n\nWant me to save this as a CSV?'
const step = (messageId, id, tool, args) => ev('tool', { messageId, completed: true, item: { id, type: 'mcpToolCall', tool, arguments: args, result: 'ok' } })
const scenarios = {
  empty: () => [],
  // A finished answer: steps folded, table in view.
  idle: () => [
    ev('user', { messageId: 'm1', text: 'Compare the plans on this page and put them in a table', page }),
    step('m1', 't1', 'chrome_use_snapshot', {}), step('m1', 't2', 'chrome_use_click', { ref: '@e14' }),
    step('m1', 't3', 'chrome_use_scroll', { direction: 'down' }), step('m1', 't4', 'chrome_use_get_text', { ref: '@e31' }),
    ev('textFinal', { messageId: 'm1', itemId: 'a1', part: 0, text: answer }),
  ],
  // The agent stops and asks before writing a file.
  approve: () => [
    ev('user', { messageId: 'm1', text: 'Save the plans on this page to my Downloads as a CSV', page }),
    step('m1', 't1', 'chrome_use_snapshot', {}), step('m1', 't2', 'chrome_use_get_text', { ref: '@e31' }),
    ev('textFinal', { messageId: 'm1', itemId: 'a1', part: 0, text: 'Found **3 plans**: Starter, Pro and Team. Writing them to a CSV now.' }),
    ev('request', { requestId: 7, method: 'item/commandExecution/requestApproval', params: { reason: 'Write the plan table to your Downloads folder', command: 'cat > ~/Downloads/acme-plans.csv', cwd: '~' } }),
  ],
}
const events = (scenarios[s] || scenarios.idle)()
const conv = { id: 'c1', title: 'Compare Acme plans', agentId: 'codex', site: 'acme.example', updated: now - 60000,
  phase: s === 'approve' ? 'waitingInput' : 'idle', model: 'gpt-5.5', fullAccess: false, queued: 0 }
const convs = s === 'empty' ? [] : [conv,
  { id: 'c2', title: 'Summarize my GitHub notifications', agentId: 'claude', site: 'github.com', updated: now - 7200e3 },
  { id: 'c3', title: 'Draft replies to support tickets', agentId: 'codex', site: 'acme.example', updated: now - 3 * 86400e3 }]
const agents = {
  codex: { version: '0.48.0', kind: 'chatgpt', models: ['gpt-5.5', 'gpt-5.5-mini'] },
  claude: { version: '2.1.4', kind: 'claude.ai', models: ['opus', 'sonnet', 'haiku'] },
  opencode: { version: '1.0.12', kind: 'api key', models: ['anthropic/claude-sonnet', 'openai/gpt-5.5'] },
}
const ops = {
  conversationList: () => ({ conversations: convs }),
  conversationRead: ({ after }) => ({ conversation: conv, journal: { events: events.filter(e => e.sequence > after), cursor: seq, more: false }, queuedMessages: [] }),
  discover: () => ({ agents: [
    { id: 'codex', name: 'Codex', executable: '/usr/local/bin/codex' },
    { id: 'claude', name: 'Claude Code', executable: '/usr/local/bin/claude' },
    { id: 'opencode', name: 'OpenCode', executable: '/usr/local/bin/opencode' },
    { id: 'pi', name: 'Pi', executable: null }] }),
  agentInspect: ({ agentId }) => {
    const agent = agents[agentId] || { version: '', models: [] }
    return { inspection: { agentId, versionReported: agent.version, complete: true,
      account: agent.kind ? { status: 'reportedSignedIn', kind: agent.kind } : { status: 'reportedSignedOut' },
      models: agent.models.map(id => ({ id, label: id })) } }
  },
  skillList: () => ({ skills: [
    { id: 'lib:compare-plans', name: 'compare-plans', source: 'library', enabled: true, editable: true, description: 'Read every pricing tier on the page and return a comparison table with prices, limits and the cheapest fit.' },
    { id: 'lib:triage-inbox', name: 'triage-inbox', source: 'library', enabled: false, editable: true, description: 'Sort unread notifications into reply, read later and ignore.' },
    { id: 'codex:release-notes', name: 'release-notes', source: 'codex', enabled: true, editable: false, description: 'Draft release notes from merged pull requests.' }] }),
  skillRead: () => ({ skill: { instructions: '---\nname: compare-plans\ndescription: Compare pricing tiers\n---\n\nRead every plan on the page and return a table.' } }),
  scheduleList: () => ({ schedules: [
    { id: 's1', name: 'Morning GitHub digest', enabled: true, agentId: 'claude', lastResult: 'completed', lastChat: 'c2', nextRun: now + 5 * 3600e3, rule: { kind: 'weekdays', time: '09:00' }, prompt: 'Summarize my GitHub notifications', options: {} },
    { id: 's2', name: 'Check Acme pricing for changes', enabled: true, agentId: 'codex', lastResult: 'unconfirmed', nextRun: now + 40 * 60e3, rule: { kind: 'interval', minutes: 60 }, prompt: 'Compare the plans', options: {} },
    { id: 's3', name: 'Weekly support ticket sweep', enabled: false, agentId: 'codex', lastResult: null, nextRun: now + 3 * 86400e3, rule: { kind: 'cron', expression: '0 8 * * 1' }, prompt: 'Draft replies', options: {} }] }),
  configRead: ({ agentId }) => ({ configuration: { agentId, path: '~/.codex/config.toml', warnings: [],
    fields: [{ id: 'model', value: 'gpt-5.5' }, { id: 'effort', value: 'high' }] } }),
  configPlan: () => ({ configuration: { agentId: 'codex', planId: 'p', files: [{ changes: [{ path: ['effort'], before: { value: 'high' }, after: { value: 'xhigh' } }] }] } }),
}
const status = { connected: true, tabCount: 3, agentPanel: { schemaVersion: 1, mode: 'full', chatAgents: ['codex', 'claude', 'opencode', 'pi'],
  fullAccess: true, pageContext: true, modelInspection: true, conversationDelete: true, skillLibrary: q.get('lib') !== '0', schedules: q.get('lib') !== '0' } }
function reply(m) {
  if (m.type === 'ab-status') return status
  const f = ops[m.operation]
  return f ? { success: true, result: f(m.params || {}) } : { success: false, error: 'no mock ' + m.operation }
}
const store = { ab_panel_conversation: s === 'empty' ? '' : 'c1' }
// The real UI text: _locales/<lang>/messages.json, with Chrome's $NAME$ / $1 substitution.
const lang = q.get('lang') || 'en'
const messages = (() => { const x = new XMLHttpRequest(); x.open('GET', `../ab-connect/_locales/${lang}/messages.json`, false); x.send(); return JSON.parse(x.responseText) })()
const i18n = {
  getUILanguage: () => lang.replace('_', '-'),
  getMessage(key, subs = []) {
    const entry = messages[key]
    if (!entry) return ''
    const list = [].concat(subs)
    const fill = text => text.replace(/\$(\d)/g, (_, n) => list[n - 1] ?? '')
    return entry.message.replace(/\$([A-Za-z_]+)\$/g, (_, name) => fill(entry.placeholders?.[name.toLowerCase()]?.content || '')).replace(/\$\$/g, '$')
  },
}
globalThis.chrome = {
  i18n,
  runtime: {
    sendMessage(m, cb) { const r = reply(m); if (cb) { cb(r); return } return Promise.resolve(r) },
    onMessage: { addListener() {} }, openOptionsPage() {}, getManifest: () => ({ version: q.get('v') || '0.5.53' }),
  },
  storage: { local: { async get() { return store }, async set() {} }, sync: { get(d, cb) { const v = typeof d === 'object' && d ? d : {}; if (cb) { cb(v); return } return Promise.resolve(v) }, async set() {} } },
  tabs: { async query() { return [{ id: 5, ...page }] }, create() {}, onActivated: { addListener() {} }, onUpdated: { addListener() {} } },
}
if (q.has('agents')) setTimeout(() => document.getElementById('agentsTab').click(), 300)
if (q.has('config')) setTimeout(() => document.querySelector('.agent-row button')?.click(), 700)
if (q.has('tab')) setTimeout(() => document.getElementById(`${q.get('tab')}SettingsTab`)?.click(), 500)
if (q.has('edit')) setTimeout(() => [...document.querySelectorAll(q.get('tab') === 'skills' ? '#skillEntries button' : '#scheduleEntries button')].find(b => /Edit|编辑/.test(b.textContent))?.click(), 900)
if (q.has('plan')) setTimeout(() => {
  const input = document.querySelector('#configFields input[data-field="effort"]')
  if (!input) return
  input.value = 'xhigh'
  input.dispatchEvent(new Event('input', { bubbles: true }))
  document.getElementById('configForm').requestSubmit()
  // Scroll the panel itself; scrollIntoView would also move the page around it.
  setTimeout(() => {
    const row = document.querySelector('.agent-row.selected'), body = document.querySelector('.agents-body')
    if (row && body) body.scrollTop = row.offsetTop - body.offsetTop - 6
  }, 400)
}, 1400)
