// Shared by the popup and panel so missing-host errors give the same remedy.
export function missingHostHint(connectionError) {
  if (!String(connectionError || '').toLowerCase().includes('host not found')) return null;
  return { messageKey: 'popup_hint_install_host', command: 'chrome-use extension install' };
}
