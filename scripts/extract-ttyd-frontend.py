#!/usr/bin/env python3
"""Extract the frontend bundle ttyd ships inside src/html.h.

ttyd embeds its built client as a gzipped C array. Serving that exact bundle from
fttyd (via `-I`) is the strongest available conformance test: it is the client real
ttyd users run in their browsers, byte for byte.

Usage: scripts/extract-ttyd-frontend.py [ttyd-repo] [output]
"""

import gzip
import pathlib
import re
import sys


def main() -> int:
    repo = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else "/tmp/ttyd")
    output = pathlib.Path(sys.argv[2] if len(sys.argv) > 2 else "e2e/.cache/ttyd-client.html")

    header = repo / "src" / "html.h"
    if not header.is_file():
        print(
            f"error: {header} not found (clone https://github.com/tsl0922/ttyd there)",
            file=sys.stderr,
        )
        return 1

    source = header.read_text()
    body = source.split("{", 1)[1].split("}", 1)[0]
    packed = bytes(int(value, 16) for value in re.findall(r"0x([0-9a-fA-F]{2})", body))
    html = gzip.decompress(packed)

    expected = int(re.search(r"index_html_size\s*=\s*(\d+)", source).group(1))
    if len(html) != expected:
        print(f"error: decompressed {len(html)} bytes, expected {expected}", file=sys.stderr)
        return 1

    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_bytes(html)
    print(f"wrote {output} ({len(html)} bytes)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
