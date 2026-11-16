// The sidebar file tree: lazy loading, keyboard navigation, downloads, uploads and
// mutations. Rows are windowed so a directory with a hundred thousand entries stays
// responsive instead of building a hundred thousand DOM nodes.

import {
  Api,
  ApiError,
  formatBytes,
  formatTime,
  joinPath,
  parentPath,
  type Entry,
} from "./api";
import { askConfirm, askText } from "./modal";
// Material Icon Theme (MIT): https://github.com/material-extensions/vscode-material-icon-theme
import lockedFolderIcon from "./icons/folder-admin.svg?url";
import folderIcon from "./icons/folder-base.svg?url";
import hiddenIcon from "./icons/folder-controller.svg?url";
import newFolderIcon from "./icons/folder-include.svg?url";
import parentIcon from "./icons/folder-interface.svg?url";
import followIcon from "./icons/folder-routes.svg?url";
import refreshIcon from "./icons/folder-update.svg?url";
import uploadIcon from "./icons/folder-upload.svg?url";
import fileIcon from "./icons/document.svg?url";

const ROW_HEIGHT = 22;
/** Rows rendered beyond the viewport on each side. */
const OVERSCAN = 6;

export interface TreeOptions {
  api: Api;
  writable: boolean;
  root: string;
  onPathChange?: (path: string) => void;
  /** Types `cd` into the attached terminal. */
  onOpenTerminal?: (path: string) => void;
}

interface TreeNode {
  path: string;
  name: string;
  kind: Entry["kind"];
  depth: number;
  entry: Entry | null;
  expanded: boolean;
  children: TreeNode[] | null;
  loading: boolean;
  error: string | null;
}

function nodeFromEntry(entry: Entry, depth: number): TreeNode {
  return {
    path: entry.path ?? "",
    name: entry.name,
    kind: entry.kind,
    depth,
    entry,
    expanded: false,
    children: null,
    loading: false,
    error: entry.error,
  };
}

export class FileTree {
  private readonly toolbar: HTMLElement;
  private readonly list: HTMLElement;
  private readonly status: HTMLElement;
  private readonly nodeByPath = new Map<string, TreeNode>();
  private root: TreeNode;
  private selected: string | null = null;
  private showHidden = false;
  private followEnabled = true;
  private lastShellCwd: string | null = null;
  private followButton: HTMLButtonElement | null = null;
  private renderQueued = false;

  constructor(
    container: HTMLElement,
    private readonly options: TreeOptions,
  ) {
    const element = document.createElement("div");
    element.className = "file-tree";
    container.appendChild(element);
    this.root = {
      path: options.root,
      name: options.root,
      kind: "dir",
      depth: 0,
      entry: null,
      expanded: true,
      children: null,
      loading: false,
      error: null,
    };
    this.nodeByPath.set(this.root.path, this.root);

    this.toolbar = document.createElement("div");
    this.toolbar.className = "tree-toolbar";
    this.list = document.createElement("div");
    this.list.className = "tree-list";
    this.list.tabIndex = 0;
    this.list.dataset.testid = "file-tree";
    this.status = document.createElement("div");
    this.status.className = "tree-status";

    element.append(this.toolbar, this.list, this.status);
    this.buildToolbar();
    this.installHandlers();
  }

  get writable(): boolean {
    return this.options.writable;
  }

  async start(): Promise<void> {
    await this.loadChildren(this.root);
    this.selected = this.root.path;
    this.render();
  }

  /** Directory that toolbar actions and drops apply to. */
  targetDirectory(): string {
    const node = this.selected ? this.nodeByPath.get(this.selected) : undefined;
    if (!node) return this.root.path;
    if (node.kind === "dir") return node.path;
    return parentPath(node.path) ?? this.root.path;
  }

