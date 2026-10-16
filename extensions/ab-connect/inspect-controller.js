// Inspect & Ask background controller: starts pickers from trusted gestures,
// authenticates the input frame, runs the framework probe and screenshot
// capture, and relays the frame's requests to the native host. The page never
// talks to this module; the picker content script and the extension frame do,
// and both are checked against the instance registry on every message.

import { t } from './i18n.js';
import { InspectRegistry, INSPECT_PORT_NAME, isLocalOrigin } from './inspect-registry.js';
import { inspectProbe, validateProbeResult } from './inspect-probe.js';
import { captureVerdict, cropScreenshots, shrinkForStorage } from './inspect-capture.js';
import { chooseSource, formatAnnotation, sourceLabel, NOTE_MAX, INSTRUCTION_MAX } from './inspect-format.js';
import { inspectChipsKey, inspectPickKey, chipsFromAnnotations, mergeChips, upsertChips, settlePending, markStale, QUESTION_MAX } from './inspect-chips.js';

export const INSPECT_COMMAND = 'inspect-element';
const SETTINGS_KEY = 'inspectSettings';
const DRAFTS_KEY = 'inspectDrafts';
// Where picked elements go, per site: the tab's side-panel chat (default) or
// a terminal session over ocs. Only sites that chose the terminal are kept.
const DESTINATIONS_KEY = 'inspectDestinations';
const MAX_DESTINATIONS = 200;
const MAX_ELEMENTS = 8;
const DRAFT_TTL_MS = 7 * 24 * 60 * 60 * 1000;
const MAX_IMAGE_BYTES = 4 * 1024 * 1024;
const CAPTURE_GAP_MS = 600;
const EDITORS = ['vscode', 'cursor', 'windsurf', 'webstorm', 'none'];

export const DEFAULT_INSPECT_SETTINGS = Object.freeze({ enabled: false, editor: 'vscode' });

export function normalizeSettings(value) {
  const editor = EDITORS.includes(value?.editor) ? value.editor : DEFAULT_INSPECT_SETTINGS.editor;
  return { enabled: value?.enabled === true, editor };
}

function sanitizedPageUrl(url) {
  try {
    const parsed = new URL(url);
    return parsed.origin + parsed.pathname;
  } catch {
    return '';
  }
}

function originPort(origin) {
  try {
    const url = new URL(origin);
    return url.port ? Number(url.port) : url.protocol === 'https:' ? 443 : 80;
  } catch {
    return null;
  }
}

function pngBase64(dataUrl) {
  if (typeof dataUrl !== 'string' || !dataUrl.startsWith('data:image/png;base64,')) return null;
  const base64 = dataUrl.slice('data:image/png;base64,'.length);
  if (!/^[A-Za-z0-9+/]+=*$/.test(base64) || base64.length * 0.75 > MAX_IMAGE_BYTES) return null;
  return base64;
}

// Pure: what the host receives for one annotation.
export function annotationPayload(annotation, number) {
  const source = chooseSource(annotation.element.source, annotation.probe);
  return {
    note: annotation.note ? String(annotation.note).slice(0, NOTE_MAX) : undefined,
    label: annotation.element.label,
    sourcePath: source?.path,
    sourceLine: source?.line ?? undefined,
    sourceColumn: source?.column ?? undefined,
    sourceKind: source ? source.kind : 'none',
    component: annotation.probe?.component || undefined,
    ancestors: annotation.probe?.ancestors?.length ? annotation.probe.ancestors : undefined,
    selector: annotation.element.selector,
    markdown: formatAnnotation(annotation, number).slice(0, 20000),
  };
}

const UNCERTAIN = ['sending', 'unknown', 'unconfirmed'];
export function pruneDrafts(drafts, now) {
  const kept = [];
  for (const [origin, draft] of Object.entries(drafts || {})) {
    const old = now - (draft?.updatedAt || 0) >= DRAFT_TTL_MS;
    if (UNCERTAIN.includes(draft?.state)) kept.push([origin, old ? { deliveryId: draft.deliveryId, state: draft.state, updatedAt: draft.updatedAt } : draft]);
    else if (!old) kept.push([origin, draft]);
  }
  kept.sort((a, b) => (UNCERTAIN.includes(b[1].state) - UNCERTAIN.includes(a[1].state)) || (b[1].updatedAt || 0) - (a[1].updatedAt || 0));
  return Object.fromEntries(kept.slice(0, Math.max(30, kept.filter(([, d]) => UNCERTAIN.includes(d.state)).length)));
}

