// Inspect & Ask background controller: starts pickers from trusted gestures,
// authenticates the input frame, runs the framework probe and screenshot
// capture, and relays the frame's requests to the native host. The page never
// talks to this module; the picker content script and the extension frame do,
// and both are checked against the instance registry on every message.

import { InspectRegistry, INSPECT_PORT_NAME, isLocalOrigin } from './inspect-registry.js';
import { inspectProbe, validateProbeResult } from './inspect-probe.js';
import { captureVerdict, cropScreenshots } from './inspect-capture.js';
import { chooseSource, formatAnnotation, sourceLabel, NOTE_MAX, INSTRUCTION_MAX } from './inspect-format.js';

export const INSPECT_COMMAND = 'inspect-element';
const SETTINGS_KEY = 'inspectSettings';
const DRAFTS_KEY = 'inspectDrafts';
const MAX_ELEMENTS = 8;
const MAX_IMAGE_BYTES = 4 * 1024 * 1024;
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

// Pure: which delivery states may never be sent again under the same id.
export function deliveryLocked(state) {
  return ['sending', 'sent', 'stored', 'unknown', 'unconfirmed'].includes(state);
}

export class InspectController {
  constructor({ chrome, request, hostReady }) {
    this.chrome = chrome;
    this.request = request;
    this.hostReady = hostReady;
    this.registry = new InspectRegistry({ runtimeId: chrome.runtime.id });
  }

  install() {
    const c = this.chrome;
    c.commands?.onCommand.addListener((command, tab) => {
      if (command === INSPECT_COMMAND) void this.start(tab).catch(error => console.warn('Inspect start failed:', String(error.message || error)));
    });
    c.runtime.onConnect.addListener(port => {
      if (port.name === INSPECT_PORT_NAME) this.acceptPort(port);
    });
    c.tabs.onRemoved.addListener(tabId => this.registry.revokeTab(tabId, 'closed'));
    c.tabs.onActivated.addListener(({ tabId, windowId }) => { this.registry.bumpWindow(windowId); this.registry.bump(tabId); });
    c.tabs.onZoomChange?.addListener(({ tabId }) => this.registry.bump(tabId));
    c.windows?.onRemoved?.addListener(windowId => this.registry.bumpWindow(windowId));
    c.webNavigation.onCommitted.addListener(details => {
      if (details.frameId !== 0) return;
      this.registry.bump(details.tabId);
      if (details.documentId) this.registry.revokeDocument(details.tabId, details.documentId);
    });
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
    if (!tab?.id || !/^https?:/.test(tab.url || '')) throw new Error('Inspect & Ask works on http(s) pages only.');
    const frame = await c.webNavigation.getFrame({ tabId: tab.id, frameId: 0 });
    if (!frame?.documentId) throw new Error('Could not identify the page document.');
    const instance = this.registry.create({ tabId: tab.id, windowId: tab.windowId, documentId: frame.documentId, url: tab.url });
    if (!instance) throw new Error('This page cannot be inspected.');
    await c.scripting.executeScript({ target: { tabId: tab.id, frameIds: [0] }, files: ['inspect-extract.js', 'inspect-picker.js'] });
    return { started: true };
  }