  /** Called when the server reports a new shell working directory. */
  async follow(path: string): Promise<void> {
    this.lastShellCwd = path;
    if (!this.followEnabled) return;
    if (this.selected === path) return;
    const node = await this.reveal(path);
    if (!node) {
      // The tree was rebased somewhere that does not contain the shell's directory.
      // Anchor it at the shell instead, which is what "follow" promises.
      await this.rebase(path, path);
      return;
    }
    this.selected = node.path;
    this.render();
    this.scrollIntoView(node.path);
    this.options.onPathChange?.(this.targetDirectory());
  }

  /** Re-enables following and jumps to the shell's directory. */
  async resumeFollowing(): Promise<void> {
    this.followEnabled = true;
    this.paintFollowButton();
    if (this.lastShellCwd) await this.follow(this.lastShellCwd);
    this.paintSelection();
  }

  private disableFollowing(): void {
    if (!this.followEnabled) return;
    this.followEnabled = false;
    this.paintFollowButton();
  }

  /** Expands every ancestor of `path` and returns the node itself when reachable. */
  private async reveal(path: string): Promise<TreeNode | null> {
    if (path !== this.root.path && !path.startsWith(`${this.root.path.replace(/\/+$/, "")}/`)) {
      return null;
    }
    const suffix = this.root.path === "/" ? path.slice(1) : path.slice(this.root.path.length + 1);
    const parts = suffix.split("/").filter(Boolean);
    let current = this.root;
    for (const part of parts) {
      if (current.children === null) await this.loadChildren(current);
      const child = current.children?.find((candidate) => candidate.name === part);
      if (!child) return current;
      if (child.kind !== "dir") return child;
      child.expanded = true;
      current = child;
    }
    // Expanding the target without its children would render a caret that toggles
    // nothing, so load them here.
    if (current.kind === "dir" && current.children === null) await this.loadChildren(current);
    current.expanded = true;
    return current;
  }

  async refresh(path?: string): Promise<void> {
    const target = path ?? this.root.path;
    const node = this.nodeByPath.get(target);
    if (!node || node.kind !== "dir") return;
    node.children = null;
    await this.loadChildren(node);
    if (path === undefined) this.selected = this.root.path;
    this.render();
  }

  private buildToolbar(): void {
    const button = (icon: string, title: string, action: () => void): HTMLButtonElement => {
      const element = document.createElement("button");
      element.type = "button";
      element.className = "btn icon";
      element.title = title;
      element.dataset.tooltip = title;
      // The glyph used to supply the accessible name, so it has to be spelled out now.
      element.setAttribute("aria-label", title);
      const image = document.createElement("img");
      image.className = "btn-icon";
      image.src = icon;
      image.alt = "";
      image.width = 16;
      image.height = 16;
      element.appendChild(image);
      element.addEventListener("click", action);
      return element;
    };

    const up = button(parentIcon, "Parent directory", () => void this.goUp());
    up.dataset.testid = "up";
    const refresh = button(refreshIcon, "Refresh", () => void this.refresh(this.root.path));
    const hidden = button(hiddenIcon, "Toggle hidden files", () => {
      this.showHidden = !this.showHidden;
      hidden.classList.toggle("active", this.showHidden);
      this.render();
    });
    hidden.dataset.testid = "toggle-hidden";

    this.toolbar.append(up, refresh, hidden);

    if (this.options.writable) {
      const mkdir = button(newFolderIcon, "New folder", () => void this.createFolder());
      mkdir.dataset.testid = "new-folder";
      const upload = button(uploadIcon, "Upload files", () => this.pickFiles());
      upload.dataset.testid = "upload";
      this.toolbar.append(mkdir, upload);
    }

    const follow = button(followIcon, "Follow the shell's working directory", () => {
      if (this.followEnabled) {
        this.disableFollowing();
      } else {
        void this.resumeFollowing();
      }
    });
    follow.dataset.testid = "follow";
    this.followButton = follow;
    this.paintFollowButton();
    this.toolbar.append(follow);

    const spacer = document.createElement("span");
    spacer.className = "tree-toolbar-spacer";
    this.toolbar.appendChild(spacer);
  }

