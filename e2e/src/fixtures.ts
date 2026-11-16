import { mkdtempSync, mkdirSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";

export interface Fixture {
  root: string;
  cleanup(): void;
}

/** Creates an isolated directory tree for one test. */
export function makeFixture(name: string): Fixture {
  const root = mkdtempSync(path.join(tmpdir(), `fttyd-e2e-${name}-`));
  mkdirSync(path.join(root, "sub"));
  writeFileSync(path.join(root, "a.txt"), "alpha");
  writeFileSync(path.join(root, "sub", "b.txt"), "bravo");
  writeFileSync(path.join(root, "empty.txt"), "");
  symlinkSync("a.txt", path.join(root, "link.txt"));
  return {
    root,
    cleanup() {
      rmSync(root, { recursive: true, force: true });
    },
  };
}

/** Directory holding hidden files, to exercise the visibility toggle. */
export function withHidden(fixture: Fixture): Fixture {
  writeFileSync(path.join(fixture.root, ".hidden.txt"), "shh");
  mkdirSync(path.join(fixture.root, ".hidden-dir"));
  return fixture;
}