// Pure: a target pair as the frame quotes it from inspectResolve. Only the
// fields the host re-validates travel on; the host recomputes the fingerprint.
export function validPair(pair) {
  if (!pair || typeof pair !== 'object') return null;
  const str = (v, max = 4096) => typeof v === 'string' && v.length > 0 && v.length <= max;
  if (!str(pair.root) || !str(pair.sessionId, 200) || !str(pair.sessionCwd) || !str(pair.fingerprint, 128) || !str(pair.pathScope, 2000) || !pair.pathScope.startsWith('/')) return null;
  if (!Array.isArray(pair.sourceRoots) || !pair.sourceRoots.length || pair.sourceRoots.length > 16 || !pair.sourceRoots.every(r => str(r))) return null;
  return { root: pair.root, sourceRoots: [...pair.sourceRoots], sessionId: pair.sessionId, sessionCwd: pair.sessionCwd, sessionLabel: str(pair.sessionLabel, 200) ? pair.sessionLabel : null, pathScope: pair.pathScope, fingerprint: pair.fingerprint };
}

export function normalizeDestination(value) {
  return value === 'terminal' ? 'terminal' : 'panel';
}

export function tabTargetPayload(target) {
  return { root: target.root, sourceRoots: target.sourceRoots, sessionId: target.sessionId, sessionCwd: target.sessionCwd, fingerprint: target.fingerprint };
}

// Pure: which delivery states may never be sent again under the same id.
export function deliveryLocked(state) {
  return ['sending', 'sent', 'stored', 'unknown', 'unconfirmed'].includes(state);
}

export class InspectController {
  constructor({ chrome, request, hostReady, resolveReady = () => false, repliesReady = () => false }) {
    this.chrome = chrome;
    this.request = request;
    this.hostReady = hostReady;
    // Hosts from v1.5.214 resolve path-scoped bindings, claims and tab
    // targets; older hosts only know one binding per origin.
    this.resolveReady = resolveReady;
    // Hosts that send as a reply-safe identity and keep agent replies.
    this.repliesReady = repliesReady;
    this.registry = new InspectRegistry({ runtimeId: chrome.runtime.id, onRevoke: instance => { void this.directEnded(instance); } });
  }

  install() {
    const c = this.chrome;
    c.commands?.onCommand.addListener((command, tab) => {
      if (command === INSPECT_COMMAND) void this.start(tab).catch(error => console.warn('Inspect start failed:', String(error.message || error)));
    });
    c.runtime.onConnect.addListener(port => {
      if (port.name === INSPECT_PORT_NAME) this.acceptPort(port);
    });
    c.tabs.onRemoved.addListener(tabId => { this.registry.revokeTab(tabId, 'closed'); void c.storage.session?.remove(inspectChipsKey(tabId)).catch(() => {}); });
    c.tabs.onActivated.addListener(({ tabId, windowId }) => { this.registry.bumpWindow(windowId); this.registry.bump(tabId); });
    c.tabs.onZoomChange?.addListener(({ tabId }) => this.registry.bump(tabId));
    c.windows?.onRemoved?.addListener(windowId => this.registry.bumpWindow(windowId));
    // pushState / replaceState / popstate in a single-page app: same document,
    // new path. The tab-only target belonged to the old path; the panel must
    // resolve again and the user re-checks what is selected.
    c.webNavigation.onHistoryStateUpdated?.addListener(details => {
      if (details.frameId !== 0) return;
      void this.staleChips(details.tabId, { pageUrl: details.url });
      const instance = this.registry.get(details.tabId);
      if (!instance) return;
      instance.tabTarget = null;
      instance.port?.postMessage({ type: 'pageChanged', url: (() => { try { const u = new URL(details.url); return u.origin + u.pathname; } catch { return null; } })() });
    });
    c.webNavigation.onCommitted.addListener(details => {
      if (details.frameId !== 0) return;
      this.registry.bump(details.tabId);
      if (details.documentId) this.registry.revokeDocument(details.tabId, details.documentId);
      void this.staleChips(details.tabId, { documentId: details.documentId || null, pageUrl: details.url });
    });
  }

  // Chips waiting in a tab's panel stay after the page moves on, marked
  // stale: the agent is told they may no longer be there.
  async staleChips(tabId, where) {
    const session = this.chrome.storage.session;
    if (!session) return;
    try {
      const key = inspectChipsKey(tabId);
      const next = markStale((await session.get(key))?.[key], where);
      if (next) await session.set({ [key]: next });
    } catch {}
  }

  async destination(origin) {
    const stored = (await this.chrome.storage.local.get(DESTINATIONS_KEY))?.[DESTINATIONS_KEY] || {};
    return normalizeDestination(stored[origin]);
  }

  async saveDestination(origin, value) {
    const stored = { ...((await this.chrome.storage.local.get(DESTINATIONS_KEY))?.[DESTINATIONS_KEY] || {}) };
    delete stored[origin];
    if (normalizeDestination(value) === 'terminal') stored[origin] = 'terminal';
    const kept = Object.fromEntries(Object.entries(stored).slice(-MAX_DESTINATIONS));
    await this.chrome.storage.local.set({ [DESTINATIONS_KEY]: kept });
    return normalizeDestination(value);
  }

  async settings() {
    const stored = await this.chrome.storage.local.get(SETTINGS_KEY);
    return normalizeSettings(stored?.[SETTINGS_KEY]);
  }

