// CPU, memory and disk gauges drawn with Unicode blocks so they match the terminal
// beside them without needing an icon font.

import { formatBytes, type DiskUsage, type StatsSnapshot } from "./api";

/** Cells in a gauge bar; a monospace cell count keeps every row aligned. */
const CELLS = 16;
const FULL = "\u2588"; // █
const EMPTY = "\u2591"; // ░

export interface GaugeValues {
  cpu: number;
  memUsed: number;
  memTotal: number;
  diskUsed: number;
  diskTotal: number;
}

function bar(percent: number): string {
  const clamped = Math.max(0, Math.min(100, percent));
  const filled = Math.round((clamped / 100) * CELLS);
  return FULL.repeat(filled) + EMPTY.repeat(CELLS - filled);
}

function severity(percent: number): string {
  if (percent >= 90) return "critical";
  if (percent >= 75) return "warn";
  return "ok";
}

/** Picks the mount that contains `path`, which is what the user cares about. */
export function diskForPath(disks: DiskUsage[], path: string): DiskUsage | undefined {
  const candidates = disks
    .filter((disk) => path === disk.mount || path.startsWith(disk.mount.endsWith("/") ? disk.mount : `${disk.mount}/`))
    .sort((left, right) => right.mount.length - left.mount.length);
  return candidates[0] ?? disks.find((disk) => disk.mount === "/") ?? disks[0];
}

export function gaugeValues(stats: StatsSnapshot, path: string): GaugeValues {
  const disk = diskForPath(stats.disks, path);
  const diskTotal = disk?.total ?? 0;
  const diskUsed = disk ? Math.max(0, disk.total - disk.available) : 0;
  return {
    cpu: stats.cpu,
    memUsed: stats.mem_used,
    memTotal: stats.mem_total,
    diskUsed,
    diskTotal,
  };
}

export class Gauges {
  private readonly cpuBar: HTMLElement;
  private readonly cpuText: HTMLElement;
  private readonly memBar: HTMLElement;
  private readonly memText: HTMLElement;
  private readonly diskBar: HTMLElement;
  private readonly diskText: HTMLElement;
  private readonly footer: HTMLElement;

  constructor(container: HTMLElement) {
    container.classList.add("gauges");
    const row = (label: string) => {
      const wrapper = document.createElement("div");
      wrapper.className = "gauge";
      const name = document.createElement("span");
      name.className = "gauge-label";
      name.textContent = label;
      const barElement = document.createElement("span");
      barElement.className = "gauge-bar";
      const text = document.createElement("span");
      text.className = "gauge-text";
      wrapper.append(name, barElement, text);
      container.appendChild(wrapper);
      return { barElement, text };
    };

    ({ barElement: this.cpuBar, text: this.cpuText } = row("cpu"));
    ({ barElement: this.memBar, text: this.memText } = row("mem"));
    ({ barElement: this.diskBar, text: this.diskText } = row("disk"));

    this.footer = document.createElement("div");
    this.footer.className = "gauge-footer";
    container.appendChild(this.footer);
  }

  update(stats: StatsSnapshot, path: string): void {
    const values = gaugeValues(stats, path);
    const memPercent = values.memTotal > 0 ? (values.memUsed / values.memTotal) * 100 : 0;
    const diskPercent = values.diskTotal > 0 ? (values.diskUsed / values.diskTotal) * 100 : 0;

    this.render(this.cpuBar, this.cpuText, values.cpu, `${values.cpu.toFixed(0)}%`);
    this.render(
      this.memBar,
      this.memText,
      memPercent,
      `${formatBytes(values.memUsed)}/${formatBytes(values.memTotal)}`,
    );
    this.render(
      this.diskBar,
      this.diskText,
      diskPercent,
      `${formatBytes(values.diskUsed)}/${formatBytes(values.diskTotal)}`,
    );

    const load = `${stats.load_one.toFixed(2)} ${stats.load_five.toFixed(2)} ${stats.load_fifteen.toFixed(2)}`;
    this.footer.textContent = `load ${load}  up ${formatUptime(stats.uptime)}  ${stats.cpu_count}c`;
    this.footer.title = "load average (1m 5m 15m), uptime, core count";
  }

  private render(barElement: HTMLElement, text: HTMLElement, percent: number, label: string): void {
    barElement.textContent = bar(percent);
    barElement.dataset.level = severity(percent);
    text.textContent = label;
  }
}

export function formatUptime(seconds: number): string {
  const days = Math.floor(seconds / 86_400);
  const hours = Math.floor((seconds % 86_400) / 3_600);
  const minutes = Math.floor((seconds % 3_600) / 60);
  if (days > 0) return `${days}d${hours}h`;
  if (hours > 0) return `${hours}h${minutes}m`;
  return `${minutes}m`;
}
