// HTTP API client for the sidebar. Paths are absolute and always sent as a query
// parameter, so the browser percent-encodes them for us.

export type EntryKind = "dir" | "file" | "symlink" | "other";

export interface Entry {
  name: string;
  path: string | null;
  kind: EntryKind;
  size: number | null;
  mtime: number | null;
  mode: string | null;
  mode_octal: number | null;
  uid: number | null;
  gid: number | null;
  symlink_target: string | null;
  error: string | null;
}

export interface Listing {
  path: string;
  parent: string | null;
  name: string;
  entries: Entry[];
}

export interface Info {
  version: string;
  writable: boolean;
  path: string | null;
  roots: string[];
  csrf_header: string;
}

export interface DiskUsage {
  mount: string;
  file_system: string;
  total: number;
  available: number;
}

export interface StatsSnapshot {
  cpu: number;
  cpu_count: number;
  mem_total: number;
  mem_used: number;
  swap_total: number;
  swap_used: number;
  load_one: number;
  load_five: number;
  load_fifteen: number;
  uptime: number;
  disks: DiskUsage[];
}

export type MutationBody =
  | { op: "mkdir"; path: string }
  | { op: "rename"; path: string; to: string }
  | { op: "chmod"; path: string; mode: string };

/** Upload chunk size: large enough to be efficient, small enough to resume cheaply. */
const CHUNK_SIZE = 4 * 1024 * 1024;

export class ApiError extends Error {
  constructor(
    message: string,
    readonly status: number,
  ) {
    super(message);
  }
}

async function readError(response: Response): Promise<string> {
  try {
    const body = (await response.json()) as { error?: string };
    if (body?.error) return body.error;
  } catch {
    // Fall through to the status text.
  }
  return `${response.status} ${response.statusText}`;
}

export class Api {
  private csrfHeader = "x-fttyd-token";
  private token = "";

  constructor(private readonly base: string) {}

  /** Fetches capabilities and the CSRF token. Safe to call again after a reconnect. */
  async connect(): Promise<Info> {
    const [tokenResponse, info] = await Promise.all([
      fetch(`${this.base}/token`, { credentials: "same-origin" }),
      this.getJson<Info>(`${this.base}/api/info`),
    ]);
    if (tokenResponse.ok) {
      const body = (await tokenResponse.json()) as { token?: string };
      this.token = typeof body.token === "string" ? body.token : "";
    }
    this.csrfHeader = info.csrf_header || this.csrfHeader;
    return info;
  }

  async list(path: string): Promise<Listing> {
    return this.getJson<Listing>(`${this.base}/api/fs?path=${encodeURIComponent(path)}`);
  }

  downloadUrl(path: string): string {
    return `${this.base}/api/download?path=${encodeURIComponent(path)}`;
  }

  archiveUrl(path: string, format: "zip" | "tar.gz"): string {
    return `${this.base}/api/archive?path=${encodeURIComponent(path)}&format=${format}`;
  }

  /** Uploads a file in resumable chunks, reporting progress from 0 to 1. */
  async upload(directory: string, file: File, onProgress?: (fraction: number) => void): Promise<void> {
    const target = joinPath(directory, file.name);
    const url = `${this.base}/api/upload?path=${encodeURIComponent(target)}`;
    if (file.size === 0) {
      await this.putChunk(url, new Blob([]), 0, 0);
      onProgress?.(1);
      return;
    }
    let offset = 0;
    while (offset < file.size) {
      const end = Math.min(offset + CHUNK_SIZE, file.size);
      await this.putChunk(url, file.slice(offset, end), offset, file.size);
      offset = end;
      onProgress?.(offset / file.size);
    }
  }

  private async putChunk(url: string, chunk: Blob, start: number, total: number): Promise<void> {
    const headers: Record<string, string> = {
      [this.csrfHeader]: this.token,
      "content-type": "application/octet-stream",
      "content-range": total === 0 ? "bytes */0" : `bytes ${start}-${start + chunk.size - 1}/${total}`,
    };
    const response = await fetch(url, {
      method: "PUT",
      headers,
      body: chunk,
      credentials: "same-origin",
    });
    if (!response.ok) throw new ApiError(await readError(response), response.status);
  }

  async mutate(body: MutationBody): Promise<void> {
    const response = await fetch(`${this.base}/api/fs`, {
      method: "PATCH",
      headers: { "content-type": "application/json", [this.csrfHeader]: this.token },
      body: JSON.stringify(body),
      credentials: "same-origin",
    });
    if (!response.ok) throw new ApiError(await readError(response), response.status);
  }

  async remove(path: string): Promise<void> {
    const response = await fetch(`${this.base}/api/fs?path=${encodeURIComponent(path)}`, {
      method: "DELETE",
      headers: { [this.csrfHeader]: this.token },
      credentials: "same-origin",
    });
    if (!response.ok) throw new ApiError(await readError(response), response.status);
  }

  private async getJson<T>(url: string): Promise<T> {
    const response = await fetch(url, { credentials: "same-origin" });
    if (!response.ok) throw new ApiError(await readError(response), response.status);
    return (await response.json()) as T;
  }
}

export function joinPath(directory: string, name: string): string {
  if (directory.endsWith("/")) return `${directory}${name}`;
  return `${directory}/${name}`;
}

export function parentPath(path: string): string | null {
  if (path === "/") return null;
  const trimmed = path.replace(/\/+$/, "");
  const index = trimmed.lastIndexOf("/");
  if (index <= 0) return "/";
  return trimmed.slice(0, index);
}

export function formatBytes(bytes: number | null): string {
  if (bytes === null || Number.isNaN(bytes)) return "";
  const units = ["B", "K", "M", "G", "T", "P"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  const rounded = value >= 10 || unit === 0 ? value.toFixed(0) : value.toFixed(1);
  return `${rounded}${units[unit]}`;
}

export function formatTime(seconds: number | null): string {
  if (seconds === null) return "";
  const date = new Date(seconds * 1000);
  if (Number.isNaN(date.getTime())) return "";
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}`;
}
