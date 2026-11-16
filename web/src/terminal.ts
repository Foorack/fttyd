import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";
import { Unicode11Addon } from "@xterm/addon-unicode11";
import { WebLinksAddon } from "@xterm/addon-web-links";
import "@xterm/xterm/css/xterm.css";

import { applyPreferences, type Settings } from "./options";

// Wire protocol, byte-compatible with ttyd.
const CMD_OUTPUT = 0x30;
const CMD_SET_WINDOW_TITLE = 0x31;
const CMD_SET_PREFERENCES = 0x32;
const CMD_INPUT = 0x30;
const CMD_RESIZE = 0x31;
const CMD_PAUSE = 0x32;
const CMD_RESUME = 0x33;
/**
 * fttyd extension: the child's working directory. Same byte as the client-sent
 * RESUME command, which is fine because the two never travel in the same direction.
 */
const CMD_SET_CWD = 0x33;

/** Write batching thresholds, same values as ttyd's client. */
const FLOW = { limit: 100_000, highWater: 10, lowWater: 4 };

/** Drops rapid duplicate resize messages produced while the splitter is dragged. */
const RESIZE_SETTLE_MS = 100;

export interface Endpoints {
  wsUrl: string;
  tokenUrl: string;
}

export interface TerminalViewCallbacks {
  onPreferences?: (prefs: Record<string, unknown>) => void;
  onConnectionChange?: (state: "connecting" | "open" | "closed") => void;
  onCwd?: (path: string) => void;
}

export class TerminalView {
  readonly term: Terminal;
  private readonly fitAddon = new FitAddon();
  private readonly overlay: HTMLDivElement;
  private decoder = new TextDecoder();
  private encoder = new TextEncoder();
  private socket: WebSocket | null = null;
  private token = "";
  private opened = false;
  private title = "";
  private titleFixed = "";
  private pending = 0;
  private written = 0;
  private reconnectEnabled = true;
  private reconnectTimer: number | null = null;
  private overlayTimer: number | null = null;
  private lastSize = { cols: 0, rows: 0 };
  private disposed = false;
  private resizeTimer: number | null = null;
  private readonly resizeObserver: ResizeObserver;
  private readonly leaveGuard = (event: BeforeUnloadEvent) => {
    if (!this.settings.client.disableLeaveAlert) {
      event.preventDefault();
      event.returnValue = "";
    }
  };

  constructor(
    private readonly container: HTMLElement,
    private settings: Settings,
    private readonly endpoints: Endpoints,
    private readonly callbacks: TerminalViewCallbacks = {},
  ) {
    this.term = new Terminal(settings.term);
    this.overlay = document.createElement("div");
    this.overlay.className = "terminal-overlay";
    this.overlay.hidden = true;
    this.container.appendChild(this.overlay);
    this.resizeObserver = new ResizeObserver(() => {
      if (this.resizeTimer !== null) window.clearTimeout(this.resizeTimer);
      this.resizeTimer = window.setTimeout(() => this.fit(), RESIZE_SETTLE_MS);
    });
    this.installListeners();
  }

  async start(): Promise<void> {
    this.term.loadAddon(this.fitAddon);
    this.term.loadAddon(new Unicode11Addon());
    this.term.loadAddon(new WebLinksAddon());
    if (this.settings.client.unicodeVersion === "11") {
      this.term.unicode.activeVersion = "11";
    }
    this.titleFixed = this.settings.client.titleFixed;
    this.term.open(this.container);
    this.loadRenderer();
    this.fit();
    await this.refreshToken();
    this.connect();
  }

  dispose(): void {
    this.disposed = true;
    window.removeEventListener("beforeunload", this.leaveGuard);
    this.resizeObserver.disconnect();
    if (this.reconnectTimer !== null) window.clearTimeout(this.reconnectTimer);
    if (this.overlayTimer !== null) window.clearTimeout(this.overlayTimer);
    this.socket?.close(1000, "client disposed");
    this.socket = null;
    this.term.dispose();
    this.overlay.remove();
  }

