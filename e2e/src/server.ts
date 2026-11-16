import { spawn, type ChildProcess } from "node:child_process";
import { existsSync } from "node:fs";
import { fileURLToPath } from "node:url";
import path from "node:path";

const here = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(here, "../..");

export const binaryPath =
  process.env.FTTYD_BIN ?? path.join(repoRoot, "target", "debug", "fttyd");

/** Path to the frontend bundle extracted from ttyd's `src/html.h`. */
export const ttydClientPath = path.join(repoRoot, "e2e", ".cache", "ttyd-client.html");

export function ttydClientAvailable(): boolean {
  return existsSync(ttydClientPath);
}

export interface RunningServer {
  port: number;
  baseURL: string;
  logs: string[];
  stop(): Promise<void>;
  /** Sends a signal to the process, as the user's Ctrl+C would. */
  signal(signal: NodeJS.Signals): void;
  /** Resolves with the exit code once the process ends. */
  exited(): Promise<number | null>;
}

/**
 * Starts the real binary on an ephemeral port and waits for its `listening on port N`
 * line, so tests never race a fixed port and never need to know how binding works.
 */
export async function startServer(args: string[]): Promise<RunningServer> {
  if (!existsSync(binaryPath)) {
    throw new Error(`fttyd binary not found at ${binaryPath}; run \`cargo build\` first`);
  }

  const child: ChildProcess = spawn(binaryPath, ["-p", "0", ...args], {
    stdio: ["ignore", "pipe", "pipe"],
  });
  const logs: string[] = [];

  const port = await new Promise<number>((resolve, reject) => {
    const onData = (chunk: Buffer) => {
      const text = chunk.toString();
      logs.push(text);
      const match = /listening on port (\d+)/.exec(text);
      if (match) resolve(Number(match[1]));
    };
    child.stderr?.on("data", onData);
    child.stdout?.on("data", onData);
    child.once("exit", (code) =>
      reject(new Error(`fttyd exited with ${code}: ${logs.join("")}`)),
    );
    setTimeout(() => reject(new Error(`fttyd did not start: ${logs.join("")}`)), 15_000);
  });

  return {
    port,
    baseURL: `http://127.0.0.1:${port}`,
    logs,
    signal(signal: NodeJS.Signals) {
      child.kill(signal);
    },
    exited() {
      return new Promise<number | null>((resolve) => {
        if (child.exitCode !== null) {
          resolve(child.exitCode);
          return;
        }
        child.once("exit", (code) => resolve(code));
      });
    },
    async stop() {
      if (child.exitCode !== null) return;
      await new Promise<void>((resolve) => {
        child.once("exit", () => resolve());
        child.kill("SIGTERM");
        setTimeout(() => {
          child.kill("SIGKILL");
          resolve();
        }, 3_000);
      });
    },
  };
}

/** Waits for an interactive shell by running a command until it answers. */
export async function waitForShell(page: import("@playwright/test").Page): Promise<void> {
  await page.waitForSelector(".xterm", { state: "visible" });
  await page.locator(".xterm").click();
}
