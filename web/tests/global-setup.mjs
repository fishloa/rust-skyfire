import { spawn } from "node:child_process";
import { setTimeout as sleep } from "node:timers/promises";

// Spawns skyfire-server (stream origin) and the Bun web server (app origin),
// and tears them down after the run. Ports are fixed for the harness.
export default async function globalSetup() {
  const root = new URL("../../", import.meta.url).pathname;
  const sf = spawn(`${root}target/debug/skyfire-server`,
    ["--fixtures", `${root}fixtures/streams`, "--port", "8090"],
    { stdio: "inherit" });
  const web = spawn("bun", ["run", "serve.ts"],
    { cwd: `${root}web`, env: { ...process.env, PORT: "8080" }, stdio: "inherit" });
  // Wait for both to answer.
  for (let i = 0; i < 50; i++) {
    try {
      const [a, b] = await Promise.all([
        fetch("http://127.0.0.1:8090/api/streams"),
        fetch("http://127.0.0.1:8080/index.html"),
      ]);
      if (a.ok && b.ok) break;
    } catch {}
    await sleep(200);
  }
  globalThis.__sfProcs = [sf, web];
  return async () => { sf.kill(); web.kill(); };
}