  private installHandlers(): void {
    this.list.addEventListener("click", (event) => {
      const row = (event.target as HTMLElement).closest<HTMLElement>(".file-row");
      if (!row) return;
      const path = row.dataset.path;
      if (!path) return;
      if ((event.target as HTMLElement).classList.contains("row-caret")) {
        void this.toggle(path);
        return;
      }
      // Repainting the selection in place (instead of rebuilding the list) keeps the
      // element under the pointer alive, which is what makes double-click work.
      this.selected = path;
      this.disableFollowing();
      this.paintSelection();
      this.options.onPathChange?.(this.targetDirectory());
    });

    this.list.addEventListener("dblclick", (event) => {
      const row = (event.target as HTMLElement).closest<HTMLElement>(".file-row");
      const path = row?.dataset.path;
      if (!path) return;
      const node = this.nodeByPath.get(path);
      if (node?.kind === "dir") void this.toggle(path);
    });

    this.list.addEventListener("keydown", (event) => this.onKeydown(event));

    // Rows outside the window are not in the DOM, so scrolling has to repaint or the
    // user scrolls into blank spacers.
    this.list.addEventListener("scroll", () => this.scheduleRender());

    this.list.addEventListener("contextmenu", (event) => {
      const row = (event.target as HTMLElement).closest<HTMLElement>(".file-row");
      const path = row?.dataset.path;
      if (!path) return;
      event.preventDefault();
      this.selected = path;
      this.render();
      this.showContextMenu(path, event.clientX, event.clientY);
    });

    this.list.addEventListener("dragover", (event) => {
      if (!this.options.writable) return;
      event.preventDefault();
      this.list.classList.add("drop-target");
    });
    this.list.addEventListener("dragleave", () => this.list.classList.remove("drop-target"));
    this.list.addEventListener("drop", (event) => {
      this.list.classList.remove("drop-target");
      if (!this.options.writable) return;
      event.preventDefault();
      const target = this.selected ? this.nodeByPath.get(this.selected) : undefined;
      const directory = target?.kind === "dir" ? target.path : this.targetDirectory();
      const items = event.dataTransfer?.items;
      if (items && items.length > 0) {
        void this.uploadDataTransfer(items, directory);
      }
    });
  }

  private onKeydown(event: KeyboardEvent): void {
    const paths = this.visibleNodes().map((node) => node.path);
    if (paths.length === 0) return;
    const index = this.selected ? paths.indexOf(this.selected) : -1;
    const move = (delta: number) => {
      this.disableFollowing();
      const next = Math.max(0, Math.min(paths.length - 1, index + delta));
      this.selected = paths[next];
      this.scrollIntoView(this.selected);
      this.render();
      this.options.onPathChange?.(this.targetDirectory());
    };

    switch (event.key) {
      case "ArrowDown":
        event.preventDefault();
        move(1);
        break;
      case "ArrowUp":
        event.preventDefault();
        move(-1);
        break;
      case "ArrowRight": {
        event.preventDefault();
        const node = this.selected ? this.nodeByPath.get(this.selected) : undefined;
        if (node?.kind === "dir") {
          if (!node.expanded) void this.toggle(node.path);
          else move(1);
        }
        break;
      }
      case "ArrowLeft": {
        event.preventDefault();
        const node = this.selected ? this.nodeByPath.get(this.selected) : undefined;
        if (node?.expanded) void this.toggle(node.path);
        else void this.goUp();
        break;
      }
      case "Enter": {
        event.preventDefault();
        const node = this.selected ? this.nodeByPath.get(this.selected) : undefined;
        if (node?.kind === "dir") void this.toggle(node.path);
        else if (node) this.download(node, "file");
        break;
      }
      case "Backspace":
        event.preventDefault();
        void this.goUp();
        break;
      case "Delete": {
        event.preventDefault();
        const node = this.selected ? this.nodeByPath.get(this.selected) : undefined;
        if (node && node !== this.root) void this.remove(node);
        break;
      }
      default:
        break;
    }
  }