  async saveSettings(patch) {
    const next = normalizeSettings({ ...(await this.settings()), ...patch });
    await this.chrome.storage.local.set({ [SETTINGS_KEY]: next });
    return next;
  }

  // Trusted entry points only: keyboard command and the popup button.
  async start(tab) {
    const c = this.chrome;
    if (!tab?.id) [tab] = await c.tabs.query({ active: true, currentWindow: true });
    if (!tab?.id || !/^https?:/.test(tab.url || '')) throw new Error(t('inspect_error_http_only'));
    const frame = await c.webNavigation.getFrame({ tabId: tab.id, frameId: 0 });
    if (!frame?.documentId) throw new Error(t('inspect_error_no_document'));
    const instance = this.registry.create({ tabId: tab.id, windowId: tab.windowId, documentId: frame.documentId, url: tab.url });
    if (!instance) throw new Error(t('inspect_error_cannot_inspect'));
    await c.scripting.executeScript({ target: { tabId: tab.id, frameIds: [0] }, files: ['inspect-extract.js', 'inspect-picker.js'] });
    return { started: true };
  }

  // The side panel's Pick button: a picker on the panel's own tab whose picks
  // go straight into that panel's composer. The click in the panel grants no
  // activeTab, so a site the extension may not script yet answers with the
  // host pattern the panel asks the user for, then the panel tries again.
  async startDirect(tabId, { keep = false } = {}) {
    const c = this.chrome;
    const tab = await c.tabs.get(tabId);
    if (!/^https?:/.test(tab?.url || '')) throw new Error(t('inspect_error_http_only'));
    const frame = await c.webNavigation.getFrame({ tabId, frameId: 0 });
    if (!frame?.documentId) throw new Error(t('inspect_error_no_document'));
    const instance = this.registry.create({ tabId, windowId: tab.windowId, documentId: frame.documentId, url: tab.url, mode: 'direct', keep });
    if (!instance) throw new Error(t('inspect_error_cannot_inspect'));
    instance.chipIds = new Map();
    instance.known = new Set();
    instance.directImages = new Map();
    instance.imagesSettled = new Set();
    try {
      await c.scripting.executeScript({ target: { tabId, frameIds: [0] }, files: ['inspect-extract.js', 'inspect-picker.js'] });
    } catch (error) {
      this.registry.revoke(instance, 'failed');
      const message = String(error?.message || error);
      if (/permission|Cannot access/i.test(message)) return { needsPermission: `${instance.origin}/*` };
      throw error;
    }
    await c.storage.session?.set({ [inspectPickKey(tabId)]: { instanceId: instance.id, keep: instance.keep } });
    return { started: true };
  }

  // The Pick button again, or Esc in the panel.
  async cancelDirect(tabId) {
    const instance = this.registry.get(tabId);
    if (instance?.mode === 'direct') {
      void this.chrome.tabs.sendMessage(tabId, { type: 'inspect:cancel', instanceId: instance.id }, { frameId: 0 }).catch(() => {});
      this.registry.revoke(instance, 'cancelled');
    } else {
      await this.chrome.storage.session?.remove(inspectPickKey(tabId)).catch(() => {});
    }
    return { cancelled: true };
  }

  // A direct picker ended (pick done, Esc, cancel, navigation): the panel's
  // button leaves its active state and placeholders stop waiting.
  async directEnded(instance) {
    if (instance.mode !== 'direct') return;
    const session = this.chrome.storage.session;
    if (!session) return;
    try {
      const pickKey = inspectPickKey(instance.tabId);
      if ((await session.get(pickKey))?.[pickKey]?.instanceId === instance.id) await session.remove(pickKey);
      const key = inspectChipsKey(instance.tabId);
      const next = settlePending((await session.get(key))?.[key], [...(instance.chipIds?.values() || [])]);
      if (next) await session.set({ [key]: next });
    } catch {}
  }

  // Direct picks become chips at once (element facts the picker already
  // sent); the framework probe and the screenshot update them in place.
  async directChips(instance, elements, probes) {
    const session = this.chrome.storage.session;
    if (!session) return;
    const keyOf = e => JSON.stringify([e.selector, e.frames || [], e.shadowHosts || []]);
    const ids = elements.map(e => {
      const k = keyOf(e);
      if (!instance.chipIds.has(k)) instance.chipIds.set(k, crypto.randomUUID());
      return instance.chipIds.get(k);
    });
    const annotations = elements.map((element, i) => ({ element, probe: probes?.[i] || null, note: '' }));
    const chips = chipsFromAnnotations(annotations, { documentId: instance.topDocumentId, pageUrl: (await this.pageNow(instance)).pageUrl, ids }).map((chip, i) => ({
      ...chip,
      image: instance.directImages.get(chip.id) || null,
      pending: { image: !instance.directImages.has(chip.id) && !instance.imagesSettled.has(chip.id), source: !probes && !elements[i].source },
    }));
    const key = inspectChipsKey(instance.tabId);
    const record = upsertChips((await session.get(key))?.[key], chips, instance.known);
    for (const id of ids) instance.known.add(id);
    await session.set({ [key]: record });
  }

