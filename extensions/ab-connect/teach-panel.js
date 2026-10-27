// The side panel's Record control (teach by demonstration): record what the
// user does in the panel's tab, show the steps for review and edit, then save
// a `chrome-use script` the user can replay with no model. Kept apart from the
// conversation controller in sidepanel.js; the background records
// (teach-controller.js) and the native host compiles and saves.

/** Parse the reviewed program; it must stay a JSON op-list. */
export function parseProgram(text) {
  let program;
  try { program = JSON.parse(text); } catch (error) { throw new Error(`not JSON: ${error.message}`); }
  if (!Array.isArray(program) || !program.length || !program.every(op => op && typeof op === 'object' && !Array.isArray(op))) {
    throw new Error('the script must be a JSON array of steps');
  }
  return program;
}

export function mountTeach({ doc, runtime, tabs, permissions, panelTab, t }) {
  const button = doc.getElementById('teachRecord');
  const view = doc.getElementById('teachReview');
  if (!button || !view || !Number.isInteger(panelTab)) return null;
  let state = 'idle';
  const el = (tag, text, className) => {
    const node = doc.createElement(tag);
    if (text != null) node.textContent = text;
    if (className) node.className = className;
    return node;
  };
  const send = message => Promise.resolve(runtime.sendMessage(message));
  const request = async (operation, params) => {
    const reply = await send({ type: 'ab-agent-request', operation, params });
    if (!reply?.success) throw new Error(reply?.error || t('panel_teach_host_failed'));
    return reply.result;
  };

  function render() {
    const recording = state === 'recording';
    button.setAttribute('aria-pressed', String(recording));
    button.classList.toggle('active', recording);
    button.disabled = state === 'busy';
    const label = t(recording ? 'panel_teach_stop' : 'panel_teach_start');
    button.title = label;
    button.setAttribute('aria-label', label);
  }

  function showError(message) {
    view.replaceChildren(el('p', message, 'teach-error'));
    view.hidden = false;
  }

  function showReview(result, recording) {
    const name = el('input');
    name.value = result.name || '';
    name.setAttribute('aria-label', t('panel_teach_name'));
    name.placeholder = t('panel_teach_name');
    const steps = el('ol', null, 'teach-steps');
    for (const step of result.steps) steps.append(el('li', step));
    const notes = result.params.map(p => el('p', t(p.kind === 'file' ? 'panel_teach_param_file' : 'panel_teach_param_secret', p.name), 'teach-note'));
    const source = el('textarea', null, 'teach-source');
    source.value = JSON.stringify(result.program, null, 2);
    source.rows = 10;
    source.spellcheck = false;
    source.setAttribute('aria-label', t('panel_teach_source'));
    const details = el('details');
    details.append(el('summary', t('panel_teach_edit')), source);
    const status = el('p', '', 'teach-status');
    status.setAttribute('role', 'status');
    const save = el('button', t('panel_teach_save'));
    save.type = 'button';
    const discard = el('button', t('panel_teach_discard'), 'quiet');
    discard.type = 'button';
    save.addEventListener('click', async () => {
      save.disabled = true;
      try {
        const program = parseProgram(source.value);
        const saved = await request('teachSave', { name: name.value || result.name, program });
        status.textContent = t('panel_teach_saved', saved.path);
        const run = el('code', saved.run, 'teach-run');
        status.append(el('br'), run);
        discard.textContent = t('panel_teach_close');
      } catch (error) {
        status.textContent = String(error.message || error);
        save.disabled = false;
      }
    });
    discard.addEventListener('click', () => { view.hidden = true; view.replaceChildren(); });
    const actions = el('div', null, 'teach-actions');
    actions.append(save, discard);
    view.replaceChildren(el('h3', t('panel_teach_review_title')), name, steps, ...notes, details, actions, status);
    view.hidden = false;
    view.dataset.events = String(recording.events.length);
  }

  async function start() {
    let reply = await send({ type: 'ab-teach-start', tabId: panelTab });
    if (reply?.success && reply.needsPermission) {
      const granted = await Promise.resolve(permissions?.request?.({ origins: [reply.needsPermission] })).catch(() => false);
      if (!granted) throw new Error(t('panel_teach_permission_denied'));
      reply = await send({ type: 'ab-teach-start', tabId: panelTab });
    }
    if (!reply?.success || !reply.started) throw new Error(reply?.error || t('panel_teach_failed'));
    view.hidden = true;
  }

  async function stop() {
    const reply = await send({ type: 'ab-teach-stop', tabId: panelTab });
    if (!reply?.success) throw new Error(reply?.error || t('panel_teach_failed'));
    const result = await request('teachCompile', { name: '', recording: reply.recording });
    showReview(result, reply.recording);
  }

  async function toggle() {
    const was = state;
    state = 'busy';
    render();
    try {
      if (was === 'recording') { await stop(); state = 'idle'; }
      else { await start(); state = 'recording'; }
    } catch (error) {
      state = was === 'recording' ? 'idle' : was;
      showError(String(error.message || error));
    }
    render();
  }

  async function refresh() {
    const tab = await Promise.resolve(tabs?.get?.(panelTab)).catch(() => null);
    button.hidden = !/^https?:/.test(tab?.url || tab?.pendingUrl || '') && state !== 'recording';
    const reply = await send({ type: 'ab-teach-status', tabId: panelTab }).catch(() => null);
    if (reply?.success && state !== 'busy') state = reply.recording ? 'recording' : 'idle';
    render();
  }

  button.addEventListener('click', () => void toggle());
  tabs?.onUpdated?.addListener?.((tabId, change) => { if (tabId === panelTab && (change.url || change.status)) void refresh(); });
  void refresh();
  return { toggle, refresh, state: () => state };
}
