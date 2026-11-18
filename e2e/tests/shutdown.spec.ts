import { expect, test } from "@playwright/test";

import { startServer } from "../src/server";
import { waitForConnected } from "../src/terminal";

test("SIGINT stops the server even with a terminal attached", async ({ page }) => {
  const server = await startServer(["-W", "-w", "/tmp", "bash", "--norc", "--noprofile"]);
  try {
    await page.goto(server.baseURL);
    await waitForConnected(page);

    // The session is live: something has to close it, or graceful shutdown waits for a
    // browser tab that may never close.
    server.signal("SIGINT");
    await expect
      .poll(() => server.exited(), { timeout: 5_000, message: "server should exit" })
      .toBe(0);
    expect(server.logs.join("")).toMatch(/session ended after .*: server shutting down/);
  } finally {
    await server.stop();
  }
});

test("SIGTERM stops the server with no clients at all", async () => {
  const server = await startServer(["-W", "bash", "--norc", "--noprofile"]);
  server.signal("SIGTERM");
  await expect.poll(() => server.exited(), { timeout: 5_000 }).toBe(0);
});
