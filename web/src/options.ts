import type { ITerminalOptions, ITheme } from "@xterm/xterm";

export type RendererType = "dom" | "canvas" | "webgl";

/** Client-side switches, mirroring ttyd's `-t` keys. */
export interface ClientOptions {
  rendererType: RendererType;
  disableLeaveAlert: boolean;
  disableResizeOverlay: boolean;
  /** Accepted for ttyd compatibility; file transfer now goes through the sidebar. */
  enableZmodem: boolean;
  enableTrzsz: boolean;
  enableSixel: boolean;
  titleFixed: string;
  closeOnDisconnect: boolean;
  isWindows: boolean;
  unicodeVersion: string;
}

export const DEFAULT_CLIENT_OPTIONS: ClientOptions = {
  rendererType: "webgl",
  disableLeaveAlert: false,
  disableResizeOverlay: false,
  enableZmodem: false,
  enableTrzsz: false,
  enableSixel: false,
  titleFixed: "",
  closeOnDisconnect: false,
  isWindows: false,
  unicodeVersion: "11",
};

const DEFAULT_THEME: ITheme = {
  // The VGA palette from the owner's GNOME terminal profile: pure primaries and
  // bold-is-bright, on a near-black slate instead of the profile's pure black, which
  // reads as a hole on a web page.
  foreground: "#d0cfcc",
  background: "#080b10",
  cursor: "#fbbf24",
  cursorAccent: "#080b10",
  selectionBackground: "#3a3a3a",
  black: "#000000",
  red: "#cd0000",
  green: "#00cd00",
  yellow: "#cdcd00",
  blue: "#0000cd",
  magenta: "#cd00cd",
  cyan: "#00cdcd",
  white: "#e5e5e5",
  brightBlack: "#7f7f7f",
  brightRed: "#ff0000",
  brightGreen: "#00ff00",
  brightYellow: "#ffff00",
  brightBlue: "#0075ff",
  brightMagenta: "#ff00ff",
  brightCyan: "#00ffff",
  brightWhite: "#ffffff",
};

export const DEFAULT_TERM_OPTIONS: ITerminalOptions = {
  fontSize: 13,
  // The profile this palette came from sets bold-is-bright; xterm defaults to the same.
  drawBoldTextInBrightColors: true,
  fontFamily: "Consolas,Liberation Mono,Menlo,Courier,monospace",
  theme: DEFAULT_THEME,
  allowProposedApi: true,
};

/** Everything a session needs, after `-t` options have been applied. */
export interface Settings {
  client: ClientOptions;
  term: ITerminalOptions;
}

export function defaultSettings(): Settings {
  return {
    client: { ...DEFAULT_CLIENT_OPTIONS },
    term: { ...DEFAULT_TERM_OPTIONS, theme: { ...DEFAULT_THEME } },
  };
}

/**
 * Merges the preference payload sent by the server (`-t key=value`).
 *
 * Values are JSON-decoded on the server, so a mismatch is possible: anything whose
 * shape does not match the built-in default is ignored rather than allowed to break
 * the terminal. Unknown keys are ignored on purpose, which is what makes ttyd
 * invocations that pass options we do not implement yet (`enableZmodem`, ...) still
 * work.
 */
export function applyPreferences(settings: Settings, prefs: Record<string, unknown>): void {
  for (const [key, value] of Object.entries(prefs)) {
    if (key in settings.client) {
      applyClientOption(settings, key, value);
      continue;
    }
    applyTermOption(settings, key, value);
  }
}

function applyClientOption(settings: Settings, key: string, value: unknown): void {
  const client = settings.client as unknown as Record<string, unknown>;
  const current = client[key];
  if (key === "rendererType") {
    if (value === "dom" || value === "canvas" || value === "webgl") {
      client[key] = value;
    }
    return;
  }
  if (key === "titleFixed") {
    if (typeof value === "string" || typeof value === "number" || typeof value === "boolean") {
      client[key] = String(value);
    }
    return;
  }
  if (typeof current === typeof value) {
    client[key] = value;
  }
}

function applyTermOption(settings: Settings, key: string, value: unknown): void {
  const term = settings.term as unknown as Record<string, unknown>;
  if (!(key in term)) return;
  const current = term[key];
  if (key === "theme") {
    if (value && typeof value === "object" && !Array.isArray(value)) {
      term[key] = { ...(current as ITheme), ...(value as ITheme) };
    }
    return;
  }
  if (typeof current !== typeof value) return;
  if (typeof value === "number" && (!Number.isFinite(value) || value <= 0) && key !== "scrollback") {
    return;
  }
  term[key] = value;
}
