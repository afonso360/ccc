//! Source files, spans, provenance, and line/column lookup shared by compiler phases.

use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;

use ccc_target::EffectiveCompilationConfig;

/// A stable index for one source-file occurrence in a [`SourceMap`].
///
/// Including the same physical file twice produces two distinct identifiers.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct FileId(u32);

impl FileId {
    pub const fn index(self) -> u32 {
        self.0
    }
}

/// A stable index into the source-origin arena.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct OriginId(u32);

impl OriginId {
    /// The origin used by tokens written directly in a source occurrence.
    pub const DIRECT: Self = Self(0);

    pub const fn index(self) -> u32 {
        self.0
    }

    pub const fn is_direct(self) -> bool {
        self.0 == Self::DIRECT.0
    }
}

impl Default for OriginId {
    fn default() -> Self {
        Self::DIRECT
    }
}

/// A half-open byte range in a source occurrence.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Span {
    pub file: FileId,
    pub start: usize,
    pub end: usize,
    pub origin: OriginId,
}

impl Span {
    /// Creates a span for text written directly in a source occurrence.
    pub const fn new(file: FileId, start: usize, end: usize) -> Self {
        Self {
            file,
            start,
            end,
            origin: OriginId::DIRECT,
        }
    }

    pub const fn with_origin(file: FileId, start: usize, end: usize, origin: OriginId) -> Self {
        Self {
            file,
            start,
            end,
            origin,
        }
    }

    pub const fn with_origin_id(self, origin: OriginId) -> Self {
        Self { origin, ..self }
    }
}

/// A human-readable physical source position. Lines and columns are one-based.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SourceLocation {
    pub line: usize,
    pub column: usize,
    pub line_start: usize,
    pub line_end: usize,
}

/// A presumed source position after applying `#line` mappings.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PresumedLocation<'a> {
    pub file_name: &'a str,
    pub line: usize,
    pub column: usize,
    pub physical_line: usize,
    pub line_start: usize,
    pub line_end: usize,
}

/// The physical identity used to recognize repeated occurrences of a file.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum FileIdentity {
    CanonicalPath(PathBuf),
    DeviceInode { device: u64, inode: u64 },
    Opaque(String),
}

/// The directive that introduced a source occurrence.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct IncludeSite {
    pub parent: FileId,
    pub directive: Span,
}

/// Metadata supplied when adding one source occurrence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceFileSpec {
    pub display_name: String,
    pub spelled_path: PathBuf,
    pub resolved_path: PathBuf,
    pub identity: Option<FileIdentity>,
    pub include_site: Option<IncludeSite>,
    pub system_header: bool,
}

