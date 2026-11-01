// MangaVox phone companion service worker. Browsers only run it in a secure context (HTTPS or
// localhost), so over the plain-HTTP Wi-Fi address it is never registered; app.js checks
// `isSecureContext` first.
//
// It keeps the app shell (page, script, styles, icons: no personal data) so the home-screen app
// opens even when the PC is briefly unreachable, and it shows notifications. API responses
// (/api/..., covers and pages included) are never cached here: they go straight to the network.
// The server fills in VERSION from the shell's contents, so an update replaces the cache.
const VERSION = '__MV_VERSION__';
const CACHE = 'mv-shell-' + VERSION;
const SHELL = ['/', '/app.js', '/app.css', '/manifest.webmanifest', '/icon.png', '/icon-512.png', '/logo.png'];
// How long the network gets before the cached shell answers (the PC is on the same Wi-Fi, so it
// normally answers in milliseconds).
const NET_WAIT = 1500;

self.addEventListener('install', e => {
  e.waitUntil((async () => {
    const cache = await caches.open(CACHE);
    for (const url of SHELL) {
      try {
        const res = await fetch(url, { cache: 'no-store', credentials: 'same-origin' });
        // An unpaired browser gets the locked page for "/" instead of the app: never keep that.
        if (!res.ok || res.redirected) continue;
        if (url === '/' && !(await res.clone().text()).includes('/app.js')) continue;
        await cache.put(url, res);
      } catch { /* offline: cached on the next install */ }
    }
    await self.skipWaiting();
  })());
});

self.addEventListener('activate', e => {
  e.waitUntil((async () => {
    for (const k of await caches.keys()) if (k !== CACHE) await caches.delete(k);
    await self.clients.claim();
  })());
});

function shellKey(req) {
  const u = new URL(req.url);
  if (u.origin !== location.origin || req.method !== 'GET') return null;
  if (req.mode === 'navigate') return u.pathname === '/' || u.pathname === '/index.html' ? '/' : null;
  return SHELL.includes(u.pathname) ? u.pathname : null;
}

// Network first (a fresh shell after an update), the cached shell when the PC is slow or away.
self.addEventListener('fetch', e => {
  const key = shellKey(e.request);
  if (!key) return;
  e.respondWith((async () => {
    const cache = await caches.open(CACHE);
    const net = fetch(e.request).then(res => {
      if (res.ok && !res.redirected && key !== '/') cache.put(key, res.clone()).catch(() => {});
      return res;
    });
    const timeout = new Promise(r => setTimeout(r, NET_WAIT));
    const first = await Promise.race([net.catch(() => null), timeout]);
    if (first) return first;
    return (await cache.match(key)) || net;
  })());
});

self.addEventListener('notificationclick', e => {
  e.notification.close();
  const url = (e.notification.data && e.notification.data.url) || '/#/activity';
  e.waitUntil((async () => {
    const wins = await self.clients.matchAll({ type: 'window', includeUncontrolled: true });
    for (const w of wins) {
      if (new URL(w.url).origin === location.origin) {
        await w.focus();
        if ('navigate' in w) await w.navigate(url).catch(() => {});
        return;
      }
    }
    await self.clients.openWindow(url);
  })());
});
