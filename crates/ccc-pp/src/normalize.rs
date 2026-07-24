use ccc_diag::codes::preprocessor as diagnostic_codes;
use ccc_session::{FileId, Span};

use crate::diagnostic::{PpDiagnostic, PpDiagnosticCategory, PpSeverity};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct NormalizeOptions {
    pub trigraphs: bool,
    pub warn_trigraphs: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct NormalizedSource {
    pub text: String,
    /// For each normalized byte, the corresponding original byte offset.
    offsets: Vec<usize>,
    /// Exclusive original end offset for each normalized byte.
    original_ends: Vec<usize>,
    /// Original physical line for each normalized byte.
    physical_lines: Vec<usize>,
    original_len: usize,
    pub diagnostics: Vec<PpDiagnostic>,
}

impl NormalizedSource {
    pub fn original_span(&self, file: FileId, start: usize, end: usize) -> Span {
        let original_start = self
            .offsets
            .get(start)
            .copied()
            .unwrap_or(self.original_len);
        let original_end = if end == start {
            original_start
        } else {
            self.original_ends
                .get(end.saturating_sub(1))
                .copied()
                .unwrap_or(original_start)
        };
        Span::new(file, original_start, original_end)
    }

    pub fn physical_line(&self, offset: usize) -> usize {
        self.physical_lines
            .get(offset)
            .copied()
            .or_else(|| self.physical_lines.last().copied())
            .unwrap_or(1)
    }
}

pub(crate) fn normalize(file: FileId, source: &str, options: NormalizeOptions) -> NormalizedSource {
    let bom_len = usize::from(source.starts_with('\u{feff}')) * '\u{feff}'.len_utf8();
    let mut text = String::with_capacity(source.len());
    let mut offsets = Vec::with_capacity(source.len());
    let mut original_ends = Vec::with_capacity(source.len());
    let mut physical_lines = Vec::with_capacity(source.len());
    let bytes = source.as_bytes();
    let mut index = bom_len;
    let mut physical_line = 1_usize;

    // Normalize source newlines while preserving a byte-to-byte source map.
    while index < bytes.len() {
        if bytes[index] == b'\r' {
            text.push('\n');
            offsets.push(index);
            physical_lines.push(physical_line);
            let width = if bytes.get(index + 1) == Some(&b'\n') {
                2
            } else {
                1
            };
            original_ends.push(index + width);
            index += width;
            physical_line += 1;
        } else {
            let character = source[index..].chars().next().expect("index is valid");
            text.push(character);
            offsets.extend(index..index + character.len_utf8());
            original_ends.extend(index + 1..=index + character.len_utf8());
            physical_lines.extend(std::iter::repeat_n(physical_line, character.len_utf8()));
            index += character.len_utf8();
            if character == '\n' {
                physical_line += 1;
            }
        }
    }

    let (text, offsets, original_ends, physical_lines, diagnostics) =
        replace_trigraphs(file, text, offsets, original_ends, physical_lines, options);
    let (text, offsets, original_ends, physical_lines) =
        splice_lines(text, offsets, original_ends, physical_lines);
    NormalizedSource {
        text,
        offsets,
        original_ends,
        physical_lines,
        original_len: source.len(),
        diagnostics,
    }
}

fn replace_trigraphs(
    file: FileId,
    text: String,
    offsets: Vec<usize>,
    original_ends: Vec<usize>,
    physical_lines: Vec<usize>,
    options: NormalizeOptions,
) -> (
    String,
    Vec<usize>,
    Vec<usize>,
    Vec<usize>,
    Vec<PpDiagnostic>,
) {
    let bytes = text.as_bytes();
    let mut output = String::with_capacity(text.len());
    let mut output_offsets = Vec::with_capacity(offsets.len());
    let mut output_ends = Vec::with_capacity(original_ends.len());
    let mut output_lines = Vec::with_capacity(physical_lines.len());
    let mut diagnostics = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let replacement = bytes
            .get(index..index.saturating_add(3))
            .and_then(trigraph_replacement);
        if let Some(replacement) = replacement {
            if options.warn_trigraphs {
                let start = offsets[index];
                let end = original_ends[index + 2];
                diagnostics.push(
                    PpDiagnostic::new(
                        PpSeverity::Warning,
                        diagnostic_codes::TRIGRAPH.as_str(),
                        if options.trigraphs {
                            "trigraph converted to a single character"
                        } else {
                            "trigraph ignored in this language mode"
                        },
                    )
                    .with_span(Span::new(file, start, end))
                    .with_category(PpDiagnosticCategory::Trigraph),
                );
            }
            if options.trigraphs {
                output.push(replacement);
                output_offsets.push(offsets[index]);
                output_ends.push(original_ends[index + 2]);
                output_lines.push(physical_lines[index]);
                index += 3;
                continue;
            }
        }

        let character = text[index..].chars().next().expect("index is valid");
        output.push(character);
        output_offsets.extend_from_slice(&offsets[index..index + character.len_utf8()]);
        output_ends.extend_from_slice(&original_ends[index..index + character.len_utf8()]);
        output_lines.extend_from_slice(&physical_lines[index..index + character.len_utf8()]);
        index += character.len_utf8();
    }
    (
        output,
        output_offsets,
        output_ends,
        output_lines,
        diagnostics,
    )
}

