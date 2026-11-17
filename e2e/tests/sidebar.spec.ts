import { execFileSync } from "node:child_process";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
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

test("dialog buttons are readable on their own fill", async ({ page }) => {
  const fixture = makeFixture("contrast");
  mkdirSync(path.join(fixture.root, "victim"));
  const { server, close } = await openWithFixture(fixture);
  try {
    await page.goto(server.baseURL);
    await expect(page.getByTestId("file-tree")).toContainText("victim/");

    // Ratio of the label against the button's own background, the way WCAG defines it.
    const contrast = async (selector: string) =>
      page.locator(selector).evaluate((element) => {
        const style = getComputedStyle(element);
        const parse = (value: string) => value.match(/\d+/g)!.slice(0, 3).map(Number);
        const channel = (c: number) => {
          const s = c / 255;
          return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
        };
        const luminance = ([r, g, b]: number[]) =>
          0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b);
        const foreground = luminance(parse(style.color));
        const background = luminance(parse(style.backgroundColor));
        const [light, dark] = [foreground, background].sort((a, b) => b - a);
        return (light + 0.05) / (dark + 0.05);
      });

    await page.getByTestId("new-folder").click();
    // 4.5:1 is the AA floor for body text; white on amber was 1.67:1.
    expect(await contrast(".modal .btn.primary")).toBeGreaterThan(4.5);
    await page.keyboard.press("Escape");

    await page.locator(".file-row[data-kind=dir]", { hasText: "victim/" }).click({ button: "right" });
    await page.getByTestId("delete").click();
    expect(await contrast(".modal .btn.danger")).toBeGreaterThan(4.5);
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

test("the tree climbs above the directory the server started in", async ({ page }) => {
  const fixture = makeFixture("climb");
  // Start inside a subdirectory, with the whole filesystem allowed.
  const server = await startServer([
    "-W",
    "-w",
    `${fixture.root}/sub`,
    "--file-root",
    "/",
    "bash",
    "--norc",
    "--noprofile",
  ]);
  try {
    await page.goto(server.baseURL);
    // `toHaveText` on the name, not `toContainText`: the initial text already contains
    // the first expected value, so a substring match would not wait for the rebase.
    const rootName = page.locator(".root-row .row-name");
    await expect(rootName).toHaveText(`${fixture.root}/sub`);

    await page.getByTestId("up").click();
    await expect(rootName).toHaveText(fixture.root);

    await page.getByTestId("up").click();
    await expect(rootName).toHaveText(path.dirname(fixture.root));

    // Navigating by hand pins the tree, like clicking a row does.
    await expect(page.getByTestId("follow")).not.toHaveClass(/active/);
  } finally {
    await server.stop();
    fixture.cleanup();
  }
});

test("rows a climb revealed are usable", async ({ page }) => {
  const fixture = makeFixture("climb-rows");
  mkdirSync(path.join(fixture.root, "sibling"));
  writeFileSync(path.join(fixture.root, "sibling", "inner.txt"), "x");
  const server = await startServer([
    "-W",
    "-w",
    `${fixture.root}/sub`,
    "--file-root",
    "/",
    "bash",
    "--norc",
    "--noprofile",
  ]);
  try {
    await page.goto(server.baseURL);
    const rootName = page.locator(".root-row .row-name");
    await expect(rootName).toHaveText(`${fixture.root}/sub`);

    await page.getByTestId("up").click();
    await expect(rootName).toHaveText(fixture.root);

    // `sibling/` only exists in the listing the climb fetched, so it is the row the old
    // anchor never knew about.
    const sibling = page.locator(".file-row[data-kind=dir]", { hasText: "sibling/" });
    await sibling.dblclick();
    await expect(page.locator(".file-row", { hasText: "inner.txt" })).toBeVisible();

    await sibling.click({ button: "right" });
    await expect(page.getByTestId("download-zip")).toBeVisible();
    // The menu is drawn over the row it belongs to, so it has to go before clicking.
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("download-zip")).toHaveCount(0);

    // Toolbar actions target the selection, not the anchor the climb landed on.
    await sibling.click();
    await page.getByTestId("new-folder").click();
    await page.locator(".modal-input").fill("made-here");
    await page.locator(".modal [data-role=confirm]").click();
    await expect(page.locator(".file-row", { hasText: "made-here/" })).toBeVisible();
    expect(existsSync(path.join(fixture.root, "sibling", "made-here"))).toBe(true);
  } finally {
    await server.stop();
    fixture.cleanup();
  }
});

test("a confined root stops the climb and says why", async ({ page }) => {
  const fixture = makeFixture("confined");
  const server = await startServer([
    "-W",
    "-w",
    fixture.root,
    "--file-root",
    fixture.root,
    "bash",
    "--norc",
    "--noprofile",
  ]);
  try {
    await page.goto(server.baseURL);
    const rootRow = page.locator(".root-row");
    await expect(rootRow).toContainText(fixture.root);

    await page.getByTestId("up").click();
    await expect(page.locator(".tree-status")).toContainText("outside the allowed roots");
    await expect(page.locator(".root-row .row-name")).toHaveText(fixture.root);
  } finally {
    await server.stop();
    fixture.cleanup();
  }
});

test("a long directory renders the rows you scroll to", async ({ page }) => {
  const fixture = makeFixture("windowing");
  const many = path.join(fixture.root, "many");
  mkdirSync(many);
  for (let index = 0; index < 300; index += 1) {
    writeFileSync(path.join(many, `file-${String(index).padStart(3, "0")}.txt`), "x");
  }
  const server = await startServer(["-w", fixture.root, "bash", "--norc", "--noprofile"]);
  try {
    await page.goto(server.baseURL);
    await page.locator(".file-row[data-kind=dir]", { hasText: "many/" }).dblclick();
    await expect(page.locator(".file-row", { hasText: "file-000.txt" })).toBeVisible();

    // The tail of the list starts outside the window, so it exists only after scrolling.
    const list = page.getByTestId("file-tree");
    await expect(page.locator(".file-row", { hasText: "file-299.txt" })).toHaveCount(0);
    await list.evaluate((element) => element.scrollTo({ top: element.scrollHeight }));
    await expect(page.locator(".file-row", { hasText: "file-299.txt" })).toBeVisible();
  } finally {
    await server.stop();
    fixture.cleanup();
  }
});

test("a rebase under a scrolled list still renders rows", async ({ page }) => {
  const fixture = makeFixture("rebase-scroll");
  const many = path.join(fixture.root, "many");
  mkdirSync(many);
  for (let index = 0; index < 300; index += 1) {
    writeFileSync(path.join(many, `file-${String(index).padStart(3, "0")}.txt`), "x");
  }
  const server = await startServer([
    "-W",
    "-w",
    fixture.root,
    "--file-root",
    "/",
    "bash",
    "--norc",
    "--noprofile",
  ]);
  try {
    await page.goto(server.baseURL);
    // Following the shell into the long directory keeps follow mode on for the next
    // step, which is where the anchor has to move.
    await page.locator(".xterm").click();
    await page.keyboard.type("cd many\n");
    await expect(page.locator(".file-row", { hasText: "file-000.txt" })).toBeVisible();

    const list = page.getByTestId("file-tree");
    await list.evaluate((element) => element.scrollTo({ top: element.scrollHeight }));
    await expect(page.locator(".file-row", { hasText: "file-299.txt" })).toBeVisible();

    // The shell leaves the tree behind, so a short listing replaces a long, scrolled one.
    await page.locator(".xterm").click();
    await page.keyboard.type("cd /\n");
    await expect(page.locator(".root-row .row-name")).toHaveText("/");

    // A stale scroll offset would window past the end of the new listing.
    const state = await list.evaluate((element) => ({
      top: element.scrollTop,
      rows: element.querySelectorAll(".file-row").length,
    }));
    expect(state.top).toBe(0);
    expect(state.rows).toBeGreaterThan(1);
  } finally {
    await server.stop();
    fixture.cleanup();
  }
});

test("the toolbar and rows carry the theme's icons", async ({ page, request }) => {
  const fixture = makeFixture("icons");
  mkdirSync(path.join(fixture.root, "locked"));
  chmodSync(path.join(fixture.root, "locked"), 0o000);
  const server = await startServer(["-W", "-w", fixture.root, "bash", "--norc", "--noprofile"]);
  try {
    await page.goto(server.baseURL);
    const tree = page.getByTestId("file-tree");

    // Every action keeps an accessible name even though it now shows a picture.
    for (const name of [
      "Parent directory",
      "Refresh",
      "Toggle hidden files",
      "New folder",
      "Upload files",
      "Follow the shell's working directory",
    ]) {
      const button = page.getByRole("button", { name });
      await expect(button).toBeVisible();
      const icon = button.locator("img");
      await expect(icon).toHaveAttribute("alt", "");
      // A 200 with an empty body would still be an <img> that renders nothing.
      await expect.poll(async () => icon.evaluate((image: HTMLImageElement) => image.naturalWidth)).toBeGreaterThan(0);
    }

    // Rows show a folder or a document; the padlock marks the one we cannot open.
    const lockedRow = tree.locator(".file-row", { hasText: "locked/" });
    await expect(lockedRow.locator("img")).toHaveAttribute("src", /folder-base/);
    await lockedRow.dblclick();
    await expect(lockedRow.locator("img")).toHaveAttribute("src", /folder-admin/);

    await expect(tree.locator(".file-row", { hasText: "a.txt" }).locator("img")).toHaveAttribute(
      "src",
      /document/,
    );

    const favicon = await request.get(`${server.baseURL}/favicon.svg`);
    expect(favicon.status()).toBe(200);
    expect(favicon.headers()["content-type"]).toBe("image/svg+xml");
    await expect(page.locator('link[rel="icon"]')).toHaveAttribute("href", "./favicon.svg");
  } finally {
    await server.stop();
    // The fixture is deleted recursively, which cannot descend into a 000 directory.
    chmodSync(path.join(fixture.root, "locked"), 0o755);
    fixture.cleanup();
  }
});

test("the footer names the running server's version", async ({ page, request }) => {
  const fixture = makeFixture("footer");
  const { server, close } = await openWithFixture(fixture);
  try {
    // The expected string comes from the server, so a hardcoded constant in the client
    // would not pass: it has to be reading what this process reports.
    const info = await (await request.get(`${server.baseURL}/api/info`)).json();
    await page.goto(server.baseURL);
    await expect(page.locator(".sidebar-footer")).toHaveText(`fttyd ${info.version}`);
  } finally {
    await close();
  }
});