impl SourceFileSpec {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        Self {
            display_name: path.to_string_lossy().into_owned(),
            spelled_path: path.clone(),
            resolved_path: path,
            identity: None,
            include_site: None,
            system_header: false,
        }
    }

    pub fn with_display_name(mut self, display_name: impl Into<String>) -> Self {
        self.display_name = display_name.into();
        self
    }

    pub fn with_resolved_path(mut self, resolved_path: impl Into<PathBuf>) -> Self {
        self.resolved_path = resolved_path.into();
        self
    }

    pub fn with_identity(mut self, identity: FileIdentity) -> Self {
        self.identity = Some(identity);
        self
    }

    pub fn included_from(mut self, parent: FileId, directive: Span) -> Self {
        self.include_site = Some(IncludeSite { parent, directive });
        self
    }

    pub fn as_system_header(mut self) -> Self {
        self.system_header = true;
        self
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct LogicalLineMapping {
    physical_line: usize,
    presumed_line: usize,
    presumed_file: Option<String>,
}

#[derive(Debug)]
struct SourceFile {
    spec: SourceFileSpec,
    source: String,
    line_starts: Vec<usize>,
    logical_lines: Vec<LogicalLineMapping>,
    system_header_transitions: Vec<(usize, bool)>,
}

impl SourceFile {
    fn new(spec: SourceFileSpec, source: String) -> Self {
        let mut line_starts = vec![0];
        let bytes = source.as_bytes();
        let mut index = 0;
        while index < bytes.len() {
            match bytes[index] {
                b'\r' => {
                    index += if bytes.get(index + 1) == Some(&b'\n') {
                        2
                    } else {
                        1
                    };
                    line_starts.push(index);
                }
                b'\n' => {
                    index += 1;
                    line_starts.push(index);
                }
                _ => index += 1,
            }
        }
        Self {
            system_header_transitions: vec![(0, spec.system_header)],
            spec,
            source,
            line_starts,
            logical_lines: Vec::new(),
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
        let line_end = self.source.as_bytes()[line_start..]
            .iter()
            .position(|byte| matches!(byte, b'\r' | b'\n'))
            .map_or(self.source.len(), |index| line_start + index);
        let column = self.source[line_start..offset].chars().count() + 1;

        Some(SourceLocation {
            line: line_index + 1,
            column,
            line_start,
            line_end,
        })
    }

    fn presumed_location(&self, offset: usize) -> Option<PresumedLocation<'_>> {
        let physical = self.location(offset)?;
        let mapping_index = self
            .logical_lines
            .partition_point(|mapping| mapping.physical_line <= physical.line);
        let active_mapping = mapping_index
            .checked_sub(1)
            .and_then(|index| self.logical_lines.get(index));

        let line = active_mapping.map_or(physical.line, |mapping| {
            mapping.presumed_line + physical.line.saturating_sub(mapping.physical_line)
        });
        let file_name = self.logical_lines[..mapping_index]
            .iter()
            .rev()
            .find_map(|mapping| mapping.presumed_file.as_deref())
            .unwrap_or(&self.spec.display_name);

        Some(PresumedLocation {
            file_name,
            line,
            column: physical.column,
            physical_line: physical.line,
            line_start: physical.line_start,
            line_end: physical.line_end,
        })
    }

    fn line_text(&self, line: usize) -> Option<&str> {
        let line_start = *self.line_starts.get(line.checked_sub(1)?)?;
        let line_end = self.source.as_bytes()[line_start..]
            .iter()
            .position(|byte| matches!(byte, b'\r' | b'\n'))
            .map_or(self.source.len(), |index| line_start + index);
        Some(&self.source[line_start..line_end])
    }

    fn system_header_at(&self, offset: usize) -> Option<bool> {
        if offset > self.source.len() || !self.source.is_char_boundary(offset) {
            return None;
        }
        let index = self
            .system_header_transitions
            .partition_point(|&(start, _)| start <= offset)
            .saturating_sub(1);
        self.system_header_transitions
            .get(index)
            .map(|&(_, state)| state)
    }
}

/// How a generated preprocessing token relates to source text.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum OriginKind {
    MacroExpansion {
        macro_name: String,
        invocation: Span,
        definition: Span,
    },
    ArgumentSubstitution {
        parameter: String,
        argument: Span,
        replacement: Span,
    },
    Stringization {
        operator: Span,
        argument: Span,
    },
    TokenPaste {
        operator: Span,
        left: Span,
        right: Span,
    },
}

/// One interned source-origin node.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Origin {
    pub kind: OriginKind,
    pub parent: OriginId,
    /// The source occurrence where expansion produced the token.
    pub include_file: FileId,
    /// The system-header state at the expansion point.
    pub system_header: bool,
}

/// A bounded ancestry lookup, ordered from the requested origin outwards.
#[derive(Clone, Debug)]
pub struct OriginTrace<'a> {
    pub frames: Vec<&'a Origin>,
    pub truncated: bool,
}

/// A bounded include lookup, ordered from the immediate includer outwards.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IncludeTrace {
    pub sites: Vec<IncludeSite>,
    pub truncated: bool,
}

