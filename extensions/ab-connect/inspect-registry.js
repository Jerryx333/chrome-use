// Inspect & Ask picker instances. An instance is the only thing that can read
// captured page context or ask the native host to deliver it, so everything
// here checks Chrome-supplied sender facts (tab, frame, document) plus a
// one-time secret, never values a page could have written into the DOM.
//
// Handshake (Chrome has no runtime.getFrameId(element)): the picker content
// script in frame 0 fetches the secret once, hands it to the iframe it created
// over a MessageChannel the page cannot observe, and the iframe presents it on
// its runtime port. A page that copies the frame never learns the secret. See
// docs/superpowers/plans/inspect-poc/ for the build-host PoC.

export const INSPECT_FRAME_PATH = '/inspect-frame.html';
export const INSPECT_PORT_NAME = 'inspect-frame';
const IDLE_MS = 10 * 60 * 1000;

export function inspectOrigin(url) {
  try {
    const parsed = new URL(url);
    if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') return null;
    return parsed.origin;
  } catch {
    return null;
  }
}

export function isLocalOrigin(origin) {
  try {
    const host = new URL(origin).hostname;
    return host === 'localhost' || host === '127.0.0.1' || host === '[::1]' || host.endsWith('.localhost');
  } catch {
    return false;
  }
}

export class InspectRegistry {
  constructor({ runtimeId, now = () => Date.now(), randomId = () => crypto.randomUUID(), idleMs = IDLE_MS, onRevoke = () => {} } = {}) {
    this.runtimeId = runtimeId;
    this.onRevoke = onRevoke;
    this.now = now;
    this.randomId = randomId;
    this.idleMs = idleMs;
    this.byTab = new Map();
  }

  // Called only from a trusted user gesture (keyboard command or popup click).
  // `mode` 'direct': started from the side panel's Pick button; picks go
  // straight into that panel's composer and no input frame is shown.
  create({ tabId, windowId, documentId, url, mode = 'frame', keep = false }) {
    const origin = inspectOrigin(url);
    if (!Number.isInteger(tabId) || typeof documentId !== 'string' || !documentId || !origin) return null;
    this.revokeTab(tabId, 'replaced');
    const instance = {
      id: this.randomId(), secret: this.randomId(), secretHanded: false, used: false,
      tabId, windowId, topDocumentId: documentId, origin, pageUrl: url,
      createdAt: this.now(), lastActive: this.now(), generation: 0,
      frameId: null, frameDocumentId: null, port: null, revoked: false, revokedReason: null,
      annotations: [], images: new Map(), mode: mode === 'direct' ? 'direct' : 'frame', keep: keep === true,
    };
    this.byTab.set(tabId, instance);
    return instance;
  }

  live(instance) {
    if (!instance || instance.revoked) return null;
    if (this.now() - instance.lastActive > this.idleMs) {
      this.revoke(instance, 'idle');
      return null;
    }
    return instance;
  }

  // The picker in frame 0 of the exact document asks once for the secret.
  handSecret(sender) {
    const instance = this.forPickerSender(sender);
    if (!instance || instance.secretHanded) return null;
    instance.secretHanded = true;
    return { instanceId: instance.id, secret: instance.secret };
  }

  forPickerSender(sender, instanceId) {
    if (sender?.id !== this.runtimeId) return null;
    const instance = this.live(this.byTab.get(sender.tab?.id));
    if (!instance) return null;
    if (instanceId !== undefined && instance.id !== instanceId) return null;
    if (sender.frameId !== 0 || sender.documentId !== instance.topDocumentId) return null;
    instance.lastActive = this.now();
    return instance;
  }

  // First message on an inspect-frame port. Wrong secrets are refused without
  // consuming the instance, so a page flooding fake inits cannot lock it out.
  // First message on an inspect-frame port. The secret is checked first, so
  // a flood of wrong secrets never delays the real frame; wrong attempts only
  // count toward a cap after which further ports are refused outright.
  authenticatePort(port, secret) {
    const sender = port?.sender;
    if (port?.name !== INSPECT_PORT_NAME || sender?.id !== this.runtimeId) return null;
    let path;
    try { path = new URL(sender.url).pathname; } catch { return null; }
    if (path !== INSPECT_FRAME_PATH) return null;
    if (!Number.isInteger(sender.frameId) || sender.frameId === 0 || typeof sender.documentId !== 'string') return null;
    const instance = this.live(this.byTab.get(sender.tab?.id));
    if (!instance || instance.used || !instance.secretHanded || !instance.secret) return null;
    if (typeof secret === 'string' && secret.length === instance.secret.length && secret === instance.secret) {
      instance.used = true;
      instance.secret = null;
      instance.port = port;
      instance.frameId = sender.frameId;
      instance.frameDocumentId = sender.documentId;
      instance.lastActive = this.now();
      return instance;
    }
    instance.failedAuth = (instance.failedAuth || 0) + 1;
    return null;
  }

  forPort(port) {
    const instance = this.live(this.byTab.get(port?.sender?.tab?.id));
    if (!instance || instance.port !== port) return null;
    if (port.sender.frameId !== instance.frameId || port.sender.documentId !== instance.frameDocumentId) return null;
    instance.lastActive = this.now();
    return instance;
  }

  bump(tabId) {
    const instance = this.byTab.get(tabId);
    if (instance) instance.generation++;
  }

  bumpWindow(windowId) {
    for (const instance of this.byTab.values()) if (instance.windowId === windowId) instance.generation++;
  }

  revoke(instance, reason) {
    if (!instance || instance.revoked) return;
    instance.revoked = true;
    instance.revokedReason = reason;
    instance.secret = null;
    instance.annotations = [];
    instance.images.clear();
    instance.generation++;
    if (this.byTab.get(instance.tabId) === instance) this.byTab.delete(instance.tabId);
    const port = instance.port;
    instance.port = null;
    if (port) {
      try { port.postMessage({ type: 'revoked', reason }); } catch {}
      try { port.disconnect(); } catch {}
    }
    try { this.onRevoke(instance, reason); } catch {}
  }

  revokeTab(tabId, reason) {
    this.revoke(this.byTab.get(tabId), reason);
  }

  // Top-frame navigation replaces the document; the old instance is dead.
  revokeDocument(tabId, documentId, reason = 'navigated') {
    const instance = this.byTab.get(tabId);
    if (instance && instance.topDocumentId !== documentId) this.revoke(instance, reason);
  }

  get(tabId) {
    return this.live(this.byTab.get(tabId));
  }
}