  private async goUp(): Promise<void> {
    // Navigating by hand pins the tree, like clicking a row does.
    this.disableFollowing();
    const current = this.selected ? this.nodeByPath.get(this.selected) : undefined;
    const from = current ?? this.root;
    const parent = parentPath(from.path);
    if (!parent) return;

    const node = this.nodeByPath.get(parent);
    if (node) {
      this.selected = node.path;
      if (!node.expanded) await this.toggle(node.path);
      this.toggleExpandedAncestors(parent);
      this.render();
      this.options.onPathChange?.(this.targetDirectory());
      return;
    }
    if (parent === this.root.path) {
      this.selected = this.root.path;
      this.render();
      return;
    }

    // Above the tree's anchor: ask the server. It answers 403 when a --file-root stops
    // us, and the listing otherwise, so the browser can climb to whatever it is allowed
    // to see instead of silently refusing to move.
    await this.rebase(parent, from.path);
  }

  /**
   * Moves the tree's anchor to `newRoot`, keeping the directory we came from expanded
   * and selected so the climb stays oriented.
   */
  private async rebase(newRoot: string, cameFrom: string): Promise<void> {
    let listing;
    try {
      listing = await this.options.api.list(newRoot);
    } catch (error) {
      this.notify(errorText(error), "error");
      return;
    }

    const previous = this.root;
    this.nodeByPath.clear();
    this.root = {
      path: newRoot,
      name: newRoot,
      kind: "dir",
      depth: 0,
      entry: null,
      expanded: true,
      children: null,
      loading: false,
      error: null,
    };
    this.nodeByPath.set(this.root.path, this.root);
    this.root.children = listing.entries
      .filter((entry) => entry.path !== null)
      .map((entry) => nodeFromEntry(entry, 1));

    // Carry over what was already loaded, so expanding a directory again does not
    // refetch it and the user keeps their place.
    const carried =
      previous.path === cameFrom
        ? this.root.children.find((child) => child.path === previous.path)
        : undefined;
    if (carried) {
      carried.children = previous.children;
      carried.expanded = true;
    }
    // Every child needs registering, not just the carried one: rows the climb revealed
    // are resolved through this map for clicks, expansion and context menus.
    for (const child of this.root.children) this.reindex(child, 1);

    // Select the directory we moved into, not the one we came from: "up" should leave
    // the selection on the parent so the next press climbs another level.
    this.selected = this.root.path;
    this.list.scrollTop = 0;
    this.render();
    this.options.onPathChange?.(this.targetDirectory());
  }

  /** Re-registers a subtree after the anchor moved, fixing depths and lookups. */
  private reindex(node: TreeNode, depth: number): void {
    node.depth = depth;
    this.nodeByPath.set(node.path, node);
    for (const child of node.children ?? []) this.reindex(child, depth + 1);
  }

  private toggleExpandedAncestors(path: string): void {
    let current = parentPath(path);
    while (current) {
      const node = this.nodeByPath.get(current);
      if (node) node.expanded = true;
      current = parentPath(current);
    }
  }

  private async toggle(path: string): Promise<void> {
    const node = this.nodeByPath.get(path);
    if (!node || node.kind !== "dir") return;
    if (!node.expanded) {
      if (node.children === null) await this.loadChildren(node);
      node.expanded = true;
      this.selected = node.path;
    } else {
      node.expanded = false;
    }
    this.render();
    this.options.onPathChange?.(this.targetDirectory());
  }

