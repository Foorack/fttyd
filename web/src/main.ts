import "./style.css";

import { defaultSettings } from "./options";
import { TerminalView } from "./terminal";

declare global {
  interface Window {
    term?: unknown;
  }
}

// Everything is served from the index path, which is what `-b` rewrites.
const basePath = window.location.pathname.replace(/\/+$/, "");
const wsProtocol = window.location.protocol === "https:" ? "wss:" : "ws:";
const endpoints = {
  wsUrl: `${wsProtocol}//${window.location.host}${basePath}/ws${window.location.search}`,
  tokenUrl: `${window.location.protocol}//${window.location.host}${basePath}/token`,
};

const app = document.getElementById("app");
if (!app) throw new Error("missing #app element");
app.className = "app";

const terminalPane = document.createElement("section");
terminalPane.className = "pane terminal-pane";
const terminalHost = document.createElement("div");
terminalHost.className = "terminal-host";
terminalPane.appendChild(terminalHost);
app.appendChild(terminalPane);

const view = new TerminalView(terminalHost, defaultSettings(), endpoints);
window.term = Object.assign(view.term, { fit: () => view.fit() });
void view.start();
