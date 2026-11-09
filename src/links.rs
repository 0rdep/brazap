use std::ops::Range;

/// Byte ranges match Rune's shaped-text links and preserve Unicode around URLs.
pub(crate) fn detect(text: &str) -> Vec<(Range<usize>, String)> {
    let mut links = Vec::new();
    let mut offset = 0;
    while offset < text.len() {
        let rest = &text[offset..];
        let next = ["https://", "http://", "www."]
            .iter()
            .filter_map(|prefix| rest.find(prefix))
            .min();
        let Some(next) = next else { break };
        let start = offset + next;
        let end = text[start..]
            .find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '\''))
            .map_or(text.len(), |n| start + n);
        let mut value = text[start..end].trim_end_matches(['.', ',', '!', '?', ';', ':']);
        for (open, close) in [('(', ')'), ('[', ']'), ('{', '}')] {
            while value.ends_with(close)
                && value.matches(close).count() > value.matches(open).count()
            {
                value = &value[..value.len() - close.len_utf8()];
            }
        }
        let url = if value.starts_with("www.") {
            format!("https://{value}")
        } else {
            value.to_owned()
        };
        let host = url
            .split("://")
            .nth(1)
            .unwrap_or("")
            .split(['/', '?', '#'])
            .next()
            .unwrap_or("");
        if !host.is_empty() && host != "www." {
            links.push((start..start + value.len(), url));
        }
        offset = end.max(start + 1);
    }
    links
}
