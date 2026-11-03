# Bundled emoji

The picker, country flags and inline text share the artwork in `art.bin`.
`BrazapEmoji.ttf` packages those same PNGs as an OpenType `sbix` color font, with
Unicode mappings and composition rules for flags, skin tones, variation selectors
and joined sequences. Its registered family is **Brazap Emoji**.

After updating `index.json` and `art.bin`, regenerate the font with:

```sh
python3 -m pip install fonttools
python3 scripts/prepare-emoji-font.py
```

Upstream artwork sources and hashes are in `sources.json`; the graphics retain
their attribution and CC BY 4.0 terms in `LICENSE-GRAPHICS`. The font is generated
from these bundled assets without downloading or installing a system font.
