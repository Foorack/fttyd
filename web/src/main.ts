import "./style.css";

import { Api, type StatsSnapshot } from "./api";
import { Gauges } from "./gauges";
import { defaultSettings } from "./options";
import { TerminalView } from "./terminal";
import { FileTree } from "./tree";

declare global {
  interface Window {
    term?: unknown;
    fttyd?: { refresh: () => Promise<void>; targetDirectory: () => string | null };
  }
}

const SIDEBAR_STORAGE_KEY = "fttyd.sidebar-width";
/** Terminal keeps at least this much width while the splitter is dragged. */
const MIN_TERMINAL_WIDTH = 320;

// Everything is served from the index path, which `-b` rewrites.
const basePath = window.location.pathname.replace(/\/+$/, "");
const wsProtocol = window.location.protocol === "https:" ? "wss:" : "ws:";
const endpoints = {
  wsUrl: `${wsProtocol}//${window.location.host}${basePath}/ws${window.location.search}`,
  tokenUrl: `${window.location.protocol}//${window.location.host}${basePath}/token`,
};

const appElement = document.getElementById("app");
if (!appElement) throw new Error("missing #app element");
const app: HTMLElement = appElement;
app.className = "app";

const terminalPane = document.createElement("section");
terminalPane.className = "pane terminal-pane";
const terminalHost = document.createElement("div");
terminalHost.className = "terminal-host";
const drawerToggle = document.createElement("button");
drawerToggle.type = "button";
drawerToggle.className = "drawer-toggle";
drawerToggle.title = "Toggle the sidebar";
drawerToggle.setAttribute("aria-label", "Toggle the sidebar");
drawerToggle.textContent = "\u2630";
terminalPane.append(terminalHost, drawerToggle);

const splitter = document.createElement("div");
splitter.className = "splitter";
splitter.setAttribute("role", "separator");
splitter.setAttribute("aria-orientation", "vertical");
splitter.title = "Drag to resize";

const sidebar = document.createElement("aside");
sidebar.className = "pane sidebar";
const gaugesHost = document.createElement("div");
const treeHost = document.createElement("div");
treeHost.className = "tree-host";
sidebar.append(gaugesHost, treeHost);

app.append(terminalPane, splitter, sidebar);

const storedWidth = Number(window.localStorage.getItem(SIDEBAR_STORAGE_KEY) ?? "");
if (Number.isFinite(storedWidth) && storedWidth > 0) {
  app.style.setProperty("--sidebar-width", `${storedWidth}px`);
}

function setSidebarWidth(width: number): void {
  const maximum = Math.max(240, window.innerWidth - MIN_TERMINAL_WIDTH);
  const clamped = Math.max(240, Math.min(width, maximum));
  app.style.setProperty("--sidebar-width", `${clamped}px`);
  window.localStorage.setItem(SIDEBAR_STORAGE_KEY, String(Math.round(clamped)));
}

splitter.addEventListener("pointerdown", (event) => {
  splitter.setPointerCapture(event.pointerId);
  splitter.classList.add("dragging");
  const move = (moveEvent: PointerEvent) => {
    setSidebarWidth(window.innerWidth - moveEvent.clientX);
  };
  const stop = () => {
    splitter.classList.remove("dragging");
    splitter.removeEventListener("pointermove", move);
    splitter.removeEventListener("pointerup", stop);
    splitter.removeEventListener("pointercancel", stop);
  };
  splitter.addEventListener("pointermove", move);
  splitter.addEventListener("pointerup", stop);
  splitter.addEventListener("pointercancel", stop);
});

drawerToggle.addEventListener("click", () => sidebar.classList.toggle("open"));
window.addEventListener("keydown", (event) => {
  if (event.key === "Escape") sidebar.classList.remove("open");
});

// The terminal connects first: it is the primary surface and never depends on the
// sidebar being available.
let tree: FileTree | null = null;
const view = new TerminalView(terminalHost, defaultSettings(), endpoints, {
  onCwd: (path) => {
    // Follow the shell unless the user pinned the tree by navigating.
    void tree?.follow(path);
  },
});
window.term = Object.assign(view.term, { fit: () => view.fit() });
void view.start();

const gauges = new Gauges(gaugesHost);
let stats: StatsSnapshot | null = null;
let currentPath = "/";

function renderGauges(): void {
  if (stats) gauges.update(stats, currentPath);
}

function connectStats(): void {
  const source = new EventSource(`${basePath}/api/stats`);
  source.addEventListener("stats", (event) => {
    try {
      stats = JSON.parse((event as MessageEvent<string>).data) as StatsSnapshot;
      renderGauges();
    } catch {
      // A malformed sample must never take the sidebar down.
    }
  });
}

connectStats();

const api = new Api(basePath);

async function startSidebar(): Promise<void> {
  const info = await api.connect();
  const root = info.path ?? "/";
  currentPath = root;

  const fileTree = new FileTree(treeHost, {
    api,
    writable: info.writable,
    root,
    onPathChange: (path) => {
      currentPath = path;
      renderGauges();
    },
    onOpenTerminal: (path) => {
      if (!info.writable) return;
      view.sendText(`cd -- ${shellQuote(path)}\n`);
    },
  });
  tree = fileTree;
  await fileTree.start();
  currentPath = fileTree.targetDirectory();
  renderGauges();
  treeHost.dataset.writable = String(info.writable);

  window.fttyd = {
    refresh: () => fileTree.refresh(),
    targetDirectory: () => fileTree.targetDirectory(),
  };
}

/** POSIX single-quote escaping for a path typed into a shell. */
function shellQuote(value: string): string {
  return `'${value.replace(/'/g, `'\\''`)}'`;
}

startSidebar().catch((error: unknown) => {
  // The sidebar is additive: a failure here must leave the terminal fully usable.
  const message = document.createElement("div");
  message.className = "sidebar-error";
  message.textContent = `Sidebar unavailable: ${error instanceof Error ? error.message : String(error)}`;
  sidebar.insertBefore(message, treeHost);
});
