// `/resume`: continue an agent CLI session that was started outside the panel
// (a terminal, an IDE). The picker lists the agent's recent sessions on this
// computer; choosing one asks the host to bind a new panel conversation to it.
// Nothing is sent to the agent until the user writes the next message.

// Every user-visible string of this feature, in one place for localization.
export const RESUME_TEXT = {
  command: 'resume',
  commandHint: 'Continue a session started in a terminal',
  menuItem: 'Resume a session…',
  title: 'Resume a session',
  close: 'Close',
  agent: 'Agent',
  search: 'Search sessions',
  loading: 'Looking for sessions…',
  empty: 'No sessions found.',
  emptyQuery: query => `No sessions match “${query}”.`,
  unsupported: agent => `${agent} sessions can't be continued here.`,
  untitled: 'Untitled session',
  running: 'Active elsewhere',
  open: 'Open in panel',
  resuming: 'Opening…',
  updateCli: 'Update the chrome-use CLI to resume sessions.',
  unsafe: 'This session is still active in another terminal; continuing here can corrupt its history.',
  unsafeHint: 'Close it there first, or continue anyway.',
  confirm: 'Continue here anyway',
  cancel: 'Cancel',
  ownership: "After you continue a session here, don't keep using it in the terminal at the same time.",
  deleteKeeps: "Removes its panel history and its browser tabs from this computer. The CLI session it continued stays, and can still be used in the terminal.",
  earlier: count => `Earlier in this session · ${count} message${count === 1 ? '' : 's'}`,
  notRecorded: 'Earlier history not recorded.',
  omitted: count => `${count} earlier message${count === 1 ? ' was' : 's were'} not imported.`,
  continued: (agent, cwd) => cwd ? `Continued from a ${agent} session in ${cwd}.` : `Continued from a ${agent} session.`,
  you: 'You',
  assistant: 'Assistant',
  tools: 'Tools',
};

/** `/resume [query]` typed in the composer, or null for any other text. */
export function parseResumeCommand(text) {
  const match = /^\/resume(?:\s+([\s\S]*))?$/.exec(String(text || '').trim());
  return match ? { query: (match[1] || '').trim() } : null;
}

/** The last folder name of a session's directory. */
export function projectName(cwd) {
  const parts = String(cwd || '').split(/[\\/]+/).filter(Boolean);
  return parts[parts.length - 1] || '';
}

/**
 * The picker. `agents()` lists `{id, name}` of the panel's chat agents,
 * `preferredAgent()` the one to show first, `executable(agentId)` an
 * optional custom CLI path, and `onResumed(conversation, existing)` opens the
 * resumed (or already continued) conversation.
 */
