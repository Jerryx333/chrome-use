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
    case 'noOcs': return { tone: 'bad', locked: false, text: 'ocs is not installed. Install it with the command above, then send again.' };
    case 'failed': return { tone: 'bad', locked: false, text: `Delivery failed${result?.error ? `: ${result.error}` : ''}. Nothing was stored.` };
    default: return { tone: 'warn', locked: true, text: 'Unrecognized delivery result. Not resent.' };
  }
}

export function newDeliveryId() {
  return crypto.randomUUID();
}