  // After a capture for a direct picker: screenshots go into their chips;
  // chips the capture could not cover stop waiting for one. The last pick of
  // a one-pick session then lets the picker leave.
  async directCaptured(instance, selection, images) {
    if (instance.revoked || instance.selectionRevision !== selection) return;
    const keys = instance.annotations.map(a => JSON.stringify([a.element.selector, a.element.frames || [], a.element.shadowHosts || []]));
    for (const [i, k] of keys.entries()) {
      const id = instance.chipIds.get(k);
      if (!id) continue;
      if (images?.[i]) { try { instance.directImages.set(id, await shrinkForStorage(images[i])); } catch {} }
      instance.imagesSettled.add(id);
    }
    if (instance.revoked || instance.selectionRevision !== selection) return;
    await this.directChips(instance, instance.annotations.map(a => a.element), instance.annotations.map(a => a.probe));
    if (instance.final) void this.chrome.tabs.sendMessage(instance.tabId, { type: 'inspect:finish', instanceId: instance.id }, { frameId: 0, documentId: instance.topDocumentId }).catch(() => this.registry.revoke(instance, 'done'));
  }

  // Messages from the picker content script (frame 0 of the instance document).
  onPickerMessage(message, sender, sendResponse) {
    const type = message?.type;
    if (type === 'inspect:hello') {
      const handed = this.registry.handSecret(sender);
      if (!handed) { sendResponse({ ok: false }); return false; }
      // The panel needs no setup: only the terminal destination asks to be
      // enabled first, so only then does the input frame open at once.
      const instance = this.registry.forPickerSender(sender, handed.instanceId);
      void Promise.all([this.settings(), this.destination(instance?.origin)]).then(([settings, destination]) => sendResponse({
        ok: true, ...handed, frameUrl: this.chrome.runtime.getURL('inspect-frame.html'),
        frameOrigin: `chrome-extension://${this.chrome.runtime.id}`, enabled: settings.enabled || destination === 'panel' || instance?.mode === 'direct',
        mode: instance?.mode || 'frame', keep: !!instance?.keep,
      }));
      return true;
    }
    const instance = this.registry.forPickerSender(sender, message?.instanceId);
    if (!instance) { sendResponse({ ok: false, error: 'Inspect session ended' }); return false; }
    if (type === 'inspect:selection') {
      void this.updateSelection(instance, message).then(sendResponse, error => sendResponse({ ok: false, error: String(error.message || error) }));
      return true;
    }
    if (type === 'inspect:closed') {
      this.registry.revoke(instance, 'closed');
      sendResponse({ ok: true });
      return false;
    }
    sendResponse({ ok: false, error: 'Unknown inspect message' });
    return false;
  }

  // Every selection gets a new revision. The probe, screenshots, notes and the
  // send are bound to it; a result that arrives after the selection changed is
  // dropped instead of being attached to different elements.
  async updateSelection(instance, { elements, probeAttribute, layout, revision, selectionSeq, final }) {
    if (!Array.isArray(elements) || elements.length > MAX_ELEMENTS) throw new Error('Too many elements');
    const selection = (instance.selectionRevision || 0) + 1;
    instance.selectionRevision = selection;
    const direct = instance.mode === 'direct';
    if (direct) {
      instance.final = final === true;
      // The chip comes first; nothing slow happens before it is written.
      try { await this.directChips(instance, elements, null); } catch {}
    }
    let probes = elements.map(() => null);
    if (typeof probeAttribute === 'string' && /^data-[a-z]{12}$/.test(probeAttribute) && elements.length) {
      try {
        const [result] = await this.chrome.scripting.executeScript({
          target: { tabId: instance.tabId, frameIds: [0] }, world: 'MAIN', func: inspectProbe, args: [probeAttribute],
        });
        probes = validateProbeResult(result?.result, elements.length);
      } catch {}
    }
    if (instance.revoked || instance.selectionRevision !== selection) return { ok: true, stale: true };
    const previous = instance.annotations;
    instance.pickerSeq = Number.isInteger(selectionSeq) ? selectionSeq : null;
    instance.annotations = elements.map((element, i) => ({ id: `${selection}:${i}`, element, probe: probes[i], note: previous.find(a => a.element.selector === element.selector)?.note || '' }));
    instance.revision = typeof revision === 'string' && /^[A-Za-z0-9._-]{1,100}$/.test(revision) ? revision : null;
    instance.images = new Map();
    this.pushAnnotations(instance);
    if (direct) { try { await this.directChips(instance, elements, probes); } catch {} }
    if (elements.length) this.queueCapture(instance);
    return { ok: true };
  }

  pushAnnotations(instance) {
    instance.port?.postMessage({
      type: 'annotations',
      revision: instance.selectionRevision || 0,
      annotations: instance.annotations.map((a, i) => ({
        id: a.id, number: i + 1, label: a.element.label, component: a.probe?.component || null,
        source: sourceLabel(chooseSource(a.element.source, a.probe)), sourceKind: chooseSource(a.element.source, a.probe)?.kind || 'none',
        selector: a.element.selector, note: a.note,
      })),
      pageRevision: instance.revision,
    });
  }

