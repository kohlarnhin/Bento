use regex::Regex;
use std::sync::LazyLock;

const WRAPPERS: &[&str] = &[
    "INSTRUCTIONS",
    "system-reminder",
    "system_instructions",
    "user_instructions",
    "environment_context",
    "command-name",
    "command-message",
    "command-args",
    "local-command-caveat",
    "caveat",
    "local-command-stdout",
    "ide_opened_file",
    "ide_selection",
    "task-notification",
    "teammate-message",
    "turn_aborted",
    "permissions instructions",
    "collaboration_mode",
    "app-context",
];

pub(super) fn strip_wrappers(raw: &str) -> Option<String> {
    let raw = if raw.trim_start().starts_with("# Context from my IDE setup:") {
        raw.split_once("## My request for Codex:")?.1
    } else {
        raw
    };
    let raw = raw.trim_start();
    let raw = if raw.starts_with("# AGENTS.md instructions") {
        // An unbounded instruction preamble has no reliable end marker.
        if let Some((_, request)) = raw.split_once("## My request for Codex:") {
            request
        } else if raw.contains("</INSTRUCTIONS>") || raw.contains("</user_instructions>") {
            raw.split_once('\n')?.1
        } else {
            return None;
        }
    } else {
        raw
    };
    let mut clean = raw.to_owned();
    for tag in WRAPPERS {
        let opening = format!("<{tag}");
        let closing = format!("</{tag}>");
        let mut offset = 0;
        while let Some(index) = clean[offset..].find(&opening) {
            let start = offset + index;
            offset = start + opening.len();
            if !clean[offset..].starts_with(['>', ' ', '\t', '/']) {
                continue;
            }
            let Some(tag_end) = clean[offset..].find('>').map(|index| offset + index) else {
                break;
            };
            let end = if clean[offset..tag_end].trim_end().ends_with('/') {
                tag_end + 1
            } else {
                clean[tag_end + 1..]
                    .find(&closing)
                    .map_or(clean.len(), |index| tag_end + 1 + index + closing.len())
            };
            clean.replace_range(start..end, "\n");
            offset = start + 1;
        }
    }
    Some(clean)
}

pub(super) fn legacy_title(text: &str) -> Option<String> {
    let mut line = text
        .lines()
        .map(str::trim)
        .find(|line| ordinary_line(line) && !line.starts_with('<'))?
        .chars()
        .filter(|character| !character.is_control());
    let mut result: String = line.by_ref().take(100).collect();
    if line.next().is_some() {
        result.push('…');
    }
    (!result.is_empty()).then_some(result)
}

pub(super) fn user_text(text: &str) -> bool {
    text.lines().map(str::trim).any(ordinary_line)
}

fn ordinary_line(line: &str) -> bool {
    !line.is_empty()
        && !line.starts_with("# AGENTS.md instructions")
        && !line
            .get(..7)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("caveat:"))
        && !matches!(
            line.split_whitespace().next(),
            Some("/exit" | "/status" | "/clear" | "/help" | "/resume" | "/compact" | "/model")
        )
}

pub(super) fn clean_title(text: &str) -> Option<String> {
    static IMAGES: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)!\[[^\]\n]*\]\([^\n)]*\)|<<ImageDisplayed>>|<image(?:\s*\d+)?\s*/?>|\[(?:image|图片)(?:\s*#?\d+)?\]").unwrap()
    });
    static LOG: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?ix) ^
            (?: Traceback\s*\( | File\s+"[^"]+",\s*line\s+\d+ | at\s+\S.*(?::\d+|\(native\))
             | (?:\w+(?:Error|Exception)|Error|Caused\s+by|panic|fatal\serror)\s*:
             | error\[E\d+\]: | thread\s+'.+'\s+panicked\s+at
             | (?:\d{4}-\d\d-\d\d[\x20T]\d\d:\d\d\S*\s+)?\[?(?:TRACE|DEBUG|INFO|WARN(?:ING)?|ERROR|FATAL)\]?(?:\s|:)
             | npm\s+(?:ERR!|error|warn)\s | \(node:\d+\)\s
             | \d+:\s+0x[0-9a-f]+ | \^+$ | (?:\s*[-+])?\s*\d+\s*\| )"#).unwrap()
    });
    let mut fence = None;
    let mut pasted_log = false;
    for raw in text.lines() {
        let line = raw.trim();
        if let Some(marker) = fence {
            if line.trim_end_matches(marker).is_empty() && line.starts_with(marker) {
                fence = None;
            }
            continue;
        }
        if line.starts_with("```") || line.starts_with("~~~") {
            fence = line.chars().next();
            continue;
        }
        let without_images = IMAGES.replace_all(line, " ");
        let line = without_images.trim();
        if LOG.is_match(line) {
            pasted_log = true;
            continue;
        }
        if pasted_log && raw.starts_with(char::is_whitespace) {
            continue;
        }
        if !ordinary_line(line) || line.starts_with('<') || long_path(line) {
            continue;
        }
        let mut sentence = line;
        for (index, character) in line.char_indices() {
            let end = index + character.len_utf8();
            if matches!(character, '。' | '！' | '？')
                || (matches!(character, '.' | '!' | '?')
                    && line[end..].starts_with(char::is_whitespace))
            {
                sentence = &line[..end];
                break;
            }
        }
        let compact = sentence.split_whitespace().collect::<Vec<_>>().join(" ");
        let mut chars = compact.chars().filter(|ch| !ch.is_control());
        let mut result: String = chars.by_ref().take(100).collect();
        if chars.next().is_some() {
            result.push('…');
        }
        if !result.is_empty() {
            return Some(result);
        }
    }
    None
}

fn long_path(line: &str) -> bool {
    let path = line.trim_matches(['`', '"', '\'']);
    path.len() >= 32
        && !path.contains(char::is_whitespace)
        && (path.starts_with(['/', '~'])
            || path.starts_with("./")
            || path.starts_with("../")
            || path.starts_with("file://")
            || path
                .as_bytes()
                .get(1..3)
                .is_some_and(|start| matches!(start, b":/" | b":\\")))
}
