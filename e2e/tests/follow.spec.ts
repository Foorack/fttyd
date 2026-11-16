import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { expect, test } from "@playwright/test";

import { makeFixture } from "../src/fixtures";
import { startServer } from "../src/server";
import { runCommand, screenText } from "../src/terminal";

test("tree follows the shell working directory", async ({ page }) => {
  const fixture = makeFixture("follow");
  mkdirSync(path.join(fixture.root, "sub", "deep"));
  const server = await startServer(["-W", "-w", fixture.root, "bash", "--norc", "--noprofile"]);
  try {
    await page.goto(server.baseURL);
    const tree = page.getByTestId("file-tree");
    await expect(tree).toContainText("sub/");

    await page.locator(".xterm").click();
    await page.keyboard.type("cd sub/deep\n");
    await expect
      .poll(async () => (await page.locator(".file-row.selected").textContent()) ?? "", {
        timeout: 15_000,
      })
      .toContain("deep/");
  } finally {
    await server.stop();
    fixture.cleanup();
  }
});

test("navigating pins the tree until follow is re-enabled", async ({ page }) => {
  const fixture = makeFixture("pin");
  mkdirSync(path.join(fixture.root, "sub2"));
  const server = await startServer(["-W", "-w", fixture.root, "bash", "--norc", "--noprofile"]);
  try {
    await page.goto(server.baseURL);
    const tree = page.getByTestId("file-tree");
    await expect(tree).toContainText("sub/");

    // Navigating by hand pins the tree.
    await page.locator(".file-row[data-kind=dir]", { hasText: "sub2/" }).click();
    await expect(page.getByTestId("follow")).not.toHaveClass(/active/);

    await page.locator(".xterm").click();
    await page.keyboard.type("cd sub\n");
    await page.waitForTimeout(2500);
    await expect(page.locator(".file-row.selected")).toContainText("sub2/");

    // Re-enabling follow jumps to the shell's directory.
    await page.getByTestId("follow").click();
    await expect
      .poll(async () => (await page.locator(".file-row.selected").textContent()) ?? "", {
        timeout: 15_000,
      })
      .toContain("sub/");
  } finally {
    await server.stop();
    fixture.cleanup();
  }
});

test("open terminal here changes the shell directory", async ({ page }) => {
  const fixture = makeFixture("openterm");
  mkdirSync(path.join(fixture.root, "target dir"));
  const server = await startServer(["-W", "-w", fixture.root, "bash", "--norc", "--noprofile"]);
  try {
    await page.goto(server.baseURL);
    const tree = page.getByTestId("file-tree");
    await expect(tree).toContainText("target dir/");

    await page
      .locator(".file-row[data-kind=dir]", { hasText: "target dir/" })
      .click({ button: "right" });
    await page.getByTestId("open-terminal").click();

    await page.locator(".xterm").click();
    await runCommand(page, "pwd", "target dir");
    const output = await screenText(page);
    expect(output).toContain(path.join(fixture.root, "target dir"));
  } finally {
    await server.stop();
    fixture.cleanup();
  }
});

test("readonly sessions hide the open-terminal action", async ({ page }) => {
  const fixture = makeFixture("readonly-term");
  mkdirSync(path.join(fixture.root, "sub3"));
  writeFileSync(path.join(fixture.root, "sub3", "x.txt"), "x");
  const server = await startServer(["-w", fixture.root, "bash", "--norc", "--noprofile"]);
  try {
    await page.goto(server.baseURL);
    await page
      .locator(".file-row[data-kind=dir]", { hasText: "sub/" })
      .click({ button: "right" });
    await expect(page.getByTestId("download-zip")).toBeVisible();
    await expect(page.getByTestId("open-terminal")).toHaveCount(0);
    await expect(page.getByTestId("rename")).toHaveCount(0);
  } finally {
    await server.stop();
    fixture.cleanup();
  }
});

test("following re-anchors the tree when the shell leaves it", async ({ page }) => {
  const fixture = makeFixture("follow-reanchor");
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

    // `/` is above the tree's anchor, so following it has to move the anchor rather
    // than reveal a path that does not exist in the loaded tree.
    await page.locator(".xterm").click();
    await page.keyboard.type("cd /\n");
    await expect(rootName).toHaveText("/");
    await expect(page.getByTestId("follow")).toHaveClass(/active/);
  } finally {
    await server.stop();
    fixture.cleanup();
  }
});

test("following returns to the shell after the tree climbed away", async ({ page }) => {
  const fixture = makeFixture("follow-climb");
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

    // Climb past the shell's directory: the tree now shows an ancestor listing.
    await page.getByTestId("up").click();
    await expect(rootName).toHaveText(fixture.root);
    await page.getByTestId("up").click();
    await expect(rootName).toHaveText(path.dirname(fixture.root));
    await expect(page.getByTestId("follow")).not.toHaveClass(/active/);

    // Re-enabling follow anchors the tree on the shell again.
    await page.getByTestId("follow").click();
    await expect(page.locator(".file-row.selected")).toContainText("sub/");
    await expect(page.getByTestId("follow")).toHaveClass(/active/);
  } finally {
    await server.stop();
    fixture.cleanup();
  }
});
