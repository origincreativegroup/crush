// Local-only preview of the shipped UI with synthetic, in-memory bridge fixtures.
// It never opens the user's catalogue and serves only the UI directory + the mock script.
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { basename, extname, join } from "node:path";

const ui = fileURLToPath(new URL("../crates/app/ui/", import.meta.url));
const mock = fileURLToPath(new URL("../crates/app/tests/mock-bridge.js", import.meta.url));
const types = { ".html": "text/html", ".css": "text/css", ".js": "text/javascript" };
const port = Number(process.env.CRUSH_PREVIEW_PORT || 4173);
createServer(async (request, response) => {
  const url = new URL(request.url, "http://127.0.0.1");
  const name = url.pathname === "/" ? "index.html" : url.pathname.slice(1);
  if (basename(name) !== name || !types[extname(name)]) { response.writeHead(404).end(); return; }
  try {
    let body = await readFile(name === "mock-bridge.js" ? mock : join(ui, name), "utf8");
    if (name === "index.html") {
      if (!url.searchParams.has("scenario")) {
        response.writeHead(302, { Location: "/?scenario=plans-editor" }).end(); return;
      }
      body = body.replace("<head>", '<head><script src="mock-bridge.js"></script>')
        .replace("<title>Crush</title>", "<title>Crush UI fixture — synthetic data</title>");
    }
    response.writeHead(200, { "Content-Type": types[extname(name)], "Cache-Control": "no-store" }).end(body);
  } catch { response.writeHead(404).end(); }
}).listen(port, "127.0.0.1", () => console.log(`Synthetic UI preview: http://127.0.0.1:${port}/?scenario=plans-editor`));
