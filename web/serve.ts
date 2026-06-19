import { createReadStream } from "node:fs";
import { stat } from "node:fs/promises";
import { join, extname } from "node:path";

const PORT = parseInt(process.env.PORT || "8080", 10);
const WEB_DIR = import.meta.dirname;
const FIXTURES_DIR = join(WEB_DIR, "..", "fixtures");

const MIME = {
  ".html": "text/html; charset=utf-8",
  ".js":   "text/javascript; charset=utf-8",
  ".ts":   "video/mp2t",
  ".tsx":  "text/javascript; charset=utf-8",
  ".css":  "text/css; charset=utf-8",
  ".wasm": "application/wasm",
  ".json": "application/json",
  ".png":  "image/png",
  ".svg":  "image/svg+xml",
};

async function serve(req) {
  const url = new URL(req.url);
  let filePath;

  if (url.pathname.startsWith("/fixtures/")) {
    const rel = url.pathname.slice("/fixtures/".length);
    filePath = join(FIXTURES_DIR, rel);
  } else {
    let rel = url.pathname.slice(1);
    if (rel === "") rel = "index.html";
    filePath = join(WEB_DIR, rel);
  }

  // Prevent directory traversal.
  if (!filePath.startsWith(WEB_DIR) && !filePath.startsWith(FIXTURES_DIR)) {
    return new Response("Not Found", { status: 404 });
  }

  let sb;
  try {
    sb = await stat(filePath);
  } catch {
    return new Response("Not Found", { status: 404 });
  }

  if (!sb.isFile()) {
    return new Response("Not Found", { status: 404 });
  }

  const ext = extname(filePath).toLowerCase();
  const contentType = MIME[ext] || "application/octet-stream";

  // Range request support — required for WebCodecs/browsers seeking into MPEG-TS.
  const rangeHeader = req.headers.get("range");
  if (rangeHeader && sb.size > 0) {
    const match = rangeHeader.match(/bytes=(\d+)-(\d*)/);
    if (match) {
      const start = parseInt(match[1], 10);
      let end = match[2] ? parseInt(match[2], 10) : sb.size - 1;
      if (end >= sb.size) end = sb.size - 1;

      const chunkSize = end - start + 1;
      const stream = createReadStream(filePath, { start, end });

      return new Response(stream, {
        status: 206,
        headers: {
          "Content-Type": contentType,
          "Content-Range": `bytes ${start}-${end}/${sb.size}`,
          "Content-Length": String(chunkSize),
          "Accept-Ranges": "bytes",
        },
      });
    }
  }

  const stream = createReadStream(filePath);
  return new Response(stream, {
    headers: {
      "Content-Type": contentType,
      "Content-Length": String(sb.size),
      "Accept-Ranges": "bytes",
    },
  });
}

const server = Bun.serve({
  port: PORT,
  fetch: serve,
});

console.log(`Skyfire dev server: http://localhost:${PORT}`);
console.log(`  Web root:   ${WEB_DIR}`);
console.log(`  Fixtures:   ${FIXTURES_DIR}`);
