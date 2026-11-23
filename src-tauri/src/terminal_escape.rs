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
    csi: Vec<u8>,
}

pub enum EscapeEvent<'a> {
    Text,
    Control,
    Osc(Vec<u8>),
    Csi(&'a [u8]),
    Other,
}

impl EscapeStream {
    pub fn event(&mut self, byte: u8) -> EscapeEvent<'_> {
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
            return if self.too_long {
                EscapeEvent::Other
            } else {
                EscapeEvent::Osc(std::mem::take(&mut self.osc))
            };
        }
        if matches!(self.state, Csi) && (0x40..=0x7e).contains(&byte) {
            self.state = Text;
            return EscapeEvent::Csi(&self.csi);
        }

        let ground = matches!(self.state, Text);
        self.state = match (self.state, byte) {
            (Text, 27) => Escape,
            (Escape, b'[') => {
                self.csi.clear();
                Csi
            }
            (Escape, b']') => {
                self.osc.clear();
                self.too_long = false;
                Osc
            }
            (Escape, b'P' | b'X' | b'^' | b'_') => String,
            (Escape | Intermediate, 0x20..=0x2f) => Intermediate,
            (Escape | Intermediate | Csi, 27) => Escape,
            (Escape | Intermediate, _) => Text,
            (Csi, _) => {
                if self.csi.len() < 40 {
                    self.csi.push(byte);
                }
                Csi
            }
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
        if text {
            EscapeEvent::Text
        } else if ground && byte != 27 {
            EscapeEvent::Control
        } else {
            EscapeEvent::Other
        }
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
}
