// See content.js. Every event is logged to storage for the driver.
let menuTab = null;
const log = async (entry) => {
  const { events = [] } = await chrome.storage.local.get('events');
  events.push({ at: Date.now(), ...entry });
  await chrome.storage.local.set({ events });
};
const hide = (tab, why) => {
  log({ kind: 'close', why, tab });
  chrome.tabs.sendMessage(tab, { cmd: 'hide' }, { frameId: 0 }).catch(() => {});
  if (menuTab === tab) menuTab = null;
};
chrome.runtime.onMessage.addListener((m, sender) => {
  const tab = sender.tab?.id;
  if (tab == null) return;
  if (m.cmd === 'open') {
    menuTab = tab;
    log({ kind: 'open', tab, frame: sender.frameId });
    chrome.tabs.sendMessage(tab, { cmd: 'show' }, { frameId: 0 }).catch(() => {});
  } else {
    hide(tab, m.cmd);
  }
});
// Bitwarden's OverlayBackground.handleOverlayCiphersUpdate: on a tab or window
// change, the menu of a tab that is not in front of the current window closes.
async function tabsChanged(why) {
  const [current] = await chrome.tabs.query({ active: true, windowId: chrome.windows.WINDOW_ID_CURRENT });
  log({ kind: 'tabs-changed', why, current: current?.id ?? null, menuTab });
  if (menuTab != null && current?.id !== menuTab) hide(menuTab, 'elsewhere');
}
chrome.tabs.onActivated.addListener((info) => {
  log({ kind: 'activated', tab: info.tabId, window: info.windowId });
  tabsChanged('activated');
});
chrome.windows.onFocusChanged.addListener((w) => {
  log({ kind: 'window-focus', window: w });
  if (w > 0) tabsChanged('focus');
});