fn trigraph_replacement(bytes: &[u8]) -> Option<char> {
    let [b'?', b'?', third] = *bytes else {
        return None;
    };
    Some(match third {
        b'=' => '#',
        b'/' => '\\',
        b'\'' => '^',
        b'(' => '[',
        b')' => ']',
        b'!' => '|',
        b'<' => '{',
        b'>' => '}',
        b'-' => '~',
        _ => return None,
    })
}

fn splice_lines(
    text: String,
    offsets: Vec<usize>,
    original_ends: Vec<usize>,
    physical_lines: Vec<usize>,
) -> (String, Vec<usize>, Vec<usize>, Vec<usize>) {
    let bytes = text.as_bytes();
    let mut output = String::with_capacity(text.len());
    let mut output_offsets = Vec::with_capacity(offsets.len());
    let mut output_ends = Vec::with_capacity(original_ends.len());
    let mut output_lines = Vec::with_capacity(physical_lines.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\\' && bytes.get(index + 1) == Some(&b'\n') {
            index += 2;
            continue;
        }
        let character = text[index..].chars().next().expect("index is valid");
        output.push(character);
        output_offsets.extend_from_slice(&offsets[index..index + character.len_utf8()]);
        output_ends.extend_from_slice(&original_ends[index..index + character.len_utf8()]);
        output_lines.extend_from_slice(&physical_lines[index..index + character.len_utf8()]);
        index += character.len_utf8();
    }
    (output, output_offsets, output_ends, output_lines)
}

#[cfg(test)]
mod tests {
    use ccc_session::SourceMap;

    use super::*;

    #[test]
    fn removes_bom_normalizes_newlines_and_splices() {
        let mut sources = SourceMap::new();
        let file = sources.add_file("test.c", "\u{feff}a\\\r\nb\rc\r\nd");
        let normalized = normalize(
            file,
            sources.source(file).unwrap(),
            NormalizeOptions {
                trigraphs: false,
                warn_trigraphs: false,
            },
        );
        assert_eq!(normalized.text, "ab\nc\nd");
    }

    #[test]
    fn trigraph_can_form_a_splice() {
        let mut sources = SourceMap::new();
        let file = sources.add_file("test.c", "a??/\nb");
        let normalized = normalize(
            file,
            sources.source(file).unwrap(),
            NormalizeOptions {
                trigraphs: true,
                warn_trigraphs: false,
            },
        );
        assert_eq!(normalized.text, "ab");
    }
}