  private async loadChildren(node: TreeNode): Promise<void> {
    if (node.loading) return;
    node.loading = true;
    try {
      const listing = await this.options.api.list(node.path);
      const addressable = listing.entries.filter((entry) => entry.path !== null);
      const hidden = listing.entries.length - addressable.length;
      node.children = addressable.map((entry) => nodeFromEntry(entry, node.depth + 1));
      if (hidden > 0) {
        // Names that are not valid UTF-8 cannot round trip through JSON, so they are
        // not shown. Saying so beats silently pretending they do not exist.
        this.notify(`${hidden} entr${hidden === 1 ? "y" : "ies"} with non-UTF-8 names not shown`);
      }
      node.error = null;
      for (const child of node.children) {
        this.nodeByPath.set(child.path, child);
      }
    } catch (error) {
      node.error = error instanceof Error ? error.message : String(error);
      node.children = [];
      this.notify(node.error, "error");
    } finally {
      node.loading = false;
    }
  }

  private visibleNodes(): TreeNode[] {
    const rows: TreeNode[] = [];
    const walk = (node: TreeNode) => {
      for (const child of node.children ?? []) {
        if (!this.showHidden && child.name.startsWith(".")) continue;
        rows.push(child);
        if (child.expanded) walk(child);
      }
    };
    walk(this.root);
    return rows;
  }

  /** Updates the selection highlight without touching the row structure. */
  private paintFollowButton(): void {
    if (!this.followButton) return;
    this.followButton.classList.toggle("active", this.followEnabled);
    this.followButton.title = this.followEnabled
      ? "Following the shell's working directory (click to pin)"
      : "Pinned; click to follow the shell again";
  }

  private paintSelection(): void {
    for (const row of this.list.querySelectorAll<HTMLElement>(".file-row")) {
      row.classList.toggle("selected", row.dataset.path !== undefined && row.dataset.path === this.selected);
    }
  }

  private render(): void {
    this.renderQueued = false;
    const rows = this.visibleNodes();
    const scrollTop = this.list.scrollTop;
    const height = this.list.clientHeight || 400;
    const last = Math.min(rows.length, Math.ceil((scrollTop + height) / ROW_HEIGHT) + OVERSCAN);
    // Clamped to `last`: when the list shrinks under a scrolled viewport, an unclamped
    // `first` would render an empty window (and a spacer taller than the content).
    const first = Math.max(0, Math.min(last, Math.floor(scrollTop / ROW_HEIGHT) - OVERSCAN));

    const fragment = document.createDocumentFragment();
    if (first > 0) {
      const spacer = document.createElement("div");
      spacer.className = "row-spacer";
      spacer.style.height = `${first * ROW_HEIGHT}px`;
      fragment.appendChild(spacer);
    }
    for (const node of rows.slice(first, last)) {
      fragment.appendChild(this.renderRow(node));
    }
    if (last < rows.length) {
      const spacer = document.createElement("div");
      spacer.className = "row-spacer";
      spacer.style.height = `${(rows.length - last) * ROW_HEIGHT}px`;
      fragment.appendChild(spacer);
    }

    this.list.replaceChildren(fragment);
    const rootRow = this.renderRootRow();
    this.list.prepend(rootRow);
  }

  private renderRootRow(): HTMLElement {
    const row = document.createElement("div");
    row.className = "file-row root-row";
    row.dataset.path = this.root.path;
    row.dataset.kind = "dir";
    row.classList.toggle("selected", this.selected === this.root.path);

    const caret = document.createElement("span");
    caret.className = "row-caret";
    caret.textContent = this.root.expanded ? "\u25be" : "\u25b8";

    const name = document.createElement("span");
    name.className = "row-name";
    name.textContent = this.root.path;

    row.append(caret, this.rowIcon(this.root.error ? lockedFolderIcon : folderIcon), name);
    return row;
  }

