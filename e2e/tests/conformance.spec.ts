import { expect, test } from "@playwright/test";

import { startServer, ttydClientAvailable, ttydClientPath, type RunningServer } from "../src/server";
import { runCommand, terminalSize, waitForConnected } from "../src/terminal";

// This is the drop-in proof: ttyd's own built client, unmodified, driven against fttyd.
test.skip(
  !ttydClientAvailable(),
  `ttyd client bundle missing at ${ttydClientPath}; run scripts/extract-ttyd-frontend.py`,
);

let server: RunningServer;

test.beforeAll(async () => {
  server = await startServer(["-W", "-I", ttydClientPath, "bash", "--norc", "--noprofile"]);
});

test.afterAll(async () => {
  await server.stop();
});

test("ttyd's own client connects and runs commands", async ({ page }) => {
  await page.goto(server.baseURL);
  await waitForConnected(page);
  await runCommand(page, "echo ttyd-client-works", "ttyd-client-works");
});

test("ttyd's own client sets the window title from the server", async ({ page }) => {
  await page.goto(server.baseURL);
  await waitForConnected(page);
  await expect.poll(() => page.title(), { timeout: 15_000 }).toContain("bash");
});

test("ttyd's own client resizes the pty", async ({ page }) => {
  await page.setViewportSize({ width: 1000, height: 600 });
  await page.goto(server.baseURL);
  await waitForConnected(page);

  await page.setViewportSize({ width: 1500, height: 900 });
  await page.waitForTimeout(600);
  const { cols, rows } = await terminalSize(page);
  expect(cols).toBeGreaterThan(90);

  await runCommand(page, "stty size", `${rows} ${cols}`);
});

test("ttyd's own client reconnects after the shell exits", async ({ page }) => {
  await page.goto(server.baseURL);
  await waitForConnected(page);
  await runCommand(page, "exit", "");
  await expect
    .poll(async () => page.locator(".xterm").isVisible(), { timeout: 15_000 })
    .toBe(true);
});