  // Captures run one at a time so two never hide and restore the picker UI
  // over each other; each is bound to the selection revision it was made for.
  queueCapture(instance) {
    const selection = instance.selectionRevision;
    instance.captureChain = (instance.captureChain || Promise.resolve()).then(() => this.capture(instance, selection)).catch(() => {})
      .then(() => (instance.mode === 'direct' ? this.directCaptured(instance, selection, instance.lastImages?.selection === selection ? instance.lastImages.images : null) : null)).catch(() => {});
    return instance.captureChain;
  }

  // One snapshot of the page's current URL (single-page apps move without a
  // new document): pageUrl and pagePath always come from the same read.
  async pageNow(instance) {
    let url = null;
    try {
      const tab = await this.chrome.tabs.get(instance.tabId);
      const parsed = new URL(tab.url);
      if (parsed.origin === instance.origin) url = parsed;
    } catch {}
    if (!url) { try { url = new URL(instance.pageUrl); } catch { url = new URL(instance.origin); } }
    return { pageUrl: url.origin + (url.pathname || '/'), pagePath: url.pathname || '/' };
  }

  async pagePath(instance) {
    return (await this.pageNow(instance)).pagePath;
  }

  // Bounded: a frozen or hidden page must not stall the capture queue.
  async pickerCall(instance, message, timeoutMs = 3000) {
    let timer;
    const timeout = new Promise((_, reject) => { timer = setTimeout(() => reject(new Error(t('inspect_capture_page_no_answer'))), timeoutMs); });
    try {
      return await Promise.race([this.chrome.tabs.sendMessage(instance.tabId, { ...message, instanceId: instance.id }, { frameId: 0, documentId: instance.topDocumentId }), timeout]);
    } finally {
      clearTimeout(timer);
    }
  }

  async capture(instance, selection = instance.selectionRevision) {
    const c = this.chrome;
    const stale = () => instance.revoked || instance.selectionRevision !== selection;
    const fail = reason => { if (!stale()) instance.port?.postMessage({ type: 'images', revision: selection, images: instance.annotations.map(() => null), reason }); };
    if (stale()) return;
    // Chrome allows two visible-tab captures a second; quick picks would
    // otherwise end in a quota error instead of a screenshot.
    const wait = (this.lastCaptureAt || 0) + CAPTURE_GAP_MS - Date.now();
    if (wait > 0) await new Promise(resolve => setTimeout(resolve, wait));
    if (stale()) return;
    try {
      const generationBefore = instance.generation;
      const before = await c.tabs.get(instance.tabId);
      // A background tab cannot be captured, and its page does not run
      // animation frames, so ask nothing of the picker in that case.
      if (!before.active) return fail(t('inspect_capture_needs_visible_tab'));
      const prepared = await this.pickerCall(instance, { type: 'inspect:prepareCapture' });
      if (!prepared?.ok) return fail(t('inspect_capture_prepare_failed'));
      // The picker's selection may already be ahead of this revision.
      if (stale() || prepared.layout?.selectionSeq !== instance.pickerSeq || prepared.layout?.rects?.length !== instance.annotations.length) { await this.pickerCall(instance, { type: 'inspect:finishCapture' }).catch(() => null); return; }
      let dataUrl;
      let after;
      let layoutAfter;
      let documentAfter;
      try {
        if (!before.active) return fail(t('inspect_capture_needs_visible_tab'));
        this.lastCaptureAt = Date.now();
        dataUrl = await c.tabs.captureVisibleTab(before.windowId, { format: 'png' });
        after = await c.tabs.get(instance.tabId);
        documentAfter = (await c.webNavigation.getFrame({ tabId: instance.tabId, frameId: 0 }))?.documentId;
      } finally {
        const finished = await this.pickerCall(instance, { type: 'inspect:finishCapture' }).catch(() => null);
        layoutAfter = finished?.layout;
      }
      const verdict = captureVerdict({
        before, after, generationBefore, generationAfter: instance.generation,
        layoutBefore: prepared.layout, layoutAfter, topDocumentId: instance.topDocumentId, documentAfter,
      });
      if (!verdict.ok) return fail(verdict.reason);
      if (stale()) return;
      const cropped = await cropScreenshots(dataUrl, prepared.layout);
      if (stale()) return;
      if (!cropped.ok) return fail(cropped.reason);
      instance.images = new Map();
      cropped.images.forEach((image, i) => { if (image) instance.images.set(i, image); });
      instance.lastImages = { selection, images: cropped.images };
      instance.port?.postMessage({ type: 'images', revision: selection, images: cropped.images, reason: cropped.images.some(x => !x) ? t('inspect_capture_outside_view') : null });
    } catch (error) {
      fail(t('inspect_capture_unavailable', String(error.message || error)));
    }
  }

