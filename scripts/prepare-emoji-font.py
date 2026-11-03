#!/usr/bin/env python3
"""Pack our existing Twemoji PNGs into a color font (requires fonttools)."""
import json
from pathlib import Path

from fontTools.feaLib.builder import addOpenTypeFeaturesFromString
from fontTools.fontBuilder import FontBuilder
from fontTools.pens.ttGlyphPen import TTGlyphPen
from fontTools.ttLib import newTable
from fontTools.ttLib.tables._c_m_a_p import CmapSubtable
from fontTools.ttLib.tables.sbixGlyph import Glyph
from fontTools.ttLib.tables.sbixStrike import Strike

root = Path(__file__).resolve().parent.parent
directory = root / "assets/emoji"
catalog = json.loads((directory / "index.json").read_text())
art = (directory / "art.bin").read_bytes()
sequences = {}
selectors = set()
for entry in catalog:
    points = tuple(ord(c) for c in entry["text"] if ord(c) not in (0xFE0E, 0xFE0F))
    if not points:
        continue
    sequences.setdefault(points, art[entry["offset"]:entry["offset"] + entry["length"]])
    for left, right in zip(entry["text"], entry["text"][1:]):
        if ord(right) == 0xFE0F:
            selectors.add(ord(left))

characters = sorted({point for points in sequences for point in points})
cmap = {point: f"u{point:X}" for point in characters}
names = {points: cmap[points[0]] if len(points) == 1 else f"emoji{i}"
         for i, points in enumerate(sorted(sequences))}
order = [".notdef"] + list(cmap.values()) + [name for points, name in names.items() if len(points) > 1]
font = FontBuilder(1024, isTTF=True)
font.setupGlyphOrder(order)
font.setupCharacterMap(cmap)
font.setupGlyf({name: TTGlyphPen(None).glyph() for name in order})
zero_width = {cmap[p] for p in characters if p == 0x200D or 0xE0020 <= p <= 0xE007F}
font.setupHorizontalMetrics({name: (0 if name in zero_width else 1024, 0) for name in order})
font.setupHorizontalHeader(ascent=860, descent=-164)
font.setupNameTable({
    "familyName": "Brazap Emoji", "styleName": "Regular",
    "uniqueFontIdentifier": "Brazap Emoji 1.0", "fullName": "Brazap Emoji",
    "psName": "BrazapEmoji", "version": "Version 1.0",
    "copyright": "Twemoji graphics: Twitter and contributors, CC BY 4.0. Font packaging: Brazap contributors.",
})
font.setupOS2(sTypoAscender=860, sTypoDescender=-164, usWinAscent=860, usWinDescent=164)
font.setupPost()
font.setupMaxp()

variation = CmapSubtable.newSubtable(14)
variation.platformID, variation.platEncID, variation.language = 0, 5, 0
variation.cmap = {}
variation.uvsDict = {0xFE0F: [(point, None) for point in sorted(selectors)]}
font.font["cmap"].tables.append(variation)

strike = Strike(ppem=72, resolution=72)
strike.glyphs = {name: Glyph(glyphName=name) for name in order}
for points, png in sequences.items():
    name = names[points]
    strike.glyphs[name] = Glyph(glyphName=name, originOffsetX=0, originOffsetY=-12,
                               graphicType="png ", imageData=png)
sbix = newTable("sbix")
sbix.strikes = {72: strike}
font.font["sbix"] = sbix

# Use a required composition feature, so code/monospace text can still display
# emoji even when ordinary typographic ligatures are disabled.
rules = ["languagesystem DFLT dflt;", "languagesystem latn dflt;", "lookup emoji_sequences useExtension {"]
for points in sorted(sequences, key=lambda points: (-len(points), points)):
    if len(points) > 1:
        rules.append(f"sub {' '.join(cmap[p] for p in points)} by {names[points]};")
rules.extend(["} emoji_sequences;", "feature ccmp { lookup emoji_sequences; } ccmp;"])
addOpenTypeFeaturesFromString(font.font, "\n".join(rules))
font.font.recalcTimestamp = False
font.font["head"].created = font.font["head"].modified = 2082844800
font.save(directory / "BrazapEmoji.ttf")
print(f"Packed {len(sequences)} emoji into BrazapEmoji.ttf")
