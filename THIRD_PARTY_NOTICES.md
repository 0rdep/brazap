# Third-party notices

The voice decoder in `src/playback/voice.rs` adapts the Opus decoding approach in
Rune's `crates/rune/src/video/foreign.rs`. Rune is available at
https://github.com/anomalyco/rune and is distributed under the following license.

MIT License

Copyright (c) 2026 Christopher Langton

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.


## Emoji artwork and search data

The emoji picker includes Twemoji 17.0.3 artwork by Twitter, Inc. and contributors
(https://github.com/jdecked/twemoji), licensed under CC-BY 4.0. Artwork is bundled
unmodified as PNGs in `assets/emoji/art.bin`; see `assets/emoji/LICENSE-GRAPHICS`.
Unicode emoji names and Portuguese CLDR annotations are copyright Unicode, Inc.
and distributed under the Unicode License; see `assets/emoji/UNICODE-LICENSE`.
Exact upstream versions and download hashes are recorded in
`assets/emoji/sources.json`. Rebuild with `python3 scripts/prepare-emojis.py`.