  acceptPort(port) {
    let instance = null;
    const timer = setTimeout(() => { if (!instance) { try { port.disconnect(); } catch {} } }, 5000);
    port.onMessage.addListener(message => {
      if (!instance) {
        if (message?.type !== 'auth') return;
        const result = this.registry.authenticatePort(port, message.secret);
        if (!result) { port.postMessage({ type: 'denied' }); try { port.disconnect(); } catch {} clearTimeout(timer); return; }
        instance = result;
        clearTimeout(timer);
        void this.onFrameAuthorized(instance);
        return;
      }
      if (this.registry.forPort(port) !== instance) { try { port.disconnect(); } catch {} return; }
      void this.onFrameMessage(instance, message).catch(error => port.postMessage({ type: 'error', requestId: message?.requestId, error: String(error.message || error) }));
    });
    port.onDisconnect.addListener(() => {
      clearTimeout(timer);
      if (instance && instance.port === port) this.registry.revoke(instance, 'frame closed');
    });
  }

  async onFrameAuthorized(instance) {
    const settings = await this.settings();
    const drafts = (await this.chrome.storage.local.get(DRAFTS_KEY))?.[DRAFTS_KEY] || {};
    // Drafts expire after 7 days, except an unconfirmed batch, which stays
    // until its outcome is checked.
    const pruned = pruneDrafts(drafts, Date.now());
    if (Object.keys(pruned).length !== Object.keys(drafts).length || JSON.stringify(pruned) !== JSON.stringify(drafts)) await this.chrome.storage.local.set({ [DRAFTS_KEY]: pruned });
    const draft = pruned[instance.origin];
    const fresh = !!draft;
    instance.port?.postMessage({
      type: 'authorized', origin: instance.origin, pageUrl: sanitizedPageUrl(instance.pageUrl),
      env: isLocalOrigin(instance.origin) ? 'local' : 'test', settings,
      draft: fresh ? draft : null, hostConnected: this.hostReady(),
      destination: await this.destination(instance.origin), tabId: instance.tabId,
    });
    this.pushAnnotations(instance);
  }

  reply(instance, requestId, payload) {
    instance.port?.postMessage({ type: 'reply', requestId, ...payload });
  }

  async host(operation, params) {
    if (!this.hostReady()) throw new Error(t('inspect_error_host_offline'));
    return this.request(operation, params);
  }

  // Drafts hold only your words. Editing drafts are deleted 7 days after their
  // last change; an unconfirmed batch keeps just its id and state (text and
  // notes dropped) until its outcome is checked. Uncertain batches are never
  // the ones cut when the 30-site cap applies.
  async saveDraft(origin, draft) {
    const stored = (await this.chrome.storage.local.get(DRAFTS_KEY))?.[DRAFTS_KEY] || {};
    if (draft) stored[origin] = { ...draft, updatedAt: Date.now() };
    else delete stored[origin];
    await this.chrome.storage.local.set({ [DRAFTS_KEY]: pruneDrafts(stored, Date.now()) });
  }

