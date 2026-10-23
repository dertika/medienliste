// Service Worker der Medienliste.
// Persönliche Seiten werden bewusst NICHT gecacht (Datenschutz, Aktualität);
// nur statische Dateien werden für schnelleres Laden vorgehalten.
const CACHE = "medienliste-static-v2";

self.addEventListener("install", () => self.skipWaiting());

self.addEventListener("activate", (event) => {
  event.waitUntil(
    caches.keys().then((keys) =>
      Promise.all(keys.filter((k) => k !== CACHE).map((k) => caches.delete(k)))
    ).then(() => self.clients.claim())
  );
});

self.addEventListener("fetch", (event) => {
  const req = event.request;
  if (req.method !== "GET") return;
  const url = new URL(req.url);
  if (url.origin !== self.location.origin || !url.pathname.includes("/static/")) return;
  event.respondWith(
    caches.open(CACHE).then(async (cache) => {
      const hit = await cache.match(req);
      if (hit) return hit;
      const resp = await fetch(req);
      if (resp.ok) {
        // Ältere Fassungen derselben Datei (anderes ?v=…) entfernen.
        for (const old of await cache.keys()) {
          const u = new URL(old.url);
          if (u.pathname === url.pathname && u.search !== url.search) await cache.delete(old);
        }
        cache.put(req, resp.clone());
      }
      return resp;
    })
  );
});
