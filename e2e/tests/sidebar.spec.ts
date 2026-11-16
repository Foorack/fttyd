import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { expect, test } from "@playwright/test";

import { makeFixture, withHidden, type Fixture } from "../src/fixtures";
import { startServer, type RunningServer } from "../src/server";

async function openWithFixture(
  fixture: Fixture,
  extraArgs: string[] = [],
): Promise<{ server: RunningServer; close: () => Promise<void> }> {
  const server = await startServer(["-W", "-w", fixture.root, ...extraArgs, "bash", "--norc", "--noprofile"]);
  return {
    server,
    async close() {
      await server.stop();
      fixture.cleanup();
    },
  };
}

test("gauges show cpu, memory and disk values", async ({ page }) => {
  const fixture = makeFixture("gauges");
  const { server, close } = await openWithFixture(fixture);
  try {
    await page.goto(server.baseURL);
    const bars = page.locator(".gauge-bar");
    await expect(bars).toHaveCount(3);
    await expect.poll(async () => (await bars.nth(0).textContent()) ?? "").toMatch(/[\u2588\u2591]{16}/);
    // The numeric percentage/labels are always present, not just the bar.
    await expect(page.locator(".gauge-text").nth(0)).toHaveText(/%/);
    await expect(page.locator(".gauge-text").nth(1)).toContainText("/");
    await expect(page.locator(".gauge-text").nth(2)).toContainText("/");
    await expect(page.locator(".gauge-footer")).toContainText("load");
  } finally {
    await close();
  }
});

test("tree lists the working directory lazily", async ({ page }) => {
  const fixture = makeFixture("tree");
  const { server, close } = await openWithFixture(fixture);
  try {
    await page.goto(server.baseURL);
    const tree = page.getByTestId("file-tree");
    await expect(tree).toContainText("a.txt");
    await expect(tree).toContainText("sub/");

    // Note: locator(selector).dblclick() miscomputes the click point for rows inside
    // this scroll container, so the row locator is built from the page instead.
    const subRow = page.locator(".file-row[data-kind=dir]", { hasText: "sub/" });
    await subRow.dblclick();
    await expect(tree).toContainText("b.txt");

    // The caret is the explicit affordance and collapses the node again.
    await page.locator(".file-row[data-kind=dir]", { hasText: "sub/" }).getByTestId("caret").click();
    await expect(tree).not.toContainText("b.txt");
  } finally {
    await close();
  }
});

test("hidden files toggle", async ({ page }) => {
  const fixture = withHidden(makeFixture("hidden"));
  const { server, close } = await openWithFixture(fixture);
  try {
    await page.goto(server.baseURL);
    const tree = page.getByTestId("file-tree");
    await expect(tree).not.toContainText(".hidden.txt");
    await page.getByTestId("toggle-hidden").click();
    await expect(tree).toContainText(".hidden.txt");
    await page.getByTestId("toggle-hidden").click();
    await expect(tree).not.toContainText(".hidden.txt");
  } finally {
    await close();
  }
});

test("downloads a single file", async ({ page }) => {
  const fixture = makeFixture("download");
  const { server, close } = await openWithFixture(fixture);
  try {
    await page.goto(server.baseURL);
    const tree = page.getByTestId("file-tree");
    await tree.locator(".file-row", { hasText: "a.txt" }).click({ button: "right" });
    const [download] = await Promise.all([
      page.waitForEvent("download"),
      page.getByTestId("download-file").click(),
    ]);
    expect(download.suggestedFilename()).toBe("a.txt");
    const file = await download.path();
    expect(readFileSync(file, "utf8")).toBe("alpha");
  } finally {
    await close();
  }
});

test("downloads a folder as a valid zip", async ({ page }) => {
  const fixture = makeFixture("zip");
  const { server, close } = await openWithFixture(fixture);
  try {
    await page.goto(server.baseURL);
    const tree = page.getByTestId("file-tree");
    await tree.locator(".file-row", { hasText: "sub/" }).click({ button: "right" });
    const [download] = await Promise.all([
      page.waitForEvent("download"),
      page.getByTestId("download-zip").click(),
    ]);
    expect(download.suggestedFilename()).toBe("sub.zip");
    const file = await download.path();
    const listing = execFileSync("unzip", ["-l", file], { encoding: "utf8" });
    expect(listing).toContain("sub/b.txt");
    execFileSync("unzip", ["-t", file]);
  } finally {
    await close();
  }
});