  async onFrameMessage(instance, message) {
    const { type, requestId } = message || {};
    const answer = payload => this.reply(instance, requestId, payload);
    switch (type) {
      case 'enable': {
        if (message.trusted !== true) throw new Error(t('inspect_error_enable_needs_click'));
        return answer({ settings: await this.saveSettings({ enabled: true }) });
      }
      case 'status': {
        // Page-supplied paths and revision only rank candidates; a target is
        // made by the user's click, a session's own link, or local config.
        const marks = instance.annotations.map(a => chooseSource(a.element.source, a.probe)).filter(s => s?.path && s.path.length <= 300 && !/[\u0000-\u001f]/.test(s.path)).map(s => s.path);
        const params = { origin: instance.origin, sourcePaths: [...new Set(marks)].slice(0, 16) };
        if (isLocalOrigin(instance.origin)) params.port = originPort(instance.origin);
        if (!this.resolveReady()) return answer({ status: await this.host('inspectStatus', params), legacy: true });
        params.pagePath = await this.pagePath(instance);
        if (instance.revision && /^[0-9a-f]{7,40}$/.test(instance.revision)) params.revision = instance.revision;
        return answer({ status: await this.host('inspectResolve', params), tabTarget: instance.tabTarget || null, pagePath: params.pagePath, pageUrl: (await this.pageNow(instance)).pageUrl });
      }
      case 'confirm': {
        if (!this.resolveReady()) throw new Error(t('inspect_error_cli_too_old'));
        const pair = validPair(message.pair);
        if (!pair) throw new Error('Unknown target');
        if (message.mode === 'tab') {
          // Held by this picker only; the host re-validates it on every send.
          instance.tabTarget = pair;
          return answer({ tabTarget: pair });
        }
        instance.tabTarget = null;
        return answer({ result: await this.host('inspectBind', { origin: instance.origin, pathScope: pair.pathScope, root: pair.root, sourceRoots: pair.sourceRoots, sessionId: pair.sessionId, sessionLabel: pair.sessionLabel || undefined, fingerprint: pair.fingerprint }) });
      }
      case 'clearTabTarget':
        instance.tabTarget = null;
        return answer({ ok: true });
      case 'claimDecide': {
        if (typeof message.claimId !== 'string' || !Array.isArray(message.expectedAffects)) throw new Error('Unknown claim');
        return answer({ result: await this.host('inspectClaimDecide', { claimId: message.claimId, expectedAffects: message.expectedAffects.filter(f => typeof f === 'string').slice(0, 64), accept: message.accept === true }) });
      }
      case 'bind':
        return answer({ result: await this.host('inspectBind', { origin: instance.origin, root: String(message.root || ''), sessionId: message.sessionId || undefined, sessionLabel: message.sessionLabel || undefined }) });
      case 'consent':
        if (!['plugin', 'testEnv'].includes(message.kind)) throw new Error('Unknown consent');
        return answer({ result: await this.host('inspectConsent', { origin: instance.origin, ...(this.resolveReady() && typeof message.pathScope === 'string' ? { pathScope: message.pathScope } : {}), kind: message.kind, granted: message.granted === true }) });
      case 'launchCommand':
        return answer({ result: await this.host('inspectLaunchCommand', { origin: instance.origin, ...(this.resolveReady() && typeof message.pathScope === 'string' ? { pathScope: message.pathScope } : {}), agent: message.agent === 'codex' ? 'codex' : 'claude' }) });
      case 'note': {
        const annotation = instance.annotations.find(a => a.id === message.annotationId);
        if (annotation) annotation.note = String(message.note || '').slice(0, NOTE_MAX);
        return answer({ ok: true });
      }
      case 'draft': {
        const draft = message.draft;
        const notes = {};
        for (const [selector, note] of Object.entries(draft?.notes || {}).slice(0, MAX_ELEMENTS)) {
          if (typeof selector === 'string' && selector.length <= 300 && typeof note === 'string' && note.trim()) notes[selector] = note.slice(0, NOTE_MAX);
        }
        const deliveryId = typeof draft?.deliveryId === 'string' && /^[0-9a-f-]{36}$/.test(draft.deliveryId) ? draft.deliveryId : null;
        // The frame edits text; only deliver() moves a batch into a sent or
        // uncertain state, and a locked batch keeps the state deliver() set.
        const stored = (await this.chrome.storage.local.get(DRAFTS_KEY))?.[DRAFTS_KEY]?.[instance.origin];
        const keep = stored && deliveryId && stored.deliveryId === deliveryId && deliveryLocked(stored.state);
        await this.saveDraft(instance.origin, draft ? { instruction: String(draft.instruction || '').slice(0, INSTRUCTION_MAX), notes, deliveryId: keep ? deliveryId : null, state: keep ? stored.state : 'editing' } : null);
        return answer({ ok: true });
      }
      case 'recapture':
        if (!instance.annotations.length) throw new Error(t('inspect_error_nothing_selected'));
        await this.queueCapture(instance);
        return answer({ ok: true });
      case 'destination':
        return answer({ destination: await this.saveDestination(instance.origin, message.destination) });
      case 'toPanel':
        return answer({ result: await this.toPanel(instance, message) });
      case 'copyText':
        return answer({ revision: instance.selectionRevision || 0, annotations: instance.annotations.map((a, i) => formatAnnotation(a, i + 1)) });
      case 'deliver':
        return answer({ result: await this.deliver(instance, message) });
      case 'replies': {
        // `replies: null` tells the frame this host keeps none.
        if (!this.repliesReady()) return answer({ replies: null });
        const deliveryId = String(message.deliveryId || '');
        if (!/^[0-9a-f-]{36}$/.test(deliveryId)) throw new Error('Missing delivery id');
        return answer({ replies: (await this.host('inspectReplies', { deliveryId })).replies });
      }
      case 'deliveryStatus':
        return answer({ result: await this.host('inspectDeliveryStatus', { deliveryId: String(message.deliveryId || '') }) });
      case 'setup': {
        const kind = message.kind === 'setupTestEnv' ? 'setupTestEnv' : 'setupPlugin';
        // Setup always goes to the site default; a tab-only target never receives it.
        return answer({ result: await this.deliver(instance, { ...message, kind, instruction: '', revision: instance.selectionRevision || 0 }) });
      }
      default:
        throw new Error('Unknown inspect frame request');
    }
  }

