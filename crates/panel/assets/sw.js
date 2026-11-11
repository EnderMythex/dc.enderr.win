// Service worker of the panel. While the panel restarts (a redeploy), the
// proxy in front of it answers with its own error page; this replaces that
// page with "Recovering connection", which reloads once the panel is back.
'use strict';

const CACHE = 'dc-panel-offline-v2';
const OFFLINE = '/offline';

self.addEventListener('install', (event) => {
  event.waitUntil(
    (async () => {
      const cache = await caches.open(CACHE);
      const page = await fetch(OFFLINE, { cache: 'no-store' });
      const html = await page.clone().text();
      await cache.put(OFFLINE, page);
      // The stylesheet and script the offline page needs.
      for (const [, url] of html.matchAll(/(?:href|src)="(\/assets\/[^"]+)"/g)) {
        await cache.add(url);
      }
      await self.skipWaiting();
    })(),
  );
});

self.addEventListener('activate', (event) => {
  event.waitUntil(
    (async () => {
      for (const key of await caches.keys()) {
        if (key !== CACHE) await caches.delete(key);
      }
      await self.clients.claim();
    })(),
  );
});

const fromPanel = (response) => response.headers.has('x-dc-panel');

/// The proxy's own error page while the panel is down: a 404 or a 5xx that
/// does not come from the panel. Anything else, redirects included (they
/// come back opaque, with no status or headers), is passed on untouched.
const isProxyError = (response) =>
  response.type !== 'opaqueredirect'
  && !fromPanel(response)
  && (response.status === 404 || response.status >= 500);

self.addEventListener('fetch', (event) => {
  const { request } = event;
  if (request.method !== 'GET') return;
  const url = new URL(request.url);
  if (url.origin !== self.location.origin) return;

  if (request.mode === 'navigate') {
    event.respondWith(
      (async () => {
        try {
          const response = await fetch(request);
          if (!isProxyError(response)) return response;
        } catch {
          // Network error: the panel is unreachable.
        }
        return (await caches.match(OFFLINE)) ?? Response.error();
      })(),
    );
    return;
  }

  // Assets: network first, the cached copy while the panel is down.
  if (url.pathname.startsWith('/assets/')) {
    event.respondWith(
      (async () => {
        try {
          const response = await fetch(request);
          if (response.ok) return response;
        } catch {
          // Fall back to the cache below.
        }
        return (await caches.match(request)) ?? Response.error();
      })(),
    );
  }
});
