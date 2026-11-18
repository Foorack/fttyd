import { expect, test } from "@playwright/test";

import { startServer, type RunningServer } from "../src/server";
import { runCommand, screenText, terminalSize, waitForConnected } from "../src/terminal";
import { openQuietClient } from "../src/ws";

let server: RunningServer;

test.beforeAll(async () => {
  server = await startServer(["-W", "bash", "--norc", "--noprofile"]);
});

test.afterAll(async () => {
  await server.stop();
});

test("serves the embedded index with a cacheable etag", async ({ request }) => {
  const response = await request.get(`${server.baseURL}/`);
  expect(response.status()).toBe(200);
  expect(response.headers()["content-type"]).toContain("text/html");
  expect(await response.text()).toContain("fttyd");
});

test("serves the bundle and returns 304 for a known etag", async ({ request }) => {
  const index = await (await request.get(`${server.baseURL}/`)).text();
  const asset = /src="\.\/(assets\/[^"]+\.js)"/.exec(index)?.[1];
  expect(asset, "index should reference a hashed javascript bundle").toBeTruthy();

  const first = await request.get(`${server.baseURL}/${asset}`);
  expect(first.status()).toBe(200);
  expect(first.headers()["content-type"]).toContain("text/javascript");
  const etag = first.headers()["etag"];

  const second = await request.get(`${server.baseURL}/${asset}`, {
    headers: { "if-none-match": etag },
  });
  expect(second.status()).toBe(304);
});

test("terminal echoes typed input", async ({ page }) => {
  await page.goto(server.baseURL);
  await waitForConnected(page);
  await runCommand(page, "echo hello-fttyd", "hello-fttyd");
});

test("window title follows the server title", async ({ page }) => {
  await page.goto(server.baseURL);
  await waitForConnected(page);
  await expect.poll(() => page.title()).toContain("bash");
});

test("resizing the viewport resizes the pty", async ({ page }) => {
  await page.setViewportSize({ width: 1000, height: 600 });
  await page.goto(server.baseURL);
  await waitForConnected(page);

  await page.setViewportSize({ width: 1400, height: 800 });
  await page.waitForTimeout(500);
  const { cols, rows } = await terminalSize(page);
  expect(cols).toBeGreaterThan(80);

  await runCommand(page, "stty size", `${rows} ${cols}`);
});

test("readonly servers ignore input", async ({ page }) => {
  const readonly = await startServer(["bash", "--norc", "--noprofile"]);
  try {
    await page.goto(readonly.baseURL);
    await waitForConnected(page);
    await page.locator(".xterm").click();
    await page.keyboard.type("echo SHOULD-NOT-APPEAR");
    await page.keyboard.press("Enter");
    await page.waitForTimeout(1000);
    expect(await screenText(page)).not.toContain("SHOULD-NOT-APPEAR");
  } finally {
    await readonly.stop();
  }
});

test("base path moves every endpoint", async ({ page }) => {
  const nested = await startServer(["-W", "-b", "/mounted/here", "bash", "--norc", "--noprofile"]);
  try {
    const redirect = await page.request.get(`${nested.baseURL}/mounted/here`, {
      maxRedirects: 0,
    });
    expect(redirect.status()).toBe(302);
    expect(redirect.headers()["location"]).toBe("/mounted/here/");

    await page.goto(`${nested.baseURL}/mounted/here/`);
    await waitForConnected(page);
    await runCommand(page, "echo nested-ok", "nested-ok");

    expect((await page.request.get(`${nested.baseURL}/`)).status()).toBe(404);
    expect((await page.request.get(`${nested.baseURL}/mounted/here/token`)).status()).toBe(200);
  } finally {
    await nested.stop();
  }
});

test("basic auth challenges then accepts the credential", async ({ page }) => {
  const secured = await startServer([
    "-W",
    "-c",
    "user:pass",
    "bash",
    "--norc",
    "--noprofile",
  ]);
  try {
    const denied = await page.request.get(`${secured.baseURL}/`);
    expect(denied.status()).toBe(401);
    expect(denied.headers()["www-authenticate"]).toContain('Basic realm="ttyd"');

    const token = await page.request.get(`${secured.baseURL}/token`, {
      headers: { authorization: "Basic dXNlcjpwYXNz" },
    });
    expect(await token.text()).toBe('{"token": "dXNlcjpwYXNz"}');
  } finally {
    await secured.stop();
  }
});

test("a client that answers no ping keeps its session", async () => {
  // A frozen background tab holds the socket open and answers nothing. Killing that
  // session at 12 s cost the user their shell, so the reaper now waits minutes.
  const client = await openQuietClient(server.port);
  try {
    await new Promise((resolve) => setTimeout(resolve, 18_000));
    expect(client.pings(), "the server should still be pinging").toBeGreaterThanOrEqual(3);
    expect(client.ended(), "the session must survive a silent client").toBe(false);
  } finally {
    client.close();
  }
});
