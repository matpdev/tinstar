/// Byte offset ↔ line/column conversion for one text, built once per file.
pub struct LineIndex<'a> {
    text: &'a str,
    starts: Vec<usize>,
}

impl<'a> LineIndex<'a> {
    pub fn new(text: &'a str) -> Self {
        let mut starts = vec![0];
        starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
        Self { text, starts }
    }

    /// 0-based line of a byte offset.
    pub fn line(&self, offset: usize) -> usize {
        self.starts.partition_point(|&s| s <= offset) - 1
    }

    /// 0-based (line, character column).
    pub fn line_col(&self, offset: usize) -> (usize, usize) {
        let line = self.line(offset);
        let col = self.text[self.starts[line]..offset].chars().count();
        (line, col)
    }

    /// 0-based (line, UTF-16 column), what the Language Server Protocol counts in.
    pub fn line_col_utf16(&self, offset: usize) -> (usize, usize) {
        let line = self.line(offset);
        let col = self.text[self.starts[line]..offset].encode_utf16().count();
        (line, col)
    }

    /// Text of a 0-based line, without its line break.
    pub fn line_text(&self, line: usize) -> &'a str {
        let start = self.starts[line];
        let end = self
            .starts
            .get(line + 1)
            .copied()
            .unwrap_or(self.text.len());
        self.text[start..end].trim_end_matches(['\n', '\r'])
    }

    pub fn span(&self, start: usize, end: usize) -> crate::Span {
        let (l1, c1) = self.line_col(start);
        let (l2, c2) = self.line_col(end);
        crate::Span {
            start,
            end,
            line: l1 + 1,
            column: c1 + 1,
            end_line: l2 + 1,
            end_column: c2 + 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_and_columns() {
        let idx = LineIndex::new("ab\nçd\n\nx");
        assert_eq!(idx.line_col(0), (0, 0));
        assert_eq!(idx.line_col(3), (1, 0));
        assert_eq!(idx.line_col(5), (1, 1)); // after the 2-byte 'ç'
        assert_eq!(idx.line_col(8), (3, 0));
        assert_eq!(idx.line_text(1), "çd");
        assert_eq!(idx.line_text(3), "x");
    }
}
