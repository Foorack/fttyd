import { expect, type Page } from "@playwright/test";

/** Reads the whole terminal buffer, independent of the renderer in use. */
export async function screenText(page: Page): Promise<string> {
  return page.evaluate(() => {
    const term = (window as { term?: { buffer: { active: { length: number; getLine(i: number): { translateToString(trim?: boolean): string } | undefined } } } }).term;
    if (!term) return "";
    const buffer = term.buffer.active;
    const lines: string[] = [];
    for (let i = 0; i < buffer.length; i += 1) {
      const line = buffer.getLine(i);
      lines.push(line ? line.translateToString(true) : "");
    }
    return lines.join("\n");
  });
}

export async function terminalSize(page: Page): Promise<{ cols: number; rows: number }> {
  return page.evaluate(() => {
    const term = (window as { term?: { cols: number; rows: number } }).term;
    return { cols: term?.cols ?? 0, rows: term?.rows ?? 0 };
  });
}

/** Types a command into the focused terminal and waits for its output. */
export async function runCommand(page: Page, command: string, expected: string): Promise<void> {
  await page.locator(".xterm").click();
  await page.keyboard.type(command);
  await page.keyboard.press("Enter");
  await expect.poll(() => screenText(page), { timeout: 15_000 }).toContain(expected);
}

export async function waitForConnected(page: Page): Promise<void> {
  await page.waitForSelector(".xterm", { state: "visible" });
  // The prompt is drawn once the shell has started; polling for output is more
  // reliable than waiting for a particular prompt string.
  await expect
    .poll(async () => (await screenText(page)).trim().length, { timeout: 20_000 })
    .toBeGreaterThan(0);
}
