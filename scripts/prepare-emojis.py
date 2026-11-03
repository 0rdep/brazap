#!/usr/bin/env python3
"""Rebuild the offline emoji artwork/search catalog from pinned upstream data."""
from pathlib import Path
import hashlib
import io
import json
import re
import subprocess
import sys
import tarfile
import urllib.request
import xml.etree.ElementTree as ET

root = Path(__file__).resolve().parent.parent
out = root / "assets" / "emoji"
out.mkdir(parents=True, exist_ok=True)
sources = {
    "twemoji": "https://codeload.github.com/jdecked/twemoji/tar.gz/b6b55fef1e8636b540a6d016a4729ca8cdf2e60b",
    "unicode": "https://unicode.org/Public/17.0.0/emoji/emoji-test.txt",
    "portuguese": "https://raw.githubusercontent.com/unicode-org/cldr/release-48/common/annotations/pt.xml",
    "portuguese_derived": "https://raw.githubusercontent.com/unicode-org/cldr/release-48/common/annotationsDerived/pt.xml",
    "unicode_license": "https://www.unicode.org/license.txt",
}
def fetch(url):
    with urllib.request.urlopen(url, timeout=120) as response:
        return response.read()

data = {name: fetch(url) for name, url in sources.items()}
art = {}
with tarfile.open(fileobj=io.BytesIO(data["twemoji"]), mode="r:gz") as tar:
    for entry in tar:
        if "/assets/72x72/" in entry.name and entry.name.endswith(".png"):
            art[Path(entry.name).stem] = tar.extractfile(entry).read()
        if entry.name.endswith("/LICENSE-GRAPHICS"):
            (out / "LICENSE-GRAPHICS").write_bytes(tar.extractfile(entry).read())
(out / "UNICODE-LICENSE").write_bytes(data["unicode_license"])
keywords = {}
for name in ["portuguese", "portuguese_derived"]:
    for entry in ET.fromstring(data[name]).iter("annotation"):
        if entry.text:
            keywords.setdefault(entry.attrib["cp"], []).append(entry.text)
index = []
packed = bytearray()
group = ""
for line in data["unicode"].decode().splitlines():
    if line.startswith("# group:"):
        group = line.split(":", 1)[1].strip()
    match = re.match(r"^([0-9A-F ]+); fully-qualified\s+# (\S+) E[\d.]+ (.+)$", line)
    if not match:
        continue
    points, emoji, name = match.groups()
    code = "-".join(part.lower() for part in points.split())
    image = art.get(code) or art.get(code.replace("-fe0f", ""))
    if not image:
        continue
    offset = len(packed)
    packed.extend(image)
    index.append({"text": emoji, "name": name, "search": (name + " " + " ".join(keywords.get(emoji, []))).lower(), "group": group, "offset": offset, "length": len(image)})
(out / "art.bin").write_bytes(packed)
(out / "index.json").write_text(json.dumps(index, ensure_ascii=False, separators=(",", ":")))
(out / "sources.json").write_text(json.dumps({name: {"url": url, "sha256": hashlib.sha256(data[name]).hexdigest()} for name,url in sources.items()}, indent=2)+"\n")
print(f"Packed {len(index)} emoji, {len(packed):,} artwork bytes")

# Keep text rendering and the picker on the same artwork revision.
subprocess.run([sys.executable, str(root / "scripts/prepare-emoji-font.py")], check=True)