test("downloads a folder as a valid tar.gz", async ({ page }) => {
  const fixture = makeFixture("targz");
  const { server, close } = await openWithFixture(fixture);
  try {
    await page.goto(server.baseURL);
    const tree = page.getByTestId("file-tree");
    await tree.locator(".file-row", { hasText: "sub/" }).click({ button: "right" });
    const [download] = await Promise.all([
      page.waitForEvent("download"),
      page.getByTestId("download-targz").click(),
    ]);
    expect(download.suggestedFilename()).toBe("sub.tar.gz");
    const file = await download.path();
    const listing = execFileSync("tar", ["-tzf", file], { encoding: "utf8" });
    expect(listing).toContain("sub/b.txt");
  } finally {
    await close();
  }
});

test("uploads a file and shows it in the tree", async ({ page }) => {
  const fixture = makeFixture("upload");
  const { server, close } = await openWithFixture(fixture);
  try {
    await page.goto(server.baseURL);
    // Select the fixture root so the upload lands there.
    await page.getByTestId("file-tree").locator(".root-row").click();

    const chooser = page.waitForEvent("filechooser");
    await page.getByTestId("upload").click();
    const fileChooser = await chooser;
    const source = mkdtempSync(path.join(tmpdir(), "fttyd-e2e-upload-src-"));
    const uploadPath = path.join(source, "upload.txt");
    writeFileSync(uploadPath, "uploaded bytes");
    await fileChooser.setFiles(uploadPath);

    const tree = page.getByTestId("file-tree");
    await expect(tree.locator(".file-row", { hasText: "upload.txt" })).toHaveCount(1, { timeout: 15_000 });
    expect(readFileSync(`${fixture.root}/upload.txt`, "utf8")).toBe("uploaded bytes");
    rmSync(source, { recursive: true, force: true });
  } finally {
    await close();
  }
});

test("creates, renames and deletes a folder", async ({ page }) => {
  const fixture = makeFixture("mutate");
  const { server, close } = await openWithFixture(fixture);
  try {
    await page.goto(server.baseURL);
    const tree = page.getByTestId("file-tree");
    await tree.locator(".root-row").click();

    await page.getByTestId("new-folder").click();
    await page.locator(".modal-input").fill("created");
    await page.locator(".modal [data-role=confirm]").click();
    await expect(tree).toContainText("created/");

    await tree.locator(".file-row", { hasText: "created/" }).click({ button: "right" });
    await page.getByTestId("rename").click();
    await page.locator(".modal-input").fill("renamed");
    await page.locator(".modal [data-role=confirm]").click();
    await expect(tree).toContainText("renamed/");
    await expect(tree).not.toContainText("created/");

    await tree.locator(".file-row", { hasText: "renamed/" }).click({ button: "right" });
    await page.getByTestId("delete").click();
    await page.locator(".modal [data-role=confirm]").click();
    await expect(tree).not.toContainText("renamed/");
  } finally {
    await close();
  }
});

test("readonly servers refuse mutations and hide write controls", async ({ page, request }) => {
  const fixture = makeFixture("readonly");
  const server = await startServer(["-w", fixture.root, "bash", "--norc", "--noprofile"]);
  try {
    await page.goto(server.baseURL);
    await expect(page.getByTestId("file-tree")).toContainText("a.txt");
    await expect(page.getByTestId("upload")).toHaveCount(0);
    await expect(page.getByTestId("new-folder")).toHaveCount(0);

    const response = await request.delete(
      `${server.baseURL}/api/fs?path=${encodeURIComponent(`${fixture.root}/a.txt`)}`,
      { headers: { "x-fttyd-token": "" } },
    );
    expect(response.status()).toBe(403);
    expect(readFileSync(`${fixture.root}/a.txt`, "utf8")).toBe("alpha");
  } finally {
    await server.stop();
    fixture.cleanup();
  }
});

test("mutations without the CSRF header are rejected", async ({ request }) => {
  const fixture = makeFixture("csrf");
  const { server, close } = await openWithFixture(fixture);
  try {
    const url = `${server.baseURL}/api/upload?path=${encodeURIComponent(`${fixture.root}/x.txt`)}`;
    const denied = await request.put(url, { data: "nope" });
    expect(denied.status()).toBe(403);

    const patchDenied = await request.patch(`${server.baseURL}/api/fs`, {
      data: { op: "mkdir", path: `${fixture.root}/nope` },
    });
    expect(patchDenied.status()).toBe(403);

    const allowed = await request.put(url, {
      data: "allowed",
      headers: { "x-fttyd-token": "", "content-range": "bytes 0-6/7" },
    });
    expect(allowed.ok()).toBeTruthy();
    expect(readFileSync(`${fixture.root}/x.txt`, "utf8")).toBe("allowed");
  } finally {
    await close();
  }
});

