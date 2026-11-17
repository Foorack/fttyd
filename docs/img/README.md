# Screenshots

Every image here comes from the real application, driven by Playwright - no mockups, no compositing.

**Nothing confidential may appear in any of them.** These files are published on GitHub, so the rule
is absolute: no real username, home directory, hostname, uptime, process table, IP address or
credential. Take the shots against a synthetic fixture instead of the machine you happen to be on.

## How they were made

```sh
# 1. A fixture that looks like a plausible project but is entirely invented.
mkdir -p /tmp/fttyd-demo/{src,docs,assets,scripts}
cd /tmp/fttyd-demo && git init -q -b main
git -c user.name="Demo Maintainer" -c user.email="demo@example.com" commit -q --allow-empty -m "init"

# 2. Serve it, confined to it, with a DOM renderer (headless WebGL paints nothing) and a
#    neutral prompt. --file-root keeps the tree inside the fixture.
PS1='demo@workstation:\w$ ' target/debug/fttyd -p 18240 -W \
  -w /tmp/fttyd-demo --file-root /tmp/fttyd-demo \
  -t 'rendererType="dom"' bash --norc --noprofile
```

Then a Playwright script (kept in `/tmp`, not committed) opened the page at 1280×800 with
`deviceScaleFactor: 2`, typed a demo command, expanded `src/` in the tree, and captured:

| File | What it shows |
|---|---|
| `hero.png` | the whole window: terminal on the left, gauges and tree on the right |
| `sidebar.png` | the sidebar alone, cropped to the pane |
| `menu.png` | a folder's context menu: download as zip/tar.gz, open terminal here, rename, permissions, delete |
| `mobile.png` | 480px viewport with the terminal full width |

Keep each under ~400 KB; `convert -strip` removes metadata that is not worth shipping.

## Before you commit a screenshot

Look at it. Specifically:

- Search the image for your own username, hostname, home path and any domain you work for.
- A `ps`/`htop`/`top` capture is never acceptable - it leaks other users' command lines.
- The `load … up N days` line under the gauges shows real uptime; a synthetic demo script is fine,
  a real one is not.
- Check the file tree: it should be the fixture, not `/etc` or your home directory.

If you find a leak after pushing, the image stays in git history until it is rewritten - so check
first.