  /** Re-fits the terminal to its container, sending a resize only if it changed. */
  fit(): void {
    this.fitAddon.fit();
  }

  focus(): void {
    this.term.focus();
  }

  /**
   * WebGL is the default renderer, but it is unavailable in some environments
   * (software rendering, lost context, headless browsers). Falling back to the DOM
   * renderer keeps the terminal usable instead of failing to start.
   */
  private loadRenderer(): void {
    if (this.settings.client.rendererType === "dom") return;
    try {
      const webgl = new WebglAddon();
      // Disposing the addon is what makes xterm.js fall back to the DOM renderer.
      webgl.onContextLoss(() => webgl.dispose());
      this.term.loadAddon(webgl);
    } catch {
      // No WebGL here: the built-in DOM renderer is already active.
    }
  }

  private installListeners(): void {
    this.term.onData((data) => this.sendData(data));
    this.term.onBinary((data) => {
      const bytes = new Uint8Array(data.length);
      for (let i = 0; i < data.length; i += 1) bytes[i] = data.charCodeAt(i);
      this.sendData(bytes);
    });
    this.term.onTitleChange((data) => {
      if (!data.trim() || this.titleFixed) return;
      document.title = `${data} | ${this.title}`;
    });
    this.term.onResize(({ cols, rows }) => {
      if (cols === this.lastSize.cols && rows === this.lastSize.rows) return;
      this.lastSize = { cols, rows };
      const payload = this.encoder.encode(JSON.stringify({ columns: cols, rows: rows }));
      const message = new Uint8Array(payload.length + 1);
      message[0] = CMD_RESIZE;
      message.set(payload, 1);
      if (this.socket?.readyState === WebSocket.OPEN) this.socket.send(message);
      if (!this.settings.client.disableResizeOverlay) {
        this.showOverlay(`${cols}x${rows}`, 300);
      }
    });
    window.addEventListener("resize", () => this.fit());
    window.addEventListener("beforeunload", this.leaveGuard);

    // The splitter and the sidebar change the container without a window resize.
    this.resizeObserver.observe(this.container);
  }

  private async refreshToken(): Promise<void> {
    try {
      const response = await fetch(this.endpoints.tokenUrl, { credentials: "same-origin" });
      if (!response.ok) return;
      const body = (await response.json()) as { token?: string };
      this.token = typeof body.token === "string" ? body.token : "";
    } catch {
      // A missing token is only fatal when the server requires one; the websocket
      // handshake will be rejected in that case and the UI reports it.
    }
  }

  private connect(): void {
    if (this.disposed) return;
    this.callbacks.onConnectionChange?.("connecting");
    const socket = new WebSocket(this.endpoints.wsUrl, ["tty"]);
    socket.binaryType = "arraybuffer";
    this.socket = socket;

    socket.addEventListener("open", () => {
      this.callbacks.onConnectionChange?.("open");
      const control = this.encoder.encode(
        JSON.stringify({ AuthToken: this.token, columns: this.term.cols, rows: this.term.rows }),
      );
      socket.send(control);
      if (this.opened) {
        this.term.reset();
        this.term.options.disableStdin = false;
        this.showOverlay("Reconnected", 600);
      }
      this.opened = true;
      this.reconnectEnabled = true;
      this.lastSize = { cols: this.term.cols, rows: this.term.rows };
      this.focus();
    });

    socket.addEventListener("message", (event) => {
      const data = new Uint8Array(event.data as ArrayBuffer);
      if (data.length === 0) return;
      this.handleFrame(data);
    });

    socket.addEventListener("close", (event) => {
      this.callbacks.onConnectionChange?.("closed");
      if (this.disposed) return;
      if (event.code !== 1000 && this.reconnectEnabled) {
        this.showOverlay("Reconnecting...");
        this.reconnectTimer = window.setTimeout(() => {
          void this.refreshToken().then(() => this.connect());
        }, 1000);
        return;
      }
      if (this.settings.client.closeOnDisconnect) {
        window.close();
        return;
      }
      this.showOverlay("Press \u23ce to Reconnect");
      const subscription = this.term.onKey((event) => {
        if (event.domEvent.key !== "Enter") return;
        subscription.dispose();
        this.showOverlay("Reconnecting...");
        void this.refreshToken().then(() => this.connect());
      });
    });

    socket.addEventListener("error", () => {
      this.reconnectEnabled = false;
    });
  }

