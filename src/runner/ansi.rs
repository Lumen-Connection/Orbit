//! Strip cursor noise and keep SGR colors as RGB spans.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyleSpan {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputLine {
    pub text: String,
    pub spans: Vec<StyleSpan>,
}

struct Performer {
    spans: Vec<StyleSpan>,
    buf: String,
    color: (u8, u8, u8),
}

impl Performer {
    fn flush(&mut self) {
        if self.buf.is_empty() {
            return;
        }
        self.spans.push(StyleSpan {
            r: self.color.0,
            g: self.color.1,
            b: self.color.2,
            text: std::mem::take(&mut self.buf),
        });
    }

    fn apply_sgr(&mut self, params: &anstyle_parse::Params) {
        let mut any = false;
        for param in params.iter() {
            any = true;
            let code = param.first().copied().unwrap_or(0);
            match code {
                0 => self.color = (200, 200, 200),
                30 => self.color = (0, 0, 0),
                31 => self.color = (220, 80, 80),
                32 => self.color = (80, 180, 90),
                33 => self.color = (220, 180, 70),
                34 => self.color = (80, 140, 220),
                35 => self.color = (180, 100, 200),
                36 => self.color = (70, 180, 200),
                37 => self.color = (220, 220, 220),
                90 => self.color = (120, 120, 120),
                91 => self.color = (255, 120, 120),
                92 => self.color = (120, 220, 130),
                93 => self.color = (240, 220, 100),
                94 => self.color = (120, 170, 255),
                95 => self.color = (210, 140, 230),
                96 => self.color = (100, 220, 230),
                97 => self.color = (255, 255, 255),
                38 => {
                    let rest: Vec<u16> = param.to_vec();
                    if rest.get(1) == Some(&2) && rest.len() >= 5 {
                        self.color = (rest[2] as u8, rest[3] as u8, rest[4] as u8);
                    } else if rest.get(1) == Some(&5) && rest.len() >= 3 {
                        self.color = xterm256(rest[2] as u8);
                    }
                }
                _ => {}
            }
        }
        if !any {
            self.color = (200, 200, 200);
        }
    }
}

impl anstyle_parse::Perform for Performer {
    fn print(&mut self, c: char) {
        self.buf.push(c);
    }

    fn execute(&mut self, byte: u8) {
        if byte == b'\n' || byte == b'\r' || byte == b'\t' {
            self.buf.push(byte as char);
        }
    }

    fn csi_dispatch(
        &mut self,
        params: &anstyle_parse::Params,
        _intermediates: &[u8],
        _ignore: bool,
        action: u8,
    ) {
        if action == b'm' {
            self.flush();
            self.apply_sgr(params);
        }
    }
}

fn xterm256(idx: u8) -> (u8, u8, u8) {
    match idx {
        0 => (0, 0, 0),
        1 => (205, 0, 0),
        2 => (0, 205, 0),
        3 => (205, 205, 0),
        4 => (0, 0, 238),
        5 => (205, 0, 205),
        6 => (0, 205, 205),
        7 => (229, 229, 229),
        8 => (127, 127, 127),
        9..=15 => (255, 0, 0),
        n => {
            let v = n.saturating_mul(3);
            (v, v, v)
        }
    }
}

/// Parse a chunk into display lines. Cursor-movement CSI is dropped.
pub fn parse_chunk(chunk: &str) -> (Vec<OutputLine>, String) {
    let stripped = String::from_utf8_lossy(&strip_ansi_escapes::strip(chunk)).into_owned();
    let mut parser = anstyle_parse::Parser::<anstyle_parse::Utf8Parser>::new();
    let mut performer = Performer {
        spans: Vec::new(),
        buf: String::new(),
        color: (200, 200, 200),
    };
    for byte in chunk.bytes() {
        parser.advance(&mut performer, byte);
    }
    performer.flush();
    let mut lines = Vec::new();
    let mut current_spans: Vec<StyleSpan> = Vec::new();
    let mut current_text = String::new();
    for span in performer.spans {
        for (i, part) in span.text.split('\n').enumerate() {
            if i > 0 {
                lines.push(OutputLine {
                    text: std::mem::take(&mut current_text),
                    spans: std::mem::take(&mut current_spans),
                });
            }
            if !part.is_empty() {
                current_text.push_str(part);
                current_spans.push(StyleSpan {
                    r: span.r,
                    g: span.g,
                    b: span.b,
                    text: part.to_string(),
                });
            }
        }
    }
    if !current_text.is_empty() || !current_spans.is_empty() {
        lines.push(OutputLine {
            text: current_text,
            spans: current_spans,
        });
    }
    let mut stripped_lines: Vec<String> = stripped
        .split('\n')
        .filter(|s| !s.is_empty() || stripped.ends_with('\n'))
        .map(ToString::to_string)
        .collect();
    if stripped.ends_with('\n') {
        stripped_lines.pop();
    }
    for (line, plain) in lines.iter_mut().zip(stripped_lines.iter()) {
        line.text = plain.clone();
    }
    (lines, String::new())
}

pub fn split_complete_lines(chunk: &str, partial: &mut String) -> Vec<OutputLine> {
    partial.push_str(chunk);
    let mut out = Vec::new();
    while let Some(idx) = partial.find('\n') {
        let mut line = partial.drain(..=idx).collect::<String>();
        if line.ends_with('\n') {
            line.pop();
            if line.ends_with('\r') {
                line.pop();
            }
        }
        let (mut parsed, _) = parse_chunk(&format!("{line}\n"));
        if let Some(parsed) = parsed.pop() {
            out.push(parsed);
        } else {
            let text = String::from_utf8_lossy(&strip_ansi_escapes::strip(&line)).into_owned();
            out.push(OutputLine {
                text: text.clone(),
                spans: vec![StyleSpan {
                    r: 200,
                    g: 200,
                    b: 200,
                    text,
                }],
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::split_complete_lines;

    #[test]
    fn strips_raw_escape_sequences_from_line_text() {
        let mut partial = String::new();
        let lines = split_complete_lines("\u{1b}[31merror\u{1b}[0m boom\n", &mut partial);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "error boom");
        assert!(!lines[0].text.contains('\u{1b}'));
        assert!(partial.is_empty());
    }

    #[test]
    fn holds_partial_line_until_newline() {
        let mut partial = String::new();
        let first = split_complete_lines("hel", &mut partial);
        assert!(first.is_empty());
        let second = split_complete_lines("lo\n", &mut partial);
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].text, "hello");
    }
}
