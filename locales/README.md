# Interface translations

`en.json` is the source catalog. `pt-BR.json` and `es.json` use the same keys.
Values may include named placeholders such as `{count}` or `{version}`; preserve
those names in every translation. Strings are UTF-8 JSON, embedded in the binary.

Use `locale::tr("Text")` for static UI copy and `locale::format("Text {count}",
&[("count", count.to_string())])` for variables. Do not translate user messages,
contact names, file names, protocol identifiers, or paths.

Changing language applies immediately. Unknown keys fall back to English.
Edit the JSON files and rebuild to update the bundled translations.
