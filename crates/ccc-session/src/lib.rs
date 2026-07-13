//! Source files, spans, and line/column lookup shared by compiler phases.

use std::fmt;

/// A stable index into a [`SourceMap`].
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct FileId(u32);

/// A half-open byte range in a source file.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Span {
    pub file: FileId,
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub const fn new(file: FileId, start: usize, end: usize) -> Self {
        Self { file, start, end }
    }
}

/// A human-readable source position. Lines and columns are one-based.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SourceLocation {
    pub line: usize,
    pub column: usize,
    pub line_start: usize,
    pub line_end: usize,
}

#[derive(Debug)]
struct SourceFile {
    name: String,
    source: String,
    line_starts: Vec<usize>,
}

impl SourceFile {
    fn new(name: String, source: String) -> Self {
        let mut line_starts = vec![0];
        for (index, byte) in source.bytes().enumerate() {
            if byte == b'\n' {
                line_starts.push(index + 1);
            }
        }
        Self {
            name,
            source,
            line_starts,
        }
    }

    fn location(&self, offset: usize) -> Option<SourceLocation> {
        if offset > self.source.len() || !self.source.is_char_boundary(offset) {
            return None;
        }

        let line_index = self
            .line_starts
            .partition_point(|&line_start| line_start <= offset)
            .saturating_sub(1);
        let line_start = self.line_starts[line_index];
        let line_end = self.source[line_start..]
            .find('\n')
            .map_or(self.source.len(), |index| line_start + index);
        let column = self.source[line_start..offset].chars().count() + 1;

        Some(SourceLocation {
            line: line_index + 1,
            column,
            line_start,
            line_end,
        })
    }

    fn line_text(&self, line: usize) -> Option<&str> {
        let line_start = *self.line_starts.get(line.checked_sub(1)?)?;
        let line_end = self.source[line_start..]
            .find('\n')
            .map_or(self.source.len(), |index| line_start + index);
        Some(
            self.source[line_start..line_end]
                .strip_suffix('\r')
                .unwrap_or(&self.source[line_start..line_end]),
        )
    }
}

/// Owns input source text and resolves spans for diagnostics and dumps.
#[derive(Debug, Default)]
pub struct SourceMap {
    files: Vec<SourceFile>,
}

impl SourceMap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_file(&mut self, name: impl Into<String>, source: impl Into<String>) -> FileId {
        let id = FileId(self.files.len().try_into().expect("too many source files"));
        self.files.push(SourceFile::new(name.into(), source.into()));
        id
    }

    pub fn source(&self, file: FileId) -> Option<&str> {
        self.files
            .get(file.0 as usize)
            .map(|file| file.source.as_str())
    }

    pub fn file_name(&self, file: FileId) -> Option<&str> {
        self.files
            .get(file.0 as usize)
            .map(|file| file.name.as_str())
    }

    pub fn location(&self, file: FileId, offset: usize) -> Option<SourceLocation> {
        self.files.get(file.0 as usize)?.location(offset)
    }

    pub fn line_text(&self, file: FileId, line: usize) -> Option<&str> {
        self.files.get(file.0 as usize)?.line_text(line)
    }
}

impl fmt::Display for Span {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "file#{}:{}..{}",
            self.file.0, self.start, self.end
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_utf8_offsets_to_character_columns() {
        let mut sources = SourceMap::new();
        let file = sources.add_file("input.c", "aé\nxyz");

        assert_eq!(
            sources.location(file, 3),
            Some(SourceLocation {
                line: 1,
                column: 3,
                line_start: 0,
                line_end: 3,
            })
        );
        assert_eq!(sources.line_text(file, 2), Some("xyz"));
    }
}
