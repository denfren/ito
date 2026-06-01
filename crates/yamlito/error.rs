use rowan::TextSize;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message} at byte {offset:?}")]
pub struct ParseError {
    pub offset: TextSize,
    pub message: String,
    pub expected: Vec<&'static str>,
}

impl ParseError {
    pub fn new(offset: TextSize, message: impl Into<String>) -> Self {
        Self {
            offset,
            message: message.into(),
            expected: Vec::new(),
        }
    }

    /// 1-based line, 0-based byte column within the line.
    pub fn line_col(&self, source: &str) -> (u32, u32) {
        let off = usize::from(self.offset).min(source.len());
        let prefix = &source[..off];
        let line = prefix.bytes().filter(|&b| b == b'\n').count() as u32 + 1;
        let col = match prefix.rfind('\n') {
            Some(nl) => (prefix.len() - nl - 1) as u32,
            None => prefix.len() as u32,
        };
        (line, col)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_col_basic() {
        let src = "ab\ncd\nef";
        assert_eq!(ParseError::new(0u32.into(), "x").line_col(src), (1, 0));
        assert_eq!(ParseError::new(2u32.into(), "x").line_col(src), (1, 2));
        assert_eq!(ParseError::new(3u32.into(), "x").line_col(src), (2, 0));
        assert_eq!(ParseError::new(5u32.into(), "x").line_col(src), (2, 2));
        assert_eq!(ParseError::new(6u32.into(), "x").line_col(src), (3, 0));
    }

    #[test]
    fn line_col_past_end_clamps() {
        let src = "abc";
        assert_eq!(ParseError::new(999u32.into(), "x").line_col(src), (1, 3));
    }
}
