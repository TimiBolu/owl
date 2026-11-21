// A static server for the demo, with nothing to install:
//   node apps/owl-wasm/demo/serve.mjs
// It serves apps/owl-wasm, so the page finds the package built beside it.
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { dirname, extname, join, normalize, sep } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const port = Number(process.env.PORT ?? 4317);
const types = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".mjs": "text/javascript; charset=utf-8",
  ".wasm": "application/wasm",
  ".json": "application/json",
  ".ts": "text/plain; charset=utf-8",
};

createServer(async (request, response) => {
  const path = decodeURIComponent(new URL(request.url, "http://localhost").pathname);
  const file = normalize(join(root, path === "/" ? "demo/index.html" : path.endsWith("/") ? `${path}index.html` : path));
  // Nothing outside the folder served.
  if (file !== root && !file.startsWith(root + sep)) {
    response.writeHead(403).end("forbidden");
    return;
  }
  try {
    const body = await readFile(file);
    response.writeHead(200, { "content-type": types[extname(file)] ?? "application/octet-stream" }).end(body);
  } catch {
    response.writeHead(404).end("not found");
  }
}).listen(port, "127.0.0.1", () => console.log(`Owl in the browser: http://127.0.0.1:${port}/demo/`));