  // The frame opened the tab's panel from the user's click; this puts the
  // reviewed selection into that panel's composer. Nothing is sent.
  async toPanel(instance, message) {
    const session = this.chrome.storage.session;
    if (!session) throw new Error(t('inspect_panel_unavailable'));
    if (message.revision !== (instance.selectionRevision || 0)) throw new Error(t('inspect_error_selection_changed_sending'));
    if (!instance.annotations.length) throw new Error(t('inspect_error_select_first'));
    const notes = {}, images = {};
    instance.annotations.forEach((a, i) => { if (typeof message.notes?.[a.id] === 'string') notes[i] = message.notes[a.id]; });
    for (const item of Array.isArray(message.images) ? message.images.slice(0, MAX_ELEMENTS) : []) {
      if (Number.isInteger(item?.index) && instance.annotations[item.index] && typeof item.dataUrl === 'string') images[item.index] = item.dataUrl;
    }
    const chips = chipsFromAnnotations(instance.annotations, { documentId: instance.topDocumentId, pageUrl: (await this.pageNow(instance)).pageUrl, notes, images });
    const key = inspectChipsKey(instance.tabId);
    const record = mergeChips((await session.get(key))?.[key], chips, String(message.question || '').slice(0, QUESTION_MAX));
    await session.set({ [key]: record });
    return { added: chips.length, waiting: record.items.length };
  }

  async deliver(instance, message) {
    const deliveryId = String(message.deliveryId || '');
    if (!/^[0-9a-f-]{36}$/.test(deliveryId)) throw new Error('Missing delivery id');
    const kind = message.kind || 'edit';
    // The frame names the selection it reviewed; a change since then (an
    // element added or removed while the send was being verified) is refused
    // rather than mixed into this batch. The batch is frozen from here on.
    if (kind === 'edit' && message.revision !== (instance.selectionRevision || 0)) throw new Error(t('inspect_error_selection_changed_sending'));
    // Deep copy before the first await: later note edits or selection
    // changes cannot alter what this batch sends. Notes come from the frame's
    // own snapshot, keyed by the annotation ids of the reviewed revision.
    const annotations = instance.annotations.map(a => structuredClone({ id: a.id, element: a.element, probe: a.probe, note: a.note }));
    if (message.notes && typeof message.notes === 'object') {
      for (const a of annotations) if (typeof message.notes[a.id] === 'string') a.note = message.notes[a.id].slice(0, NOTE_MAX);
    }
    const pageRevision = instance.revision;
    const instruction = String(message.instruction || '').slice(0, INSTRUCTION_MAX);
    if (kind === 'edit' && !annotations.length) throw new Error(t('inspect_error_select_first'));
    if (kind === 'edit' && !instruction.trim() && !annotations.some(a => a.note)) throw new Error(t('inspect_error_write_first'));
    const drafts = (await this.chrome.storage.local.get(DRAFTS_KEY))?.[DRAFTS_KEY] || {};
    const prior = drafts[instance.origin];
    if (prior?.deliveryId === deliveryId && deliveryLocked(prior.state)) {
      throw new Error(t('inspect_error_already_sent'));
    }
    const include = Array.isArray(message.images) ? message.images : [];
    const images = [];
    for (const item of include.slice(0, MAX_ELEMENTS)) {
      if (!Number.isInteger(item?.index) || !annotations[item.index]) continue;
      const base64 = pngBase64(item.dataUrl);
      if (base64) images.push({ index: item.index, pngBase64: base64 });
    }
    // Checked before anything is recorded as sending, so a refusal here
    // leaves the batch editable. The panel names the page path and the target it showed; if either moved
    // since (route change, claim accepted elsewhere, rebinding), nothing is sent.
    const page = await this.pageNow(instance);
    let scoped = {};
    if (this.resolveReady()) {
      if (typeof message.expectedPath !== 'string' || message.expectedPath !== page.pagePath) {
        return { state: 'targetChanged', reason: 'pageChanged' };
      }
      if (typeof message.expectedTarget !== 'string' || !message.expectedTarget) return { state: 'targetChanged', reason: 'bindingChanged' };
      scoped = { pagePath: page.pagePath, expectedTarget: message.expectedTarget, ...(instance.tabTarget && kind === 'edit' ? { target: tabTargetPayload(instance.tabTarget) } : {}) };
    }
    const notes = Object.fromEntries(annotations.filter(a => a.note).map(a => [a.element.selector, a.note]));
    await this.saveDraft(instance.origin, { instruction, notes, deliveryId, state: 'sending' });
    let result;
    try {
      result = await this.host('inspectDeliver', {
        ...scoped,
        deliveryId, origin: instance.origin, kind,
        instruction: kind === 'edit' ? instruction : undefined,
        pageUrl: page.pageUrl, env: isLocalOrigin(instance.origin) ? 'local' : 'test',
        revision: pageRevision || undefined,
        annotations: kind === 'edit' ? annotations.map((a, i) => annotationPayload(a, i + 1)) : [],
        images: kind === 'edit' ? images : [],
      });
    } catch (error) {
      await this.saveDraft(instance.origin, { instruction, notes, deliveryId, state: 'unconfirmed' });
      return { state: 'unconfirmed', deliveryId, error: String(error.message || error) };
    }
    const state = ['noBinding', 'noSession', 'sessionChanged', 'noOcs', 'failed', 'upgradeRequired', 'targetChanged'].includes(result?.state) ? 'editing' : result?.state || 'unconfirmed';
    await this.saveDraft(instance.origin, { instruction, notes, deliveryId, state });
    return result;
  }
}
