// See content.js. Every event is logged to storage for the driver.
// The tab with the menu, kept in session storage: a worker restart between
// two events must not forget it (Bitwarden keeps its worker alive).
let menuTab = null;
const menuTabReady = chrome.storage.session.get('menuTab').then((o) => { menuTab = o.menuTab ?? null; });
const setMenuTab = (tab) => { menuTab = tab; chrome.storage.session.set({ menuTab: tab }); };
// One read-modify-write at a time: overlapping events would drop entries.
let logChain = Promise.resolve();
const log = (entry) => {
  const at = Date.now();
  logChain = logChain.then(async () => {
    const { events = [] } = await chrome.storage.local.get('events');
    events.push({ at, ...entry });
    await chrome.storage.local.set({ events });
  }).catch(() => {});
  return logChain;
};
// For the driver: clear the log after every queued write.
globalThis.resetEvents = () => (logChain = logChain.then(() => chrome.storage.local.set({ events: [] })));
// For the driver: the log once every queued write landed.
globalThis.readEvents = () => logChain.then(() => chrome.storage.local.get('events'));
const hide = (tab, why) => {
  log({ kind: 'close', why, tab });
  chrome.tabs.sendMessage(tab, { cmd: 'hide' }, { frameId: 0 }).catch(() => {});
  if (menuTab === tab) setMenuTab(null);
};
chrome.runtime.onMessage.addListener(async (m, sender) => {
  await menuTabReady;
  const tab = sender.tab?.id;
  if (tab == null) return;
  if (m.cmd === 'open') {
    setMenuTab(tab);
    log({ kind: 'open', tab, frame: sender.frameId });
    chrome.tabs.sendMessage(tab, { cmd: 'show' }, { frameId: 0 }).catch(() => {});
  } else {
    hide(tab, m.cmd);
  }
});
// Bitwarden's OverlayBackground.handleOverlayCiphersUpdate: on a tab activated,
// replaced (a discard) or a window focus change (TabsBackground), the menu of a
// tab that is not in front of the current window closes.
async function tabsChanged(why) {
  await menuTabReady;
  const [current] = await chrome.tabs.query({ active: true, windowId: chrome.windows.WINDOW_ID_CURRENT });
  log({ kind: 'tabs-changed', why, current: current?.id ?? null, menuTab });
  if (menuTab != null && current?.id !== menuTab) hide(menuTab, 'elsewhere');
}
chrome.tabs.onActivated.addListener((info) => {
  log({ kind: 'activated', tab: info.tabId, window: info.windowId });
  tabsChanged('activated');
});
chrome.tabs.onReplaced.addListener((added, removed) => {
  log({ kind: 'replaced', added, removed });
  tabsChanged('replaced');
});
chrome.windows.onFocusChanged.addListener((w) => {
  log({ kind: 'window-focus', window: w });
  if (w > 0) tabsChanged('focus');
});