/// An invalid source-map mutation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceMapError {
    UnknownFile(FileId),
    InvalidOffset { file: FileId, offset: usize },
    InvalidPhysicalLine { file: FileId, line: usize },
    InvalidPresumedLine(usize),
}

impl fmt::Display for SourceMapError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownFile(file) => write!(formatter, "unknown source occurrence {file:?}"),
            Self::InvalidOffset { file, offset } => {
                write!(formatter, "invalid byte offset {offset} for {file:?}")
            }
            Self::InvalidPhysicalLine { file, line } => {
                write!(formatter, "invalid physical line {line} for {file:?}")
            }
            Self::InvalidPresumedLine(line) => {
                write!(
                    formatter,
                    "presumed line number must be positive, got {line}"
                )
            }
        }
    }
}

impl std::error::Error for SourceMapError {}

/// Owns source occurrences and resolves spans for diagnostics and dumps.
#[derive(Debug, Default)]
pub struct SourceMap {
    files: Vec<SourceFile>,
    origins: Vec<Origin>,
    origin_interner: HashMap<Origin, OriginId>,
}

impl SourceMap {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a top-level source occurrence using the same spelled and resolved name.
    pub fn add_file(&mut self, name: impl Into<String>, source: impl Into<String>) -> FileId {
        let name = name.into();
        self.add_file_occurrence(
            SourceFileSpec::new(PathBuf::from(&name)).with_display_name(name),
            source,
        )
    }

    /// Adds a distinct source occurrence, even when its physical identity is already present.
    pub fn add_file_occurrence(
        &mut self,
        spec: SourceFileSpec,
        source: impl Into<String>,
    ) -> FileId {
        let id = FileId(self.files.len().try_into().expect("too many source files"));
        self.files.push(SourceFile::new(spec, source.into()));
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
            .map(|file| file.spec.display_name.as_str())
    }

    pub fn file_spec(&self, file: FileId) -> Option<&SourceFileSpec> {
        self.files.get(file.0 as usize).map(|file| &file.spec)
    }

    pub fn source_count(&self) -> usize {
        self.files.len()
    }

    pub fn location(&self, file: FileId, offset: usize) -> Option<SourceLocation> {
        self.files.get(file.0 as usize)?.location(offset)
    }

