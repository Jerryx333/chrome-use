// Pure helpers for the Inspect & Ask input frame (unit-tested in Node).

export function visibilityVerdict(reply) {
  if (!reply || reply.timeout) return { ok: false, message: 'Could not confirm the panel is visible. Try again.' };
  if (reply.isVisible === undefined) return { ok: false, message: 'This browser cannot confirm the panel is visible, so sending is off. Use Copy instead.' };
  if (reply.documentVisibility !== 'visible') return { ok: false, message: 'The page is hidden. Bring it to the front and try again.' };
  if (reply.isVisible !== true || !(reply.ratio >= 0.99)) return { ok: false, message: 'The panel is covered, faded or moved. Scroll, or press the shortcut again.' };
  return { ok: true };
}

function joinRoot(root, path) {
  const separator = root.includes('\\') && !root.includes('/') ? '\\' : '/';
  return root.replace(/[\\/]+$/, '') + separator + path.replace(/^[\\/]+/, '');
}

// Editor deep link for a source inside the bound project root.
export function editorUrl(editor, root, source) {
  if (!editor || editor === 'none' || !root || !source?.path) return null;
  if (source.path.split(/[\\/]/).includes('..')) return null;
  let relativePath = source.path;
  if (/^([A-Za-z]:|[\\/])/.test(relativePath)) {
    // Absolute marks (code-inspector-plugin 2.x emits them) open only when they
    // are inside the bound project root.
    const base = root.replace(/[\\/]+$/, '');
    const normalized = relativePath.replace(/\\/g, '/');
    if (!normalized.startsWith(base.replace(/\\/g, '/') + '/')) return null;
    relativePath = normalized.slice(base.length + 1);
  }
  const file = joinRoot(root, relativePath);
  const line = source.line || 1;
  const column = source.column || 1;
  const encoded = encodeURI(file.replace(/\\/g, '/'));
  switch (editor) {
    case 'vscode': return `vscode://file${file.startsWith('/') ? '' : '/'}${encoded}:${line}:${column}`;
    case 'cursor': return `cursor://file${file.startsWith('/') ? '' : '/'}${encoded}:${line}:${column}`;
    case 'windsurf': return `windsurf://file${file.startsWith('/') ? '' : '/'}${encoded}:${line}:${column}`;
    case 'webstorm': return `webstorm://open?file=${encodeURIComponent(file)}&line=${line}&column=${column}`;
    default: return null;
  }
}

export const LOCKED_STATES = ['sending', 'sent', 'stored', 'unknown', 'unconfirmed'];

// What the panel says after a delivery attempt. stored and wake are separate
// facts from ocs; "stored" never means the agent has read or acted on it.
export function deliveryMessage(result) {
  const state = result?.state;
  const wake = result?.wake ? ` (wake: ${result.wake})` : '';
  switch (state) {
    case 'sent': return { tone: 'ok', locked: true, text: `Sent to the session${wake}. It is in the agent's queue; watch the agent for its answer.${result?.warning ? ' Note: ' + result.warning : ''}` };
    case 'stored': return { tone: 'warn', locked: true, text: `Stored in ocs, but the session was not woken${wake}. Do not resend; the agent sees it when it next reads its inbox.` };
    case 'unknown': return { tone: 'warn', locked: true, text: `Outcome unknown: ocs may have stored it${result?.warning ? ` (${result.warning})` : ''}. Not resent. Check status before starting a new batch.` };
    case 'unconfirmed': return { tone: 'warn', locked: true, text: `No confirmation from the native host${result?.error ? ` (${result.error})` : ''}. Not resent. Check status once chrome-use is connected.` };
    case 'noBinding': return { tone: 'bad', locked: false, text: 'Pick a project for this site first.' };
    case 'noSession': return { tone: 'bad', locked: false, text: 'No running agent session for this project. Start one, then press Check again.' };
    case 'sessionChanged': return { tone: 'bad', locked: false, text: 'The bound session is gone or moved to another folder. Pick the session again.' };
    case 'upgradeRequired': return { tone: 'bad', locked: false, text: 'This site has path-specific targets or a pending claim that this chrome-use version cannot apply. Nothing was sent. Upgrade with `chrome-use upgrade`.' };
    case 'targetChanged': return { tone: 'bad', locked: false, text: result?.reason === 'pageChanged'
      ? 'The page moved to another path since you reviewed it. Nothing was sent. Check the target and the selected elements, then send again.'
      : 'The target changed since the panel showed it (another session took over, it was unbound, or the dev server moved). Nothing was sent. Check the target, then send again.' };
    case 'noOcs': return { tone: 'bad', locked: false, text: 'ocs is not installed. Install it with the command above, then send again.' };
    case 'failed': return { tone: 'bad', locked: false, text: `Delivery failed${result?.error ? `: ${result.error}` : ''}. Nothing was stored.` };
    default: return { tone: 'warn', locked: true, text: 'Unrecognized delivery result. Not resent.' };
  }
}

