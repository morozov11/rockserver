import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";

const port = Number(process.argv[2] ?? 5174);
const authenticated = process.argv.includes("--authenticated");
const index = await readFile(resolve("dist/index.html"));
const preview = JSON.stringify({ request_id: "00000000-0000-0000-0000-000000000000", device_display_name: "Этот телефон", device_type: "rockmobile_android", verification_phrase: "AMBER FJORD", short_code: "A1B2C3D4", expires_at: "2030-01-01T12:00:00Z", status: "pending" });

/** Serves built UI assets with deterministic, credential-free pairing API responses for browser QA. */
createServer(async (request, response) => {
  if (request.url?.startsWith("/v1/pairing-requests/lookup")) {
    response.writeHead(200, { "content-type": "application/json", "cache-control": "no-store" }); return response.end(preview);
  }
  if (request.url === "/v1/auth/browser-session") {
    if (!authenticated) { response.writeHead(401, { "content-type": "application/json" }); return response.end('{"code":"authentication_required","message":"","request_id":"","details":{}}'); }
    response.writeHead(200, { "content-type": "application/json", "cache-control": "no-store" }); return response.end('{"account_display_name":"Алексей","csrf_token":"test-csrf"}');
  }
  if (request.url?.startsWith("/assets/")) {
    try {
      const asset = await readFile(resolve(`dist${request.url}`));
      response.writeHead(200, { "content-type": request.url.endsWith(".js") ? "text/javascript" : "text/css" });
      return response.end(asset);
    } catch { response.writeHead(404); return response.end(); }
  }
  response.writeHead(200, { "content-type": "text/html" }); response.end(index);
}).listen(port, "127.0.0.1");