export function createResumePicker({ document, api, agents, preferredAgent, executable = () => '', onResumed, onError, relativeTime = () => '', schedule = setTimeout, cancel = clearTimeout }) {
  const element = (tag, text, className) => {
    const node = document.createElement(tag);
    if (text !== undefined) node.textContent = text;
    if (className) node.className = className;
    return node;
  };
  const button = (text, action, className = 'quiet') => {
    const node = element('button', text, className); node.type = 'button';
    node.addEventListener('click', () => void Promise.resolve().then(action).catch(onError));
    return node;
  };
  const container = element('section', undefined, 'resume-picker'); container.hidden = true;
  container.setAttribute('role', 'dialog'); container.setAttribute('aria-label', RESUME_TEXT.title);
  const head = element('div', undefined, 'resume-head');
  const agent = element('select'); agent.setAttribute('aria-label', RESUME_TEXT.agent);
  head.append(element('strong', RESUME_TEXT.title), agent, button(RESUME_TEXT.close, () => close()));
  const search = element('input'); search.type = 'search'; search.placeholder = RESUME_TEXT.search; search.setAttribute('aria-label', RESUME_TEXT.search);
  const status = element('p', '', 'hint');
  const items = element('div', undefined, 'resume-items'); items.setAttribute('role', 'listbox'); items.setAttribute('aria-label', RESUME_TEXT.title);
  const confirm = element('section', undefined, 'delete-confirm'); confirm.hidden = true; confirm.setAttribute('role', 'alertdialog');
  container.append(head, search, status, items, confirm, element('p', RESUME_TEXT.ownership, 'hint'));
  let epoch = 0, timer = null, busy = false, rows = [];

  function close() { container.hidden = true; confirm.hidden = true; epoch++; if (timer) cancel(timer); timer = null; }
  function open(query = '', agentId = '') {
    const list = agents();
    agent.replaceChildren(...list.map(({ id, name }) => { const option = element('option', name); option.value = id; return option; }));
    const wanted = agentId || preferredAgent();
    agent.value = list.some(a => a.id === wanted) ? wanted : list[0]?.id || '';
    search.value = query; confirm.hidden = true; container.hidden = false;
    search.focus?.();
    return load();
  }
  function nameOf(id) { return agents().find(a => a.id === id)?.name || id; }
  async function load() {
    const current = ++epoch, agentId = agent.value, query = search.value.trim();
    status.textContent = RESUME_TEXT.loading; status.hidden = false; items.replaceChildren(); rows = [];
    const params = { agentId, limit: 50 };
    if (query) params.query = query;
    const path = executable(agentId); if (path) params.executable = path;
    let result;
    try { result = await api('conversationSessionList', params); }
    catch (failure) { if (current === epoch) status.textContent = String(failure?.message || failure); return; }
    if (current !== epoch) return;
    if (!result.supported) { status.textContent = RESUME_TEXT.unsupported(nameOf(agentId)); return; }
    rows = result.sessions;
    status.textContent = rows.length ? '' : query ? RESUME_TEXT.emptyQuery(query) : RESUME_TEXT.empty;
    status.hidden = Boolean(rows.length);
    items.replaceChildren(...rows.map(row => {
      const item = element('button', undefined, 'conversation-item resume-item'); item.type = 'button'; item.setAttribute('role', 'option');
      if (row.cwd) item.title = row.cwd;
      const title = element('span', row.title || RESUME_TEXT.untitled, 'title');
      const meta = element('span', [nameOf(row.agentId), projectName(row.cwd), relativeTime(row.lastActive)].filter(Boolean).join(' · '), 'meta');
      item.append(title, meta);
      if (row.running) item.append(element('span', RESUME_TEXT.running, 'badge running'));
      if (row.conversationId) item.append(element('span', RESUME_TEXT.open, 'badge'));
      item.addEventListener('click', () => void choose(row).catch(onError));
      return item;
    }));
  }
  async function choose(row, confirmRunning = false) {
    if (busy) return;
    if (row.conversationId) { close(); await onResumed({ id: row.conversationId }, true); return; }
    busy = true; status.textContent = RESUME_TEXT.resuming; status.hidden = false;
    try {
      const params = { agentId: row.agentId, sessionId: row.sessionId };
      if (confirmRunning) params.confirmRunning = true;
      const path = executable(row.agentId); if (path) params.executable = path;
      const result = await api('conversationResume', params);
      if (result.confirmRequired === 'running') { askRunning(row); return; }
      close();
      await onResumed(result.conversation, result.existing === true);
    } finally { busy = false; if (!container.hidden && status.textContent === RESUME_TEXT.resuming) { status.textContent = ''; status.hidden = true; } }
  }
  // A session another terminal may still be writing is never attached
  // silently: the user confirms, and the host checks again.
  function askRunning(row) {
    confirm.replaceChildren(element('p', RESUME_TEXT.unsafe), element('p', RESUME_TEXT.unsafeHint, 'hint'));
    const actions = element('div', undefined, 'actions');
    actions.append(button(RESUME_TEXT.confirm, () => { confirm.hidden = true; return choose(row, true); }, 'danger'), button(RESUME_TEXT.cancel, () => { confirm.hidden = true; }));
    confirm.append(actions); confirm.hidden = false;
  }
  agent.addEventListener('change', () => void load());
  search.addEventListener('input', () => { if (timer) cancel(timer); timer = schedule(() => { timer = null; void load(); }, 250); });
  search.addEventListener('keydown', event => { if (event.key === 'Escape') { event.preventDefault?.(); close(); } });
  return { container, open, close, choose, get rows() { return rows; }, get isOpen() { return !container.hidden; } };
}

/**
 * The collapsed block of history imported from the CLI session, or null.
 * `imported` is the timeline's projection of `importedHistory` events.
 */
export function importedView(document, imported, { agentName = id => id, renderMarkdown } = {}) {
  if (!imported?.messages?.length && !imported?.end) return null;
  const element = (tag, text, className) => {
    const node = document.createElement(tag);
    if (text !== undefined) node.textContent = text;
    if (className) node.className = className;
    return node;
  };
  const end = imported.end || {};
  const block = element('section', undefined, 'imported-history');
  block.append(element('p', RESUME_TEXT.continued(agentName(end.agentId), end.cwd || ''), 'hint'));
  const details = element('details');
  details.append(element('summary', RESUME_TEXT.earlier(imported.messages.length)));
  if (end.partial || end.omitted > 0 || end.unparsed > 0 || !imported.end) {
    const note = element('p', RESUME_TEXT.notRecorded, 'hint');
    if (end.omitted > 0) note.textContent += ` ${RESUME_TEXT.omitted(end.omitted)}`;
    details.append(note);
  }
  for (const message of imported.messages) {
    const row = element('div', undefined, `imported-message ${message.role}`);
    row.append(element('div', message.role === 'user' ? RESUME_TEXT.you : message.role === 'tool' ? RESUME_TEXT.tools : RESUME_TEXT.assistant, 'speaker'));
    const content = element('div', undefined, 'content');
    if (message.role === 'assistant' && renderMarkdown) renderMarkdown(document, content, message.text);
    else content.textContent = message.text;
    row.append(content); details.append(row);
  }
  block.append(details);
  return block;
}
