# Phone input data

`countries.json` contains country IDs, international calling codes and sample
numbers from the metadata bundled by `phonenumber 0.3.10+9.0.33` (Google
libphonenumber, Apache-2.0). Formatting and validation use that Rust library at
runtime; the country list is not a collection of hand-written masks.

Country names in `locales/*.json` come from Unicode CLDR release 48. See the
Unicode license in `assets/emoji/UNICODE-LICENSE`. `sources.json` records the
source URLs and SHA-256 digests.

Flags reuse the bundled Twemoji artwork and its existing license in
`assets/emoji/LICENSE-GRAPHICS`. No network request is needed to display them.
