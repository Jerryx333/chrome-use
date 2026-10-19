// Bitwarden 2026.10's inline menu, reduced to what chrome-use meets: a login
// field that gets focus opens a chrome-extension:// frame in the top document
// (Chrome then refuses debugger commands on the tab), and the menu closes on
// the field's blur, a trusted Escape, the page turning hidden, or (bg.js)
// another tab being activated while this one is not in front.
const isField = (el) => el instanceof HTMLInputElement && ['text', 'email', 'password'].includes(el.type);
const send = (cmd) => chrome.runtime.sendMessage({ cmd }).catch(() => {});
document.addEventListener('focus', (e) => { if (isField(e.target)) send('open'); }, true);
document.addEventListener('blur', (e) => { if (isField(e.target)) send('blurred'); }, true);
document.addEventListener('keyup', (e) => { if (e.isTrusted && e.code === 'Escape') send('escape'); }, true);
document.addEventListener('visibilitychange', () => { if (document.visibilityState === 'hidden') send('hidden'); });
if (window === top) {
  chrome.runtime.onMessage.addListener((m) => {
    const open = document.getElementById('fakepm-menu');
    if (m.cmd === 'show' && !open) {
      const f = document.createElement('iframe');
      f.id = 'fakepm-menu';
      f.src = chrome.runtime.getURL('menu.html');
      f.style = 'position:fixed;top:0;right:0;width:200px;height:80px;z-index:2147483647;border:0';
      (document.body || document.documentElement).append(f);
    }
    if (m.cmd === 'hide' && open) open.remove();
  });
}