  private renderRow(node: TreeNode): HTMLElement {
    const row = document.createElement("div");
    row.className = "file-row";
    row.dataset.path = node.path;
    row.dataset.kind = node.kind;
    row.style.paddingLeft = `${4 + node.depth * 12}px`;
    if (this.selected === node.path) row.classList.add("selected");
    // A directory we could not open is as unavailable as one whose metadata failed.
    const locked = Boolean(node.entry?.error) || node.error !== null;
    if (locked) row.classList.add("locked");

    const caret = document.createElement("span");
    caret.className = "row-caret";
    caret.dataset.testid = "caret";
    if (node.kind === "dir") {
      caret.textContent = node.expanded ? "\u25be" : "\u25b8";
    }

    const name = document.createElement("span");
    name.className = "row-name";
    name.textContent = node.kind === "dir" ? `${node.name}/` : node.name;
    if (node.kind === "symlink") name.classList.add("symlink");
    if (node.kind === "dir") name.classList.add("directory");
    if (node.entry?.symlink_target) name.title = `\u2192 ${node.entry.symlink_target}`;
    if (node.entry?.error) name.title = node.entry.error;

    const meta = document.createElement("span");
    meta.className = "row-meta";
    const parts: string[] = [];
    if (node.kind !== "dir") parts.push(formatBytes(node.entry?.size ?? null));
    if (node.entry?.mtime) parts.push(formatTime(node.entry.mtime));
    meta.textContent = parts.filter(Boolean).join("  ");

    const icon = node.kind === "dir" ? (locked ? lockedFolderIcon : folderIcon) : fileIcon;
    row.append(caret, this.rowIcon(icon), name, meta);
    if (node.error) row.title = node.error;
    return row;
  }

  private rowIcon(src: string): HTMLImageElement {
    const image = document.createElement("img");
    image.className = "row-icon";
    image.src = src;
    // The row's name already says what this is; the icon is decoration.
    image.alt = "";
    image.width = 14;
    image.height = 14;
    return image;
  }

  /** Coalesces scroll-driven repaints into one per frame. */
  private scheduleRender(): void {
    if (this.renderQueued) return;
    this.renderQueued = true;
    requestAnimationFrame(() => {
      if (this.renderQueued) this.render();
    });
  }

  private scrollIntoView(path: string): void {
    const rows = this.visibleNodes();
    const index = rows.findIndex((node) => node.path === path);
    if (index < 0) return;
    const top = (index + 1) * ROW_HEIGHT;
    const bottom = top + ROW_HEIGHT;
    const before = this.list.scrollTop;
    if (top < before) this.list.scrollTop = top;
    else if (bottom > before + this.list.clientHeight) {
      this.list.scrollTop = bottom - this.list.clientHeight;
    }
    // The row is often outside the current window, so scrolling alone would leave the
    // selection unrendered until the next scroll event.
    if (this.list.scrollTop !== before) this.render();
  }

  private showContextMenu(path: string, x: number, y: number): void {
    document.querySelectorAll(".context-menu").forEach((menu) => menu.remove());
    const node = this.nodeByPath.get(path);
    if (!node) return;

    const menu = document.createElement("div");
    menu.className = "context-menu";
    const item = (label: string, action: () => void, testId?: string) => {
      const button = document.createElement("button");
      button.type = "button";
      button.className = "context-item";
      if (testId) button.dataset.testid = testId;
      button.textContent = label;
      button.addEventListener("click", () => {
        menu.remove();
        action();
      });
      menu.appendChild(button);
    };

    if (node.kind === "dir") {
      item("Download as zip", () => this.download(node, "zip"), "download-zip");
      item("Download as tar.gz", () => this.download(node, "tar.gz"), "download-targz");
    } else {
      item("Download", () => this.download(node, "file"), "download-file");
    }

    if (this.options.onOpenTerminal && this.options.writable && node.kind === "dir") {
      item("Open terminal here", () => this.options.onOpenTerminal?.(node.path), "open-terminal");
    }

    if (this.options.writable) {
      item("Rename...", () => void this.rename(node), "rename");
      item("Permissions...", () => void this.chmod(node), "chmod");
      const remove = document.createElement("button");
      remove.type = "button";
      remove.className = "context-item danger";
      remove.dataset.testid = "delete";
      remove.textContent = "Delete";
      remove.addEventListener("click", () => {
        menu.remove();
        void this.remove(node);
      });
      menu.appendChild(remove);
    }

    menu.style.left = `${Math.min(x, window.innerWidth - 200)}px`;
    menu.style.top = `${Math.min(y, window.innerHeight - 160)}px`;
    document.body.appendChild(menu);
    this.closeMenuOnOutsideClick(menu);
  }

