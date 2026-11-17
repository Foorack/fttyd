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

test("a custom index replaces the UI but the protocol still works", async ({ page }) => {
  const index = `${process.cwd()}/.cache/ttyd-client.html`;
  const server = await startServer(["-W", "-I", index, "bash", "--norc", "--noprofile"]);
  try {
    await page.goto(server.baseURL);
    await waitForConnected(page);
    // The sidebar belongs to our UI, so a foreign index must not break anything.
    await page.locator(".xterm").click();
    await page.keyboard.type("echo foreign-index-ok\n");
    await expect
      .poll(() => screenText(page), { timeout: 15_000 })
      .toContain("foreign-index-ok");
  } finally {
    await server.stop();
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