  // Messages from the picker content script (frame 0 of the instance document).
  onPickerMessage(message, sender, sendResponse) {
    const type = message?.type;
    if (type === 'inspect:hello') {
      const handed = this.registry.handSecret(sender);
      if (!handed) { sendResponse({ ok: false }); return false; }
      void this.settings().then(settings => sendResponse({
        ok: true, ...handed, frameUrl: this.chrome.runtime.getURL('inspect-frame.html'),
        frameOrigin: `chrome-extension://${this.chrome.runtime.id}`, enabled: settings.enabled,
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

  async updateSelection(instance, { elements, probeAttribute, layout, revision }) {
    if (!Array.isArray(elements) || elements.length > MAX_ELEMENTS) throw new Error('Too many elements');
    let probes = elements.map(() => null);
    if (typeof probeAttribute === 'string' && /^data-cu-probe-[a-f0-9]{16}$/.test(probeAttribute) && elements.length) {
      try {
        const [result] = await this.chrome.scripting.executeScript({
          target: { tabId: instance.tabId, frameIds: [0] }, world: 'MAIN', func: inspectProbe, args: [probeAttribute],
        });
        probes = validateProbeResult(result?.result, elements.length);
      } catch {}
    }
    const previous = instance.annotations;
    instance.annotations = elements.map((element, i) => ({ element, probe: probes[i], note: previous.find(a => a.element.selector === element.selector)?.note || '' }));
    instance.revision = typeof revision === 'string' && /^[A-Za-z0-9._-]{1,100}$/.test(revision) ? revision : null;
    instance.images = new Map();
    this.pushAnnotations(instance);
    if (elements.length) void this.capture(instance, layout);
    return { ok: true };
  }

  pushAnnotations(instance) {
    instance.port?.postMessage({
      type: 'annotations',
      annotations: instance.annotations.map((a, i) => ({
        number: i + 1, label: a.element.label, component: a.probe?.component || null,
        source: sourceLabel(chooseSource(a.element.source, a.probe)), sourceKind: chooseSource(a.element.source, a.probe)?.kind || 'none',
        selector: a.element.selector, note: a.note,
      })),
      revision: instance.revision,
    });
  }

  async pickerCall(instance, message) {
    return this.chrome.tabs.sendMessage(instance.tabId, { ...message, instanceId: instance.id }, { frameId: 0, documentId: instance.topDocumentId });
  }

  async capture(instance, layoutHint) {
    const c = this.chrome;
    const fail = reason => instance.port?.postMessage({ type: 'images', images: instance.annotations.map(() => null), reason });
    try {
      const generationBefore = instance.generation;
      const before = await c.tabs.get(instance.tabId);
      const prepared = await this.pickerCall(instance, { type: 'inspect:prepareCapture' });
      if (!prepared?.ok) return fail('The page could not prepare a screenshot.');
      let dataUrl;
      let after;
      let layoutAfter;
      let documentAfter;
      try {
        if (!before.active) return fail('Screenshots need the page to be the visible tab.');
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
      const cropped = await cropScreenshots(dataUrl, prepared.layout);
      if (!cropped.ok) return fail(cropped.reason);
      if (instance.revoked) return;
      cropped.images.forEach((image, i) => { if (image) instance.images.set(i, image); });
      instance.port?.postMessage({ type: 'images', images: cropped.images, reason: cropped.images.some(x => !x) ? 'Some elements were outside the visible area.' : null });
    } catch (error) {
      fail(`Screenshot unavailable: ${String(error.message || error)}`);
    }
  }

  acceptPort(port) {
    let instance = null;
    const timer = setTimeout(() => { if (!instance) { try { port.disconnect(); } catch {} } }, 5000);
    port.onMessage.addListener(message => {
      if (!instance) {
        if (message?.type !== 'auth') return;
        const result = this.registry.authenticatePort(port, message.secret);
        if (result === 'busy') { port.postMessage({ type: 'busy' }); return; }
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
    instance.port?.postMessage({
      type: 'authorized', origin: instance.origin, pageUrl: sanitizedPageUrl(instance.pageUrl),
      env: isLocalOrigin(instance.origin) ? 'local' : 'test', settings,
      draft: drafts[instance.origin] || null, hostConnected: this.hostReady(),
    });
    this.pushAnnotations(instance);
  }

  reply(instance, requestId, payload) {
    instance.port?.postMessage({ type: 'reply', requestId, ...payload });
  }

  async host(operation, params) {
    if (!this.hostReady()) throw new Error('The chrome-use native host is not connected. Run `chrome-use doctor` or reinstall chrome-use, then press the shortcut again.');
    return this.request(operation, params);
  }

  async saveDraft(origin, draft) {
    const stored = (await this.chrome.storage.local.get(DRAFTS_KEY))?.[DRAFTS_KEY] || {};
    if (draft) stored[origin] = { ...draft, updatedAt: Date.now() };
    else delete stored[origin];
    const entries = Object.entries(stored).sort((a, b) => (b[1].updatedAt || 0) - (a[1].updatedAt || 0)).slice(0, 30);
    await this.chrome.storage.local.set({ [DRAFTS_KEY]: Object.fromEntries(entries) });
  }

  async onFrameMessage(instance, message) {
    const { type, requestId } = message || {};
    const answer = payload => this.reply(instance, requestId, payload);
    switch (type) {
      case 'enable': {
        if (message.trusted !== true) throw new Error('Enabling needs a click in the panel.');
        return answer({ settings: await this.saveSettings({ enabled: true }) });
      }
      case 'status': {
        // Page-supplied paths only rank candidates; the user still binds explicitly.
        const marks = instance.annotations.map(a => chooseSource(a.element.source, a.probe)).filter(s => s?.path && s.path.length <= 300 && !/[\u0000-\u001f]/.test(s.path)).map(s => s.path);
        const params = { origin: instance.origin, sourcePaths: [...new Set(marks)].slice(0, 16) };
        if (isLocalOrigin(instance.origin)) params.port = originPort(instance.origin);
        return answer({ status: await this.host('inspectStatus', params) });
      }
      case 'bind':
        return answer({ result: await this.host('inspectBind', { origin: instance.origin, root: String(message.root || ''), sessionId: message.sessionId || undefined, sessionLabel: message.sessionLabel || undefined }) });
      case 'consent':
        if (!['plugin', 'testEnv'].includes(message.kind)) throw new Error('Unknown consent');
        return answer({ result: await this.host('inspectConsent', { origin: instance.origin, kind: message.kind, granted: message.granted === true }) });
      case 'launchCommand':
        return answer({ result: await this.host('inspectLaunchCommand', { origin: instance.origin, agent: message.agent === 'codex' ? 'codex' : 'claude' }) });
      case 'note': {
        const annotation = instance.annotations[message.index];
        if (annotation) annotation.note = String(message.note || '').slice(0, NOTE_MAX);
        return answer({ ok: true });
      }
      case 'draft':
        await this.saveDraft(instance.origin, message.draft ? { instruction: String(message.draft.instruction || '').slice(0, INSTRUCTION_MAX), deliveryId: message.draft.deliveryId || null, state: message.draft.state || 'editing' } : null);
        return answer({ ok: true });
      case 'recapture':
        if (!instance.annotations.length) throw new Error('Nothing selected');
        await this.capture(instance, null);
        return answer({ ok: true });
      case 'copyText':
        return answer({ annotations: instance.annotations.map((a, i) => formatAnnotation(a, i + 1)) });
      case 'deliver':
        return answer({ result: await this.deliver(instance, message) });
      case 'deliveryStatus':
        return answer({ result: await this.host('inspectDeliveryStatus', { deliveryId: String(message.deliveryId || '') }) });
      case 'setup': {
        const kind = message.kind === 'setupTestEnv' ? 'setupTestEnv' : 'setupPlugin';
        return answer({ result: await this.deliver(instance, { ...message, kind, instruction: '' }) });
      }
      default:
        throw new Error('Unknown inspect frame request');
    }
  }

  async deliver(instance, message) {
    const deliveryId = String(message.deliveryId || '');
    if (!/^[0-9a-f-]{36}$/.test(deliveryId)) throw new Error('Missing delivery id');
    const kind = message.kind || 'edit';
    const instruction = String(message.instruction || '').slice(0, INSTRUCTION_MAX);
    if (kind === 'edit' && !instance.annotations.length) throw new Error('Select at least one element first.');
    if (kind === 'edit' && !instruction.trim() && !instance.annotations.some(a => a.note)) throw new Error('Write what should change first.');
    const drafts = (await this.chrome.storage.local.get(DRAFTS_KEY))?.[DRAFTS_KEY] || {};
    const prior = drafts[instance.origin];
    if (prior?.deliveryId === deliveryId && deliveryLocked(prior.state)) {
      throw new Error('This batch was already sent or its outcome is unconfirmed. Check its status or start a new batch.');
    }
    const include = Array.isArray(message.images) ? message.images : [];
    const images = [];
    for (const item of include.slice(0, MAX_ELEMENTS)) {
      if (!Number.isInteger(item?.index) || !instance.annotations[item.index]) continue;
      const base64 = pngBase64(item.dataUrl);
      if (base64) images.push({ index: item.index, pngBase64: base64 });
    }
    await this.saveDraft(instance.origin, { instruction, deliveryId, state: 'sending' });
    let result;
    try {
      result = await this.host('inspectDeliver', {
        deliveryId, origin: instance.origin, kind,
        instruction: kind === 'edit' ? instruction : undefined,
        pageUrl: sanitizedPageUrl(instance.pageUrl), env: isLocalOrigin(instance.origin) ? 'local' : 'test',
        revision: instance.revision || undefined,
        annotations: kind === 'edit' ? instance.annotations.map((a, i) => annotationPayload(a, i + 1)) : [],
        images: kind === 'edit' ? images : [],
      });
    } catch (error) {
      await this.saveDraft(instance.origin, { instruction, deliveryId, state: 'unconfirmed' });
      return { state: 'unconfirmed', deliveryId, error: String(error.message || error) };
    }
    const state = ['noBinding', 'noSession', 'sessionChanged', 'noOcs', 'failed'].includes(result?.state) ? 'editing' : result?.state || 'unconfirmed';
    await this.saveDraft(instance.origin, { instruction, deliveryId, state });
    return result;
  }
}