  private handleFrame(data: Uint8Array): void {
    switch (data[0]) {
      case CMD_OUTPUT:
        // Streaming decode: a UTF-8 sequence can straddle two frames.
        this.writeData(this.decoder.decode(data.subarray(1), { stream: true }));
        break;
      case CMD_SET_WINDOW_TITLE: {
        this.title = new TextDecoder().decode(data.subarray(1));
        document.title = this.titleFixed || this.title;
        break;
      }
      case CMD_SET_CWD: {
        try {
          const body = JSON.parse(new TextDecoder().decode(data.subarray(1))) as { path?: unknown };
          if (typeof body.path === "string" && body.path.startsWith("/")) {
            this.callbacks.onCwd?.(body.path);
          }
        } catch {
          // A malformed cwd frame is not worth interrupting the session for.
        }
        break;
      }
      case CMD_SET_PREFERENCES: {
        const text = new TextDecoder().decode(data.subarray(1));
        try {
          const prefs = JSON.parse(text) as Record<string, unknown>;
          applyPreferences(this.settings, prefs);
          this.term.options = this.settings.term;
          this.titleFixed = this.settings.client.titleFixed;
          if (this.titleFixed) document.title = this.titleFixed;
          this.callbacks.onPreferences?.(prefs);
          this.fit();
        } catch {
          // Malformed preferences must not take the terminal down.
        }
        break;
      }
      default:
        break;
    }
  }

  private writeData(data: string): void {
    const { limit, highWater, lowWater } = FLOW;
    this.written += data.length;
    if (this.written <= limit) {
      this.term.write(data);
      return;
    }
    this.term.write(data, () => {
      this.pending = Math.max(this.pending - 1, 0);
      if (this.pending < lowWater) this.sendCommand(CMD_RESUME);
    });
    this.pending += 1;
    this.written = 0;
    if (this.pending > highWater) this.sendCommand(CMD_PAUSE);
  }

  private sendCommand(command: number): void {
    if (this.socket?.readyState !== WebSocket.OPEN) return;
    this.socket.send(new Uint8Array([command]));
  }

  /** Sends text as terminal input, used by actions such as "open terminal here". */
  sendText(text: string): void {
    this.sendData(text);
  }

  private sendData(data: string | Uint8Array): void {
    if (this.socket?.readyState !== WebSocket.OPEN) return;
    if (typeof data === "string") {
      const payload = new Uint8Array(data.length * 3 + 1);
      payload[0] = CMD_INPUT;
      const result = this.encoder.encodeInto(data, payload.subarray(1));
      this.socket.send(payload.subarray(0, (result.written ?? 0) + 1));
    } else {
      const payload = new Uint8Array(data.length + 1);
      payload[0] = CMD_INPUT;
      payload.set(data, 1);
      this.socket.send(payload);
    }
  }

  private showOverlay(text: string, hideAfterMs?: number): void {
    this.overlay.textContent = text;
    this.overlay.hidden = false;
    if (this.overlayTimer !== null) window.clearTimeout(this.overlayTimer);
    this.overlayTimer = null;
    if (hideAfterMs !== undefined) {
      this.overlayTimer = window.setTimeout(() => {
        this.overlay.hidden = true;
      }, hideAfterMs);
    }
  }
}
