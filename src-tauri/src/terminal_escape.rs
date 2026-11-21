#[derive(Clone, Copy, Default)]
enum State {
    #[default]
    Text,
    Escape,
    Intermediate,
    Csi,
    Osc,
    OscEscape,
    String,
    StringEscape,
}

/// A stream can split a control sequence across multiple PTY reads.
#[derive(Default)]
pub struct EscapeStream {
    state: State,
    osc: Vec<u8>,
    too_long: bool,
}

impl EscapeStream {
    pub fn push(&mut self, byte: u8) -> (bool, Option<Vec<u8>>) {
        use State::*;
        let text = matches!(self.state, Text)
            && (byte >= 32 && byte != 127 || byte == b'\n' || byte == b'\t');
        let done = match self.state {
            Osc => byte == 7,
            OscEscape => byte == b'\\',
            _ => false,
        };
        if done {
            self.state = Text;
            return (
                false,
                (!self.too_long).then(|| std::mem::take(&mut self.osc)),
            );
        }

        self.state = match (self.state, byte) {
            (Text, 27) => Escape,
            (Escape, b'[') => Csi,
            (Escape, b']') => {
                self.osc.clear();
                self.too_long = false;
                Osc
            }
            (Escape, b'P' | b'X' | b'^' | b'_') => String,
            (Escape | Intermediate, 0x20..=0x2f) => Intermediate,
            (Escape | Intermediate | Csi, 27) => Escape,
            (Escape | Intermediate, _) | (Csi, 0x40..=0x7e) => Text,
            (Osc, 27) => OscEscape,
            (Osc, _) => {
                if self.osc.len() < 8192 {
                    self.osc.push(byte);
                } else {
                    self.too_long = true;
                }
                Osc
            }
            (OscEscape, _) => {
                self.too_long = true;
                Osc
            }
            (String, 27) => StringEscape,
            (StringEscape, b'\\') => Text,
            (StringEscape, _) => String,
            (state, _) => state,
        };
        (text, None)
    }
}

pub fn osc_cwd(value: &str) -> Option<std::path::PathBuf> {
    let mut url = tauri::Url::parse(value).ok()?;
    if url.scheme() != "file" {
        return None;
    }
    url.set_host(Some("localhost")).ok()?;
    url.to_file_path().ok()
}

pub fn history_text(raw: &str) -> String {
    let mut stream = EscapeStream::default();
    let mut result = String::new();
    let mut line = Vec::new();
    let mut cursor = 0usize;
    let mut alternate = false;
    let mut csi = String::new();
    for ch in raw.chars() {
        let previous = stream.state;
        let mut bytes = [0; 4];
        let printable = ch
            .encode_utf8(&mut bytes)
            .bytes()
            .fold(false, |text, byte| stream.push(byte).0 || text);
        if matches!(previous, State::Csi) {
            if matches!(stream.state, State::Csi) {
                if csi.len() < 40 {
                    csi.push(ch);
                }
                continue;
            }
            match (ch, csi.as_str()) {
                ('h', "?1049" | "?1047" | "?47") => alternate = true,
                ('l', "?1049" | "?1047" | "?47") => alternate = false,
                _ if alternate => (),
                ('K', "" | "0") => line.truncate(cursor),
                ('K', "2") => line.clear(),
                ('D', count) => cursor = cursor.saturating_sub(count.parse().unwrap_or(1)),
                ('C', count) => {
                    cursor = cursor
                        .saturating_add(count.parse().unwrap_or(1))
                        .min(16_384)
                }
                ('G', at) => {
                    cursor = at
                        .parse::<usize>()
                        .unwrap_or(1)
                        .saturating_sub(1)
                        .min(16_384)
                }
                _ => (),
            }
            csi.clear();
        } else if !alternate && matches!(previous, State::Text) {
            match ch {
                '\r' => cursor = 0,
                '\u{8}' => cursor = cursor.saturating_sub(1),
                '\n' => {
                    append_line(&mut result, &line);
                    result.push('\n');
                    line.clear();
                    cursor = 0;
                }
                ch if printable && (!ch.is_control() || ch == '\t') => {
                    if cursor >= line.len() {
                        line.resize(cursor + 1, ' ');
                    }
                    line[cursor] = ch;
                    cursor += 1;
                }
                _ => (),
            }
        }
    }
    append_line(&mut result, &line);
    result
}

fn append_line(result: &mut String, line: &[char]) {
    let end = line.iter().rposition(|ch| *ch != ' ').map_or(0, |i| i + 1);
    result.extend(line[..end].iter().copied());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cwd_decodes_spaces_and_unicode_from_osc7() {
        assert_eq!(
            osc_cwd("file://host/tmp/%E8%B4%A6%20%E5%8D%95")
                .unwrap()
                .to_str(),
            Some("/tmp/账 单")
        );
        assert!(osc_cwd("https://host/tmp").is_none());
    }

    #[test]
    fn saved_history_cannot_replay_terminal_control_sequences() {
        assert_eq!(history_text("a\x1b[?1049hvim\x1b[?1049lb\x1b]52;c;secret\x07c\x1bPabc\x1b\\d\r\n\x1b(0e\x1b[31m"), "abcd\ne");
        assert_eq!(
            history_text("first\rprompt\x1b[K\r\np\x08printf"),
            "prompt\nprintf"
        );
    }
}