test("resumable upload continues from a partial file", async ({ request }) => {
  const fixture = makeFixture("resume");
  const { server, close } = await openWithFixture(fixture);
  try {
    const target = `${fixture.root}/chunked.bin`;
    const url = `${server.baseURL}/api/upload?path=${encodeURIComponent(target)}`;
    const headers = { "x-fttyd-token": "" };

    const first = await request.put(url, {
      data: "0123456789",
      headers: { ...headers, "content-range": "bytes 0-9/20" },
    });
    expect((await first.json()).complete).toBe(false);

    const second = await request.put(url, {
      data: "abcdefghij",
      headers: { ...headers, "content-range": "bytes 10-19/20" },
    });
    expect((await second.json()).complete).toBe(true);
    expect(readFileSync(target, "utf8")).toBe("0123456789abcdefghij");

    // A wrong offset must be refused instead of corrupting the file.
    const wrong = await request.put(url, {
      data: "zzz",
      headers: { ...headers, "content-range": "bytes 5-7/20" },
    });
    expect(wrong.status()).toBe(409);
    expect(readFileSync(target, "utf8")).toBe("0123456789abcdefghij");
  } finally {
    await close();
  }
});

test("file roots confine the API", async ({ page, request }) => {
  const fixture = makeFixture("roots");
  const { server, close } = await openWithFixture(fixture, ["--file-root", fixture.root]);
  try {
    const inside = await request.get(`${server.baseURL}/api/fs?path=${encodeURIComponent(fixture.root)}`);
    expect(inside.ok()).toBeTruthy();

    const outside = await request.get(`${server.baseURL}/api/fs?path=${encodeURIComponent("/etc")}`);
    expect(outside.status()).toBe(403);

    const escape = await request.get(
      `${server.baseURL}/api/fs?path=${encodeURIComponent(`${fixture.root}/../../../etc`)}`,
    );
    expect(escape.status()).toBe(403);
  } finally {
    await close();
  }
});

test("sidebar becomes a drawer on narrow screens", async ({ page }) => {
  const fixture = makeFixture("mobile");
  const { server, close } = await openWithFixture(fixture);
  try {
    await page.setViewportSize({ width: 600, height: 800 });
    await page.goto(server.baseURL);
    const sidebar = page.locator(".sidebar");
    await expect(sidebar).not.toBeInViewport();
    await page.locator(".drawer-toggle").click();
    await expect(sidebar).toBeInViewport();
    await page.keyboard.press("Escape");
    await expect(sidebar).not.toBeInViewport();
  } finally {
    await close();
  }
});

test("dragging the splitter resizes the terminal and persists", async ({ page }) => {
  const fixture = makeFixture("splitter");
  const { server, close } = await openWithFixture(fixture);
  try {
    await page.setViewportSize({ width: 1400, height: 800 });
    await page.goto(server.baseURL);
    const splitter = page.locator(".splitter");
    const box = await splitter.boundingBox();
    expect(box).not.toBeNull();
    if (!box) return;

    await page.mouse.move(box.x + 2, box.y + 200);
    await page.mouse.down();
    await page.mouse.move(box.x - 200, box.y + 200, { steps: 8 });
    await page.mouse.up();

    const stored = await page.evaluate(() => Number(localStorage.getItem("fttyd.sidebar-width")));
    expect(stored).toBeGreaterThan(400);

    await page.reload();
    const width = await page.evaluate(
      () => getComputedStyle(document.querySelector(".app")!).getPropertyValue("--sidebar-width").trim(),
    );
    expect(parseFloat(width)).toBeGreaterThan(400);
  } finally {
    await close();
  }
});

test("downloading a symlink follows it to the file", async ({ page, request }) => {
  const fixture = makeFixture("symlink");
  const { server, close } = await openWithFixture(fixture);
  try {
    const response = await request.get(
      `${server.baseURL}/api/download?path=${encodeURIComponent(`${fixture.root}/link.txt`)}`,
    );
    expect(response.status()).toBe(200);
    expect(await response.text()).toBe("alpha");

    // A symlink pointing at a directory cannot be downloaded as a file body.
    mkdirSync(`${fixture.root}/dir-target`);
    symlinkSync(`${fixture.root}/dir-target`, `${fixture.root}/dir-link`);
    const directory = await request.get(
      `${server.baseURL}/api/download?path=${encodeURIComponent(`${fixture.root}/dir-link`)}`,
    );
    expect(directory.status()).toBe(400);
    await page.goto(server.baseURL);
  } finally {
    await close();
  }
});
