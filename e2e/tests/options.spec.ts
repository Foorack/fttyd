import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { expect, test } from "@playwright/test";

import { startServer } from "../src/server";
import { screenText, waitForConnected } from "../src/terminal";
import { handshake, handshakeWhileHeld } from "../src/ws";

test("--check-origin rejects a foreign origin", async () => {
  const server = await startServer(["-W", "-O", "bash", "--norc", "--noprofile"]);
  try {
    const foreign = await handshake(server.port, { origin: "http://evil.example" });
    expect(foreign.status).toBe(403);

    const sameOrigin = await handshake(server.port, { origin: `http://127.0.0.1:${server.port}` });
    expect(sameOrigin.status).toBe(101);
  } finally {
    await server.stop();
  }
});

test("--max-clients refuses the extra connection", async () => {
  const server = await startServer(["-W", "-m", "1", "bash", "--norc", "--noprofile"]);
  try {
    const { first, second } = await handshakeWhileHeld(server.port);
    expect(first).toBe(101);
    expect(second).toBe(503);
  } finally {
    await server.stop();
  }
});

test("--once refuses a second client and exits with the first", async () => {
  const server = await startServer(["-W", "-o", "bash", "--norc", "--noprofile"]);
  try {
    const { first, second } = await handshakeWhileHeld(server.port);
    expect(first).toBe(101);
    expect(second).toBe(503);
    // The held socket is closed by handshakeWhileHeld; with --once the server exits.
    await expect.poll(async () => server.exited(), { timeout: 15_000 }).not.toBeNull();
  } finally {
    await server.stop();
  }
});

test("--exit-no-conn exits when the last client leaves", async ({ page }) => {
  const server = await startServer(["-W", "-q", "bash", "--norc", "--noprofile"]);
  try {
    await page.goto(server.baseURL);
    await waitForConnected(page);
    await page.close();
    await expect.poll(async () => server.exited(), { timeout: 15_000 }).not.toBeNull();
  } finally {
    await server.stop();
  }
});

test("--url-arg appends the query arguments to the command", async ({ page }) => {
  const server = await startServer([
    "-W",
    "-a",
    "sh",
    "-c",
    'echo "received args: $*"; sleep 10',
    "sh",
  ]);
  try {
    await page.goto(`${server.baseURL}/?arg=alpha&arg=beta`);
    await waitForConnected(page);
    await expect
      .poll(() => screenText(page), { timeout: 15_000 })
      .toContain("received args: alpha beta");
  } finally {
    await server.stop();
  }
});

test("a custom index replaces the UI and speaks the documented protocol", async ({ page }) => {
  // A self-contained client rather than ttyd's bundle, so this runs everywhere: it sends
  // the control message the protocol defines and reads the `0` output frames back. If a
  // foreign client can drive a shell through this, `-I` is a real escape hatch.
  const dir = mkdtempSync(path.join(tmpdir(), "fttyd-index-"));
  const index = path.join(dir, "custom.html");
  writeFileSync(
    index,
    `<!doctype html><meta charset="utf-8"><title>foreign client</title>
<pre id="out"></pre>
<script>
const socket = new WebSocket(new URL("ws", location.href).href.replace(/^http/, "ws"), "tty");
const out = document.getElementById("out");
socket.binaryType = "arraybuffer";
socket.onopen = () => socket.send(JSON.stringify({ columns: 100, rows: 30 }));
socket.onmessage = (event) => {
  const bytes = new Uint8Array(event.data);
  if (bytes[0] === 48) out.textContent += new TextDecoder().decode(bytes.subarray(1));
};
window.term = {
  send(text) {
    const body = new TextEncoder().encode(text);
    const frame = new Uint8Array(body.length + 1);
    frame[0] = 48;
    frame.set(body, 1);
    socket.send(frame);
  },
};
</script>`,
  );
  const server = await startServer(["-W", "-I", index, "bash", "--norc", "--noprofile"]);
  try {
    await page.goto(server.baseURL);
    // Our sidebar belongs to our own UI, so a foreign index must not carry it.
    await expect(page.locator(".sidebar")).toHaveCount(0);
    await expect(page.locator("#out")).toHaveCount(1);

    await page.evaluate(() => (window as { term?: { send(t: string): void } }).term?.send("echo MARK_$((1+1))\n"));
    // MARK_2 is not in the text that was typed, so it proves the shell ran the command.
    await expect.poll(async () => page.locator("#out").textContent(), { timeout: 15_000 }).toContain("MARK_2");
  } finally {
    await server.stop();
    rmSync(dir, { recursive: true, force: true });
  }
});

test("--index rejects a directory", async () => {
  await expect(startServer(["-W", "-I", "/tmp", "bash"])).rejects.toThrow(/index/i);
});

test("-t rendererType selects the DOM renderer mid-session", async ({ page }) => {
  // The preference arrives over the socket, after the terminal has been created with
  // WebGL, so honouring it means shedding the addon rather than ignoring the option.
  const server = await startServer([
    "-W",
    "-t",
    'rendererType="dom"',
    "bash",
    "--norc",
    "--noprofile",
  ]);
  try {
    await page.goto(server.baseURL);
    await waitForConnected(page);
    await expect(page.locator(".xterm canvas")).toHaveCount(0);
    await expect.poll(async () => page.locator(".xterm-rows > div").count()).toBeGreaterThan(0);
  } finally {
    await server.stop();
  }
});

test("-B hands the URL to the system browser", async () => {
  test.skip(process.platform !== "linux", "the stub stands in for xdg-open");
  // A stub on PATH stands in for xdg-open: `-B` must launch it with the URL of the
  // listening port, and the test must not open a real window on the machine.
  const bin = mkdtempSync(path.join(tmpdir(), "fttyd-browser-"));
  const record = path.join(bin, "opened.txt");
  writeFileSync(
    path.join(bin, "xdg-open"),
    `#!/bin/sh\nprintf '%s\\n' "$1" > ${JSON.stringify(record)}\n`,
    { mode: 0o755 },
  );
  const server = await startServer(["-B", "-W", "bash", "--norc", "--noprofile"], {
    env: { PATH: `${bin}:${process.env.PATH ?? ""}` },
  });
  try {
    await expect
      .poll(async () => (existsSync(record) ? readFileSync(record, "utf8").trim() : ""), {
        timeout: 5_000,
      })
      .toBe(`http://localhost:${server.port}/`);
  } finally {
    await server.stop();
    rmSync(bin, { recursive: true, force: true });
  }
});