export function newDeliveryId() {
  return crypto.randomUUID();
}

// ---- matching (inspectResolve) labels -------------------------------------

export function scopeLabel(origin, pathScope) {
  let host = origin;
  try { host = new URL(origin).host; } catch {}
  return host + (pathScope && pathScope !== '/' ? pathScope : '');
}

const EVIDENCE = {
  config: 'project config',
  'config-wildcard': 'project config (wildcard)',
  'dev-server-port': 'dev server',
  'previous-binding': 'used before',
  'ocs-session': 'agent session folder',
};

export function evidenceLabels(pair) {
  const out = (pair?.evidence || []).map(e => EVIDENCE[e] || e);
  if (pair?.matchedPaths) out.push(`${pair.matchedPaths} source file${pair.matchedPaths === 1 ? '' : 's'} matched`);
  return out;
}

// Page revision is a hint; the relation is reported as git sees it locally.
export function revisionText(revision) {
  switch (revision?.relation) {
    case 'same': return 'deployed build is your local HEAD';
    case 'ahead': return `local is ${revision.ahead} commit${revision.ahead === 1 ? '' : 's'} ahead of the deployed build`;
    case 'behind': return `local is ${revision.behind} commit${revision.behind === 1 ? '' : 's'} behind the deployed build`;
    case 'diverged': return `local and deployed build diverged (${revision.ahead} ahead, ${revision.behind} behind)`;
    case 'unknown': return 'deployed build not found in this checkout';
    default: return null;
  }
}

export function reasonText(reason, binding) {
  const who = binding?.sessionLabel || binding?.sessionId || 'The session';
  switch (reason) {
    case 'sessionEnded': return `${who} is no longer running. Choose where requests go now.`;
    case 'sessionMoved': return `${who} now works in another folder. Choose where requests go now.`;
    case 'portMoved': return 'The dev server on this port now runs from another folder. Choose where requests go now.';
    case 'rootMissing': return 'The bound project folder is gone or moved. Choose where requests go now.';
    default: return null;
  }
}

export function targetLabel(t) {
  return `${t?.sessionLabel || t?.sessionId || 'session'} @ ${t?.root || '?'}`;
}

// A literal argument for the user's shell. Page URLs are page-controlled, so a
// copied command must never let `$(…)`, backticks, `;` or quotes run anything.
export function shellQuote(value, platform) {
  const text = String(value ?? '');
  if (platform === 'windows') return "'" + text.replace(/['\u2018\u2019\u201a\u201b]/g, m => m + m) + "'";
  return "'" + text.replace(/'/g, "'\\''") + "'";
}

export function linkCommand(url, platform) {
  return `chrome-use inspect link ${shellQuote(url, platform)}`;
}

// Display only: the home folder as ~ so paths fit the panel. The full path
// stays in the element's title.
export function shortPath(path) {
  return String(path ?? '').replace(/^(?:\/Users\/[^/]+|\/home\/[^/]+|[A-Za-z]:\\Users\\[^\\]+)(?=$|[\\/])/, '~');
}

// Long source locations keep their end (file name and line), which is the
// part that tells elements apart; leading folders give way to "…/".
export function tailPath(text, max = 42) {
  const value = String(text ?? '');
  if (value.length <= max) return value;
  const parts = value.split('/');
  while (parts.length > 1 && ('…/' + parts.join('/')).length > max) parts.shift();
  const tail = '…/' + parts.join('/');
  return tail.length <= max ? tail : '…' + value.slice(-(max - 1));
}
