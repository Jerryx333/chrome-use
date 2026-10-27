// Pending per-site approval requests (#781), shown in the side panel and on
// the options page with the generic approval card. Reads
// `chrome.storage.session` and answers through the worker
// (`ab-site-permission`), which checks the sender.

import { approvalCard } from './approval-card.js';
import { SITE_REQUESTS_KEY, pruneRequests } from './site-permissions.js';

const REASON_KEYS = {
  sensitive_site: 'panel_site_reason_sensitive_site',
  download: 'panel_site_reason_download',
  payment_input: 'panel_site_reason_payment_input',
  password_input: 'panel_site_reason_password_input',
  purchase: 'panel_site_reason_purchase',
  first_visit: 'panel_site_reason_first_visit',
  confirm_site: 'panel_site_reason_confirm_site',
  managed_site: 'panel_site_reason_managed',
  managed_action: 'panel_site_reason_managed',
};

/** The card spec for one request. */
export function requestSpec(request, t) {
  return {
    title: t('panel_site_approval_title', request.host || '?'),
    detail: request.description || '',
    facts: [
      [t('panel_site_approval_why'), REASON_KEYS[request.reason] ? t(REASON_KEYS[request.reason]) : request.reason],
      [t('panel_site_approval_action'), request.action],
      [t('panel_site_approval_session'), request.session],
    ],
    choices: [
      { id: 'once', label: t('panel_site_allow_once'), primary: true },
      { id: 'always', label: t('panel_site_allow_always') },
      { id: 'decline', label: t('panel_site_decline') },
    ],
  };
}

/**
 * Keep `container` showing the unanswered requests. Returns a function that
 * re-renders (the options page calls it after its own changes).
 */
export function mountSiteApprovals({ doc, container, storage, runtime, t, now = () => Date.now() }) {
  const shown = new Map();
  async function render() {
    let requests = [];
    try { requests = pruneRequests((await storage.session.get(SITE_REQUESTS_KEY))?.[SITE_REQUESTS_KEY], now()).filter(r => !r.decision); } catch {}
    const live = new Set(requests.map(r => r.id));
    for (const [id, node] of shown) if (!live.has(id)) { node.remove(); shown.delete(id); }
    for (const request of requests) {
      if (shown.has(request.id)) continue;
      const card = approvalCard(doc, requestSpec(request, t), async decision => {
        const reply = await runtime.sendMessage({ type: 'ab-site-permission', op: 'decide', id: request.id, decision });
        if (!reply?.success) throw new Error(reply?.error || t('panel_site_decision_failed'));
      });
      card.dataset.requestId = request.id;
      shown.set(request.id, card);
      container.append(card);
    }
    container.hidden = shown.size === 0;
  }
  try {
    storage.onChanged?.addListener?.((changes, area) => { if (area === 'session' && SITE_REQUESTS_KEY in changes) void render(); });
  } catch {}
  void render();
  return render;
}
