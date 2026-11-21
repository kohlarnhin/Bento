// Keep only the current viewport for approval matching. Codex's TUI repaints
// at cursor positions, so a transcript would retain prompts after dismissal.
pub(super) struct Screen {
    rows: Vec<Vec<char>>,
    width: usize,
    row: usize,
    col: usize,
    utf8: Vec<u8>,
}

impl Default for Screen {
    fn default() -> Self {
        Self {
            rows: vec![Vec::new(); 24],
            width: 80,
            row: 0,
            col: 0,
            utf8: Vec::new(),
        }
    }
}

impl Screen {
    pub(super) fn resize(&mut self, cols: u16, rows: u16) -> bool {
        let width = usize::from(cols).clamp(1, 512);
        let height = usize::from(rows).clamp(1, 256);
        if width == self.width && height == self.rows.len() {
            return false;
        }
        self.width = width;
        self.rows.resize_with(height, Vec::new);
        self.clear_text();
        self.row = self.row.min(height - 1);
        self.col = self.col.min(width - 1);
        true
    }

    pub(super) fn clear_text(&mut self) -> bool {
        let changed = self.rows.iter().any(|line| !line.is_empty());
        self.rows.iter_mut().for_each(Vec::clear);
        self.utf8.clear();
        changed
    }

    fn line_down(&mut self) -> bool {
        if self.row + 1 < self.rows.len() {
            self.row += 1;
            return false;
        }
        let changed = !self.rows[0].is_empty();
        self.rows.rotate_left(1);
        self.rows.last_mut().unwrap().clear();
        changed
    }

    pub(super) fn text(&mut self, byte: u8) -> bool {
        let ch = match byte {
            b'\r' => {
                self.col = 0;
                return false;
            }
            b'\n' => return self.line_down(),
            b'\t' => {
                self.col = (self.col / 8 * 8 + 8).min(self.width - 1);
                return false;
            }
            8 => {
                self.col = self.col.saturating_sub(1);
                return false;
            }
            32..=126 => {
                self.utf8.clear();
                char::from(byte)
            }
            128..=255 => {
                self.utf8.push(byte);
                match std::str::from_utf8(&self.utf8) {
                    Ok(text) => {
                        let ch = text.chars().next().unwrap();
                        self.utf8.clear();
                        ch
                    }
                    Err(error) => {
                        if error.error_len().is_some() {
                            self.utf8.clear();
                        }
                        return false;
                    }
                }
            }
            _ => return false,
        };
        let mut changed = false;
        if self.col >= self.width {
            changed = self.line_down();
            self.col = 0;
        }
        let line = &mut self.rows[self.row];
        if self.col >= line.len() {
            if ch != ' ' {
                line.resize(self.col + 1, ' ');
            }
        }
        if let Some(cell) = line.get_mut(self.col) {
            changed |= *cell != ch;
            *cell = ch;
        }
        self.col += 1;
        changed
    }

    pub(super) fn csi(&mut self, params: &[u8], command: u8) -> bool {
        if params.starts_with(b"?") {
            if matches!(params, b"?47" | b"?1047" | b"?1049") && matches!(command, b'h' | b'l') {
                self.row = 0;
                self.col = 0;
                return self.clear_text();
            }
            return false;
        }
        let mut values = params
            .split(|&byte| byte == b';')
            .map(|value| std::str::from_utf8(value).ok()?.parse::<usize>().ok());
        let mode = values.next().flatten().unwrap_or(0);
        let count = mode.max(1);
        let last_row = self.rows.len() - 1;
        let last_col = self.width - 1;
        match command {
            b'A' | b'F' => self.row = self.row.saturating_sub(count),
            b'B' | b'E' => self.row = self.row.saturating_add(count).min(last_row),
            b'C' => self.col = self.col.saturating_add(count).min(last_col),
            b'D' => self.col = self.col.saturating_sub(count),
            b'G' => self.col = (count - 1).min(last_col),
            b'd' => self.row = (count - 1).min(last_row),
            b'H' | b'f' => {
                self.row = (count - 1).min(last_row);
                self.col = values
                    .next()
                    .flatten()
                    .unwrap_or(1)
                    .saturating_sub(1)
                    .min(last_col);
            }
            b'J' => {
                let (start, end) = match mode {
                    0 => (self.row + 1, self.rows.len()),
                    1 => (0, self.row),
                    2 => return self.clear_text(),
                    _ => return false,
                };
                let changed = self.rows[start..end].iter().any(|line| !line.is_empty());
                self.rows[start..end].iter_mut().for_each(Vec::clear);
                return self.erase_line(mode) || changed;
            }
            b'K' => return self.erase_line(mode),
            _ => (),
        }
        if matches!(command, b'E' | b'F') {
            self.col = 0;
        }
        false
    }

    fn erase_line(&mut self, mode: usize) -> bool {
        let line = &mut self.rows[self.row];
        match mode {
            0 | 2 => {
                let keep = if mode == 0 { self.col } else { 0 };
                let changed = keep < line.len();
                line.truncate(keep);
                changed
            }
            1 => {
                let end = self.col.saturating_add(1).min(line.len());
                let changed = line[..end].iter().any(|&ch| ch != ' ');
                line[..end].fill(' ');
                changed
            }
            _ => false,
        }
    }

    pub(super) fn tail(&self) -> String {
        const LOOKBACK: usize = 24;
        let end = self
            .rows
            .iter()
            .rposition(|line| line.iter().any(|&ch| ch != ' '))
            .map_or(0, |i| i + 1);
        self.rows[end.saturating_sub(LOOKBACK)..end]
            .iter()
            .flat_map(|line| {
                line.iter()
                    .map(|ch| ch.to_ascii_lowercase())
                    .chain(std::iter::once(' '))
            })
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }
}
