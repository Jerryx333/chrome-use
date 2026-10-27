// The side panel's Record button (teach by demonstration). While the user
// records, teach-recorder.js runs in the panel's tab as a content script (the
// extension's isolated world, top frame only): the page cannot see it. It is
// injected again after each navigation of that tab and uninstalled on stop.
// The recording goes to the native host to compile (crate::teach), the same
// compiler `chrome-use teach stop` uses.
//
// Kept free of `chrome.*` globals (they are passed in) so it is unit-testable.

export const teachKey = tabId => `ab_teach_${tabId}`;
const MAX_EVENTS = 5000;

// webNavigation transition types that are the user going somewhere by hand.
const TYPED = new Set(['typed', 'auto_bookmark', 'generated', 'keyword', 'keyword_generated', 'start_page', 'auto_toplevel']);

/** The recording-side shape of a main-frame navigation. */
export function navEvent(details, now) {
  const reason = details.transitionType === 'reload' ? 'reload' : TYPED.has(details.transitionType) ? 'typed' : '';
  return { t: 'nav', url: details.url, reason, ts: now };
}

/** Keep only the fields the compiler reads from a recorder event. */
export function cleanEvent(event, now) {
  if (!event || typeof event !== 'object' || typeof event.t !== 'string') return null;
  if (!['click', 'input', 'select', 'check', 'key', 'upload', 'scroll'].includes(event.t)) return null;
  const out = { ...event, ts: Number.isFinite(event.ts) ? event.ts : now };
  // A secret is never recorded, whatever a recorder sent.
  if (out.secret) delete out.value;
  return out;
}

/**
 * Blank the query values of secret fields in recorded URLs (a GET form puts
 * the password in the next page's URL). Same rule as crate::teach.
 */
export function redactRecording(recording) {
  const names = new Set(recording.events.filter(e => e.secret && e.field).map(e => e.field));
  if (!names.size) return recording;
  for (const e of recording.events) {
    if (typeof e.url !== 'string') continue;
    try {
      const u = new URL(e.url);
      let hit = false;
      for (const k of [...u.searchParams.keys()]) if (names.has(k)) { u.searchParams.set(k, '__redacted__'); hit = true; }
      if (hit) e.url = u.toString();
    } catch {}
  }
  return recording;
}

export class TeachController {
  constructor({ chrome, now = () => Date.now() }) {
    this.chrome = chrome;
    this.now = now;
    this.active = new Map();
  }

  /** Listen for the recorded tab's navigations and closing; restore state after a worker restart. */
  install() {
    const c = this.chrome;
    c.webNavigation.onCommitted.addListener(details => this.onCommitted(details));
    c.webNavigation.onHistoryStateUpdated.addListener(details => this.onSameDocument(details));
    c.webNavigation.onDOMContentLoaded.addListener(details => void this.onLoaded(details));
    c.tabs.onRemoved.addListener(tabId => this.onRemoved(tabId));
    void this.restore();
  }

  isRecording(tabId) {
    return this.active.has(tabId);
  }

  async restore() {
    const all = await Promise.resolve(this.chrome.storage.session?.get(null)).catch(() => ({})) || {};
    for (const [key, value] of Object.entries(all)) {
      const m = /^ab_teach_(\d+)$/.exec(key);
      if (m && value?.startUrl) this.active.set(Number(m[1]), value);
    }
  }

  async save(tabId) {
    const rec = this.active.get(tabId);
    if (rec) await this.chrome.storage.session?.set({ [teachKey(tabId)]: rec }).catch(() => {});
  }

  async start(tabId) {
    if (this.active.has(tabId)) return { started: true, already: true };
    const tab = await this.chrome.tabs.get(tabId);
    const url = tab?.url || '';
    if (!/^https?:/.test(url)) throw new Error('Recording works on web pages (http or https) only');
    this.active.set(tabId, { startUrl: url, events: [], startedAt: this.now() });
    try {
      await this.inject(tabId);
    } catch (error) {
      this.active.delete(tabId);
      const message = String(error?.message || error);
      if (/permission|Cannot access/i.test(message)) return { needsPermission: `${new URL(url).origin}/*` };
      throw error;
    }
    await this.save(tabId);
    return { started: true };
  }

  async inject(tabId) {
    const scripting = this.chrome.scripting;
    await scripting.executeScript({ target: { tabId, frameIds: [0] }, files: ['teach-recorder.js'] });
    const [result] = await scripting.executeScript({
      target: { tabId, frameIds: [0] },
      func: () => {
        const r = globalThis.__cuTeachRecorder;
        return !!(r && r.install(event => { chrome.runtime.sendMessage({ type: 'teach:event', event }).catch(() => {}); }) && r.active());
      },
    });
    if (result?.result !== true) throw new Error('The recorder did not start in this page');
  }

  /** A recorder event from a content script. Only the recorded tab's top frame counts. */
  onEvent(message, sender) {
    if (sender?.id !== this.chrome.runtime.id || !sender.tab || sender.frameId !== 0) return false;
    const rec = this.active.get(sender.tab.id);
    if (!rec) return false;
    const event = cleanEvent(message.event, this.now());
    if (!event || rec.events.length >= MAX_EVENTS) return false;
    rec.events.push(event);
    void this.save(sender.tab.id);
    return true;
  }

  onCommitted(details) {
    if (details.frameId !== 0) return;
    const rec = this.active.get(details.tabId);
    if (!rec) return;
    rec.events.push(navEvent(details, this.now()));
    void this.save(details.tabId);
  }

  onSameDocument(details) {
    if (details.frameId !== 0) return;
    const rec = this.active.get(details.tabId);
    if (!rec) return;
    rec.events.push({ t: 'nav', url: details.url, sameDocument: true, ts: this.now() });
    void this.save(details.tabId);
  }

  /** A new document in the recorded tab: start the recorder there too. */
  async onLoaded(details) {
    if (details.frameId !== 0 || !this.active.has(details.tabId)) return;
    try { await this.inject(details.tabId); } catch {}
  }

  onRemoved(tabId) {
    if (!this.active.delete(tabId)) return;
    void this.chrome.storage.session?.remove(teachKey(tabId)).catch(() => {});
  }

  /** Stop and hand back `{startUrl, events}`; the recorder leaves the page. */
  async stop(tabId) {
    const rec = this.active.get(tabId);
    if (!rec) throw new Error('Nothing is being recorded in this tab');
    this.active.delete(tabId);
    await this.chrome.storage.session?.remove(teachKey(tabId)).catch(() => {});
    try {
      await this.chrome.scripting.executeScript({
        target: { tabId, frameIds: [0] },
        func: () => { const r = globalThis.__cuTeachRecorder; return r ? r.uninstall() : true; },
      });
    } catch {}
    return { recording: redactRecording({ startUrl: rec.startUrl, events: rec.events }) };
  }
}