  private closeMenuOnOutsideClick(menu: HTMLElement): void {
    const dispose = (event: Event) => {
      // Clicks inside the menu must reach their item: removing it on pointerdown
      // would detach the button before the click event fires.
      const target = event.target;
      if (target instanceof globalThis.Node && menu.contains(target)) return;
      menu.remove();
      window.removeEventListener("pointerdown", dispose, true);
      window.removeEventListener("keydown", onKey, true);
    };
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") dispose(event);
    };
    window.addEventListener("pointerdown", dispose, true);
    window.addEventListener("keydown", onKey, true);
  }

  private download(node: TreeNode, format: "file" | "zip" | "tar.gz"): void {
    const url =
      format === "file"
        ? this.options.api.downloadUrl(node.path)
        : this.options.api.archiveUrl(node.path, format);
    const link = document.createElement("a");
    link.href = url;
    link.download = "";
    document.body.appendChild(link);
    link.click();
    link.remove();
    this.notify(`Downloading ${node.name}${format === "file" ? "" : `.${format}`}`);
  }

  private async createFolder(): Promise<void> {
    const name = await askText({ title: "New folder", label: "Folder name", confirmLabel: "Create" });
    if (!name) return;
    const target = joinPath(this.targetDirectory(), name);
    try {
      await this.options.api.mutate({ op: "mkdir", path: target });
      await this.refresh(this.targetDirectory());
      this.notify(`Created ${target}`);
    } catch (error) {
      this.notify(errorText(error), "error");
    }
  }

  private async rename(node: TreeNode): Promise<void> {
    const name = await askText({
      title: "Rename",
      label: `New name for ${node.name}`,
      value: node.name,
      confirmLabel: "Rename",
    });
    if (!name || name === node.name) return;
    const directory = parentPath(node.path);
    if (!directory) return;
    try {
      await this.options.api.mutate({ op: "rename", path: node.path, to: joinPath(directory, name) });
      const parent = parentPath(node.path);
      if (parent) await this.refresh(parent);
      this.notify(`Renamed to ${name}`);
    } catch (error) {
      this.notify(errorText(error), "error");
    }
  }

  private async chmod(node: TreeNode): Promise<void> {
    const current = node.entry?.mode_octal ?? 0o644;
    const value = await askText({
      title: "Permissions",
      label: `Octal mode for ${node.name}`,
      value: current.toString(8).padStart(3, "0"),
      confirmLabel: "Apply",
    });
    if (!value) return;
    try {
      await this.options.api.mutate({ op: "chmod", path: node.path, mode: value });
      const parent = parentPath(node.path);
      if (parent) await this.refresh(parent);
      this.notify(`Mode of ${node.name} set to ${value}`);
    } catch (error) {
      this.notify(errorText(error), "error");
    }
  }

  private async remove(node: TreeNode): Promise<void> {
    const confirmed = await askConfirm({
      title: "Delete",
      message: `Delete ${node.path}? This cannot be undone.`,
      confirmLabel: "Delete",
      danger: true,
    });
    if (!confirmed) return;
    try {
      await this.options.api.remove(node.path);
      const parent = parentPath(node.path);
      if (parent) await this.refresh(parent);
      this.notify(`Deleted ${node.path}`);
    } catch (error) {
      this.notify(errorText(error), "error");
    }
  }

  private pickFiles(): void {
    const input = document.createElement("input");
    input.type = "file";
    input.multiple = true;
    input.addEventListener("change", () => {
      if (input.files) void this.uploadFiles(Array.from(input.files), this.targetDirectory());
    });
    input.click();
  }

  private async uploadDataTransfer(items: DataTransferItemList, directory: string): Promise<void> {
    const files: { file: File; relativePath: string }[] = [];
    const walkers: Promise<void>[] = [];

    const readEntry = async (entry: FileSystemEntry, prefix: string): Promise<void> => {
      if (entry.isFile) {
        const file = await new Promise<File | null>((resolve) => {
          (entry as FileSystemFileEntry).file(
            (result) => resolve(result),
            () => resolve(null),
          );
        });
        if (file) files.push({ file, relativePath: prefix ? `${prefix}/${file.name}` : file.name });
        return;
      }
      if (!entry.isDirectory) return;
      const reader = (entry as FileSystemDirectoryEntry).createReader();
      const children = await new Promise<FileSystemEntry[]>((resolve) => {
        const collected: FileSystemEntry[] = [];
        const readBatch = () => {
          reader.readEntries(
            (batch) => {
              if (batch.length === 0) {
                resolve(collected);
                return;
              }
              collected.push(...batch);
              readBatch();
            },
            () => resolve(collected),
          );
        };
        readBatch();
      });
      const nextPrefix = prefix ? `${prefix}/${entry.name}` : entry.name;
      await Promise.all(children.map((child) => readEntry(child, nextPrefix)));
    };

    for (const item of Array.from(items)) {
      const entry = item.webkitGetAsEntry?.();
      if (entry) walkers.push(readEntry(entry, ""));
      else {
        const file = item.getAsFile();
        if (file) files.push({ file, relativePath: file.name });
      }
    }
    await Promise.all(walkers);
    if (files.length === 0) return;

    for (const item of files) {
      const target = joinPath(directory, item.relativePath);
      const parent = parentPath(target);
      if (parent) await this.ensureDirectory(parent, directory);
    }
    await this.uploadFiles(
      files.map((item) => item.file),
      directory,
      files,
    );
  }

  /** Creates every missing directory between `root` and `path`. */
  private async ensureDirectory(path: string, root: string): Promise<void> {
    if (!path.startsWith(root)) return;
    const relative = path.slice(root.length).split("/").filter(Boolean);
    let current = root;
    for (const part of relative) {
      current = joinPath(current, part);
      try {
        await this.options.api.mutate({ op: "mkdir", path: current });
      } catch {
        // Already exists: the only acceptable failure here.
      }
    }
  }

  private async uploadFiles(
    files: File[],
    directory: string,
    originals?: { file: File; relativePath: string }[],
  ): Promise<void> {
    let index = 0;
    for (const file of files) {
      const relative = originals?.[index]?.relativePath ?? file.name;
      index += 1;
      const target = joinPath(directory, relative);
      const parent = parentPath(target) ?? directory;
      this.notify(`Uploading ${relative} (0%)`);
      try {
        await this.options.api.upload(parent, new File([file], basename(relative), { type: file.type }), (fraction) =>
          this.notify(`Uploading ${relative} (${Math.round(fraction * 100)}%)`),
        );
      } catch (error) {
        this.notify(`${relative}: ${errorText(error)}`, "error");
        continue;
      }
      this.notify(`Uploaded ${relative}`);
    }
    await this.refresh(directory);
  }

  private notify(message: string, kind: "info" | "error" = "info"): void {
    this.status.textContent = message;
    this.status.dataset.kind = kind;
  }
}

function basename(path: string): string {
  const index = path.lastIndexOf("/");
  return index === -1 ? path : path.slice(index + 1);
}

function errorText(error: unknown): string {
  if (error instanceof ApiError) return error.message;
  if (error instanceof Error) return error.message;
  return String(error);
}