    pub fn presumed_location(&self, file: FileId, offset: usize) -> Option<PresumedLocation<'_>> {
        self.files.get(file.0 as usize)?.presumed_location(offset)
    }

    pub fn line_text(&self, file: FileId, line: usize) -> Option<&str> {
        self.files.get(file.0 as usize)?.line_text(line)
    }

    /// Records the presumed location beginning at `physical_line`.
    ///
    /// Preprocessor callers normally pass the physical line following a `#line`
    /// directive. Passing no file name retains the current presumed file name.
    pub fn add_line_mapping(
        &mut self,
        file: FileId,
        physical_line: usize,
        presumed_line: usize,
        presumed_file: Option<String>,
    ) -> Result<(), SourceMapError> {
        if presumed_line == 0 {
            return Err(SourceMapError::InvalidPresumedLine(presumed_line));
        }
        let source = self
            .files
            .get_mut(file.0 as usize)
            .ok_or(SourceMapError::UnknownFile(file))?;
        if physical_line == 0 || physical_line > source.line_starts.len() {
            return Err(SourceMapError::InvalidPhysicalLine {
                file,
                line: physical_line,
            });
        }

        let index = source
            .logical_lines
            .partition_point(|mapping| mapping.physical_line < physical_line);
        let mapping = LogicalLineMapping {
            physical_line,
            presumed_line,
            presumed_file,
        };
        if source
            .logical_lines
            .get(index)
            .is_some_and(|existing| existing.physical_line == physical_line)
        {
            source.logical_lines[index] = mapping;
        } else {
            source.logical_lines.insert(index, mapping);
        }
        Ok(())
    }

    /// Changes system-header state at `offset` and for all subsequent direct spans.
    pub fn set_system_header_from(
        &mut self,
        file: FileId,
        offset: usize,
        system_header: bool,
    ) -> Result<(), SourceMapError> {
        let source = self
            .files
            .get_mut(file.0 as usize)
            .ok_or(SourceMapError::UnknownFile(file))?;
        if offset > source.source.len() || !source.source.is_char_boundary(offset) {
            return Err(SourceMapError::InvalidOffset { file, offset });
        }
        let index = source
            .system_header_transitions
            .partition_point(|&(start, _)| start < offset);
        if source
            .system_header_transitions
            .get(index)
            .is_some_and(|&(start, _)| start == offset)
        {
            source.system_header_transitions[index] = (offset, system_header);
        } else {
            source
                .system_header_transitions
                .insert(index, (offset, system_header));
        }
        Ok(())
    }

    pub fn is_system_header_at(&self, file: FileId, offset: usize) -> Option<bool> {
        self.files.get(file.0 as usize)?.system_header_at(offset)
    }

    /// Returns the system-header snapshot associated with a span.
    pub fn is_system_header(&self, span: Span) -> bool {
        self.origin(span.origin).map_or_else(
            || {
                self.is_system_header_at(span.file, span.start)
                    .unwrap_or(false)
            },
            |origin| origin.system_header,
        )
    }

    /// Interns provenance using the include occurrence and system-header state at
    /// `expansion_site`.
    pub fn intern_origin(
        &mut self,
        kind: OriginKind,
        parent: OriginId,
        expansion_site: Span,
    ) -> OriginId {
        let origin = Origin {
            kind,
            parent,
            include_file: expansion_site.file,
            system_header: self.is_system_header(expansion_site),
        };
        if let Some(id) = self.origin_interner.get(&origin) {
            return *id;
        }
        let id_value = self
            .origins
            .len()
            .checked_add(1)
            .expect("too many source origins");
        let id = OriginId(id_value.try_into().expect("too many source origins"));
        self.origins.push(origin.clone());
        self.origin_interner.insert(origin, id);
        id
    }

    pub fn origin(&self, id: OriginId) -> Option<&Origin> {
        let index = id.0.checked_sub(1)?;
        self.origins.get(index as usize)
    }

    pub fn origin_trace(&self, mut id: OriginId, max_depth: usize) -> OriginTrace<'_> {
        let mut frames = Vec::new();
        while !id.is_direct() && frames.len() < max_depth {
            let Some(origin) = self.origin(id) else {
                break;
            };
            frames.push(origin);
            id = origin.parent;
        }
        OriginTrace {
            frames,
            truncated: !id.is_direct(),
        }
    }

    pub fn include_trace(&self, file: FileId, max_depth: usize) -> IncludeTrace {
        let mut sites = Vec::new();
        let mut current = file;
        while sites.len() < max_depth {
            let Some(site) = self.file_spec(current).and_then(|spec| spec.include_site) else {
                return IncludeTrace {
                    sites,
                    truncated: false,
                };
            };
            sites.push(site);
            current = site.parent;
        }
        IncludeTrace {
            truncated: self
                .file_spec(current)
                .and_then(|spec| spec.include_site)
                .is_some(),
            sites,
        }
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

/// Per-compilation state shared by all compiler phases.
#[derive(Debug, Default)]
pub struct Session {
    pub sources: SourceMap,
    pub config: EffectiveCompilationConfig,
}

impl Session {
    pub fn new(config: EffectiveCompilationConfig) -> Self {
        Self {
            sources: SourceMap::new(),
            config,
        }
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

    #[test]
    fn treats_cr_lf_and_crlf_as_source_line_endings() {
        let mut sources = SourceMap::new();
        let file = sources.add_file("mixed.c", "one\rtwo\r\nthree\nfour");

        assert_eq!(
            sources.location(file, 4).map(|location| location.line),
            Some(2)
        );
        assert_eq!(
            sources.location(file, 9).map(|location| location.line),
            Some(3)
        );
        assert_eq!(
            sources.location(file, 15).map(|location| location.line),
            Some(4)
        );
        assert_eq!(sources.line_text(file, 1), Some("one"));
        assert_eq!(sources.line_text(file, 2), Some("two"));
        assert_eq!(sources.line_text(file, 3), Some("three"));
    }

    #[test]
    fn assigns_distinct_ids_to_repeated_physical_files() {
        let mut sources = SourceMap::new();
        let identity = FileIdentity::DeviceInode {
            device: 4,
            inode: 12,
        };
        let first = sources.add_file_occurrence(
            SourceFileSpec::new("spelled/first.h")
                .with_resolved_path("/real/header.h")
                .with_identity(identity.clone()),
            "",
        );
        let second = sources.add_file_occurrence(
            SourceFileSpec::new("spelled/second.h")
                .with_resolved_path("/real/header.h")
                .with_identity(identity),
            "",
        );

        assert_ne!(first, second);
        assert_eq!(
            sources
                .file_spec(first)
                .and_then(|spec| spec.identity.as_ref()),
            sources
                .file_spec(second)
                .and_then(|spec| spec.identity.as_ref())
        );
    }

    #[test]
    fn applies_line_mappings_without_losing_physical_lines() {
        let mut sources = SourceMap::new();
        let file = sources.add_file("input.c", "one\ntwo\nthree\n");
        sources
            .add_line_mapping(file, 2, 40, Some("generated.c".into()))
            .unwrap();
        sources.add_line_mapping(file, 3, 7, None).unwrap();

        assert_eq!(
            sources.presumed_location(file, 8),
            Some(PresumedLocation {
                file_name: "generated.c",
                line: 7,
                column: 1,
                physical_line: 3,
                line_start: 8,
                line_end: 13,
            })
        );
        assert_eq!(sources.line_text(file, 3), Some("three"));
    }

    #[test]
    fn snapshots_system_header_state_in_interned_origins() {
        let mut sources = SourceMap::new();
        let file = sources.add_file("header.h", "before\nafter\n");
        let invocation = Span::new(file, 7, 12);
        sources.set_system_header_from(file, 7, true).unwrap();

        let origin = sources.intern_origin(
            OriginKind::MacroExpansion {
                macro_name: "VALUE".into(),
                invocation,
                definition: Span::new(file, 0, 6),
            },
            OriginId::DIRECT,
            invocation,
        );
        sources.set_system_header_from(file, 7, false).unwrap();

        assert!(sources.origin(origin).unwrap().system_header);
        assert!(sources.is_system_header(Span::with_origin(file, 7, 12, origin)));
        assert!(!sources.is_system_header(Span::new(file, 7, 12)));
    }

    #[test]
    fn returns_bounded_origin_and_include_traces() {
        let mut sources = SourceMap::new();
        let main = sources.add_file("main.c", "#include \"a.h\"\n");
        let directive = Span::new(main, 0, 14);
        let header = sources.add_file_occurrence(
            SourceFileSpec::new("a.h").included_from(main, directive),
            "X",
        );
        let invocation = Span::new(header, 0, 1);
        let first = sources.intern_origin(
            OriginKind::MacroExpansion {
                macro_name: "X".into(),
                invocation,
                definition: invocation,
            },
            OriginId::DIRECT,
            invocation,
        );
        let second = sources.intern_origin(
            OriginKind::Stringization {
                operator: invocation,
                argument: invocation,
            },
            first,
            invocation,
        );

        let origin_trace = sources.origin_trace(second, 1);
        assert_eq!(origin_trace.frames.len(), 1);
        assert!(origin_trace.truncated);
        assert_eq!(
            sources.include_trace(header, 4),
            IncludeTrace {
                sites: vec![IncludeSite {
                    parent: main,
                    directive,
                }],
                truncated: false,
            }
        );
    }
}
