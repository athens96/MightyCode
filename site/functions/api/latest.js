// Cloudflare Pages Function: proxies live latest.json from R2 with CORS headers
export async function onRequestGet() {
  const r2Url = "https://pub-b705901d18c940159d40f58683d758ad.r2.dev/mightycode/latest.json";
  const res = await fetch(r2Url, { cf: { cacheTtl: 60, cacheEverything: true } });
  const body = await res.text();
  return new Response(body, {
    status: res.status,
    headers: {
      "content-type": "application/json; charset=utf-8",
      "cache-control": "no-cache",
      "access-control-allow-origin": "*"
    }
  });
}
