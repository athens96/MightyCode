// The website's only code: GET /api/latest returns the live update manifest from R2 (app.js reads it to fill the version
// badge and the download links), cached at the edge for a minute. Every other path is a static file (wrangler.toml's
// [assets]), which reaches this worker only when no file matches.
const LATEST = "https://pub-b705901d18c940159d40f58683d758ad.r2.dev/mightycode/latest.json";

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    if (url.pathname === "/api/latest") {
      if (request.method !== "GET" && request.method !== "HEAD") return new Response("method not allowed", { status: 405 });
      const res = await fetch(LATEST, { cf: { cacheTtl: 60, cacheEverything: true } });
      return new Response(request.method === "HEAD" ? null : await res.text(), {
        status: res.status,
        headers: {
          "content-type": "application/json; charset=utf-8",
          "cache-control": "no-cache",
          "access-control-allow-origin": "*",
        },
      });
    }
    return env.ASSETS.fetch(request);
  },
};
