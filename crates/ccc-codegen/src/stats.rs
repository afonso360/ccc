use std::fmt;

use cranelift_codegen::ir;
use object::{ObjectSection as _, ObjectSymbol as _, SectionKind};

/// Version of the stable key/value schema emitted by [`CodegenStats::write_tsv`].
pub const CODEGEN_STATS_SCHEMA_VERSION: u64 = 1;

/// Aggregate structure of the post-inlining Cranelift IR.
///
/// These counters describe the IR handed to Cranelift's own optimization and
/// machine-code lowering passes. Removed instructions which remain allocated in
/// Cranelift's data-flow graph are deliberately excluded.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct IrStats {
    pub functions: u64,
    pub blocks: u64,
    pub instructions: u64,
    pub call_instructions: u64,
    pub fixed_stack_slots: u64,
    pub fixed_stack_bytes: u64,
    pub dynamic_stack_slots: u64,
    pub signatures: u64,
    pub external_functions: u64,
    pub global_values: u64,
    pub constants: u64,
    pub jump_tables: u64,
}

impl IrStats {
    pub(crate) fn record_function(&mut self, function: &ir::Function) {
        let mut blocks = 0_u64;
        let mut instructions = 0_u64;
        let mut call_instructions = 0_u64;
        for block in function.layout.blocks() {
            blocks = blocks.saturating_add(1);
            for instruction in function.layout.block_insts(block) {
                instructions = instructions.saturating_add(1);
                if function.dfg.insts[instruction].opcode().is_call() {
                    call_instructions = call_instructions.saturating_add(1);
                }
            }
        }

        self.functions = self.functions.saturating_add(1);
        self.blocks = self.blocks.saturating_add(blocks);
        self.instructions = self.instructions.saturating_add(instructions);
        self.call_instructions = self.call_instructions.saturating_add(call_instructions);
        self.fixed_stack_slots = self
            .fixed_stack_slots
            .saturating_add(count(function.sized_stack_slots.len()));
        self.fixed_stack_bytes = self
            .fixed_stack_bytes
            .saturating_add(u64::from(function.fixed_stack_size()));
        self.dynamic_stack_slots = self
            .dynamic_stack_slots
            .saturating_add(count(function.dynamic_stack_slots.len()));
        self.signatures = self
            .signatures
            .saturating_add(count(function.dfg.signatures.len()));
        self.external_functions = self
            .external_functions
            .saturating_add(count(function.dfg.ext_funcs.len()));
        self.global_values = self
            .global_values
            .saturating_add(count(function.global_values.len()));
        self.constants = self
            .constants
            .saturating_add(count(function.dfg.constants.len()));
        self.jump_tables = self
            .jump_tables
            .saturating_add(count(function.dfg.jump_tables.len()));
    }
}

/// Aggregate metrics for the primary relocatable object emitted by Cranelift.
///
/// Section byte buckets are disjoint and use logical section sizes, so BSS and
/// uninitialized TLS are represented even though they occupy no object payload.
/// The generated bridge assembly units in [`crate::Output::assemblies`] are not
/// part of these metrics.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PrimaryObjectStats {
    pub file_bytes: u64,
    pub sections: u64,
    pub symbols: u64,
    pub defined_symbols: u64,
    pub undefined_symbols: u64,
    pub relocations: u64,
    pub text_bytes: u64,
    pub read_only_data_bytes: u64,
    pub writable_data_bytes: u64,
    pub bss_bytes: u64,
    pub tls_data_bytes: u64,
    pub tls_bss_bytes: u64,
    pub unwind_bytes: u64,
    pub debug_bytes: u64,
    pub metadata_bytes: u64,
    pub other_section_bytes: u64,
}

impl PrimaryObjectStats {
    /// Inspect a relocatable object using format-independent `object` traits.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, object::Error> {
        let object = object::File::parse(bytes)?;
        Ok(Self::from_object(&object, bytes.len()))
    }

    pub(crate) fn from_object<'data>(
        object: &impl object::Object<'data>,
        file_bytes: usize,
    ) -> Self {
        let mut stats = Self {
            file_bytes: count(file_bytes),
            ..Self::default()
        };

        for section in object.sections() {
            stats.sections = stats.sections.saturating_add(1);
            stats.relocations = stats
                .relocations
                .saturating_add(count(section.relocations().count()));
            let size = section.size();
            let name = section.name().unwrap_or_default();
            let bucket = section_bucket(section.kind(), name);
            match bucket {
                SectionBucket::Text => {
                    stats.text_bytes = stats.text_bytes.saturating_add(size);
                }
                SectionBucket::ReadOnlyData => {
                    stats.read_only_data_bytes = stats.read_only_data_bytes.saturating_add(size);
                }
                SectionBucket::WritableData => {
                    stats.writable_data_bytes = stats.writable_data_bytes.saturating_add(size);
                }
                SectionBucket::Bss => {
                    stats.bss_bytes = stats.bss_bytes.saturating_add(size);
                }
                SectionBucket::TlsData => {
                    stats.tls_data_bytes = stats.tls_data_bytes.saturating_add(size);
                }
                SectionBucket::TlsBss => {
                    stats.tls_bss_bytes = stats.tls_bss_bytes.saturating_add(size);
                }
                SectionBucket::Unwind => {
                    stats.unwind_bytes = stats.unwind_bytes.saturating_add(size);
                }
                SectionBucket::Debug => {
                    stats.debug_bytes = stats.debug_bytes.saturating_add(size);
                }
                SectionBucket::Metadata => {
                    stats.metadata_bytes = stats.metadata_bytes.saturating_add(size);
                }
                SectionBucket::Other => {
                    stats.other_section_bytes = stats.other_section_bytes.saturating_add(size);
                }
            }
        }

        for symbol in object.symbols() {
            stats.symbols = stats.symbols.saturating_add(1);
            if symbol.is_undefined() {
                stats.undefined_symbols = stats.undefined_symbols.saturating_add(1);
            } else {
                stats.defined_symbols = stats.defined_symbols.saturating_add(1);
            }
        }
        stats
    }

    /// Sum of all logical section sizes represented by the disjoint buckets.
    pub fn section_bytes(&self) -> u64 {
        [
            self.text_bytes,
            self.read_only_data_bytes,
            self.writable_data_bytes,
            self.bss_bytes,
            self.tls_data_bytes,
            self.tls_bss_bytes,
            self.unwind_bytes,
            self.debug_bytes,
            self.metadata_bytes,
            self.other_section_bytes,
        ]
        .into_iter()
        .fold(0, u64::saturating_add)
    }
}

/// Stable compiler-side statistics for one code-generation invocation.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CodegenStats {
    pub post_inline_ir: IrStats,
    pub primary_object: PrimaryObjectStats,
}

impl CodegenStats {
    /// Return the metrics in the stable schema order used by [`Self::write_tsv`].
    pub fn metrics(&self) -> [(&'static str, u64); 28] {
        [
            ("post_inline_ir.functions", self.post_inline_ir.functions),
            ("post_inline_ir.blocks", self.post_inline_ir.blocks),
            (
                "post_inline_ir.instructions",
                self.post_inline_ir.instructions,
            ),
            (
                "post_inline_ir.call_instructions",
                self.post_inline_ir.call_instructions,
            ),
            (
                "post_inline_ir.fixed_stack_slots",
                self.post_inline_ir.fixed_stack_slots,
            ),
            (
                "post_inline_ir.fixed_stack_bytes",
                self.post_inline_ir.fixed_stack_bytes,
            ),
            (
                "post_inline_ir.dynamic_stack_slots",
                self.post_inline_ir.dynamic_stack_slots,
            ),
            ("post_inline_ir.signatures", self.post_inline_ir.signatures),
            (
                "post_inline_ir.external_functions",
                self.post_inline_ir.external_functions,
            ),
            (
                "post_inline_ir.global_values",
                self.post_inline_ir.global_values,
            ),
            ("post_inline_ir.constants", self.post_inline_ir.constants),
            (
                "post_inline_ir.jump_tables",
                self.post_inline_ir.jump_tables,
            ),
            ("primary_object.file_bytes", self.primary_object.file_bytes),
            ("primary_object.sections", self.primary_object.sections),
            ("primary_object.symbols", self.primary_object.symbols),
            (
                "primary_object.defined_symbols",
                self.primary_object.defined_symbols,
            ),
            (
                "primary_object.undefined_symbols",
                self.primary_object.undefined_symbols,
            ),
            (
                "primary_object.relocations",
                self.primary_object.relocations,
            ),
            ("primary_object.text_bytes", self.primary_object.text_bytes),
            (
                "primary_object.read_only_data_bytes",
                self.primary_object.read_only_data_bytes,
            ),
            (
                "primary_object.writable_data_bytes",
                self.primary_object.writable_data_bytes,
            ),
            ("primary_object.bss_bytes", self.primary_object.bss_bytes),
            (
                "primary_object.tls_data_bytes",
                self.primary_object.tls_data_bytes,
            ),
            (
                "primary_object.tls_bss_bytes",
                self.primary_object.tls_bss_bytes,
            ),
            (
                "primary_object.unwind_bytes",
                self.primary_object.unwind_bytes,
            ),
            (
                "primary_object.debug_bytes",
                self.primary_object.debug_bytes,
            ),
            (
                "primary_object.metadata_bytes",
                self.primary_object.metadata_bytes,
            ),
            (
                "primary_object.other_section_bytes",
                self.primary_object.other_section_bytes,
            ),
        ]
    }

    /// Write deterministic tab-separated key/value rows.
    ///
    /// The first row versions the schema. All values, including the version,
    /// are unsigned decimal integers.
    pub fn write_tsv(&self, output: &mut impl fmt::Write) -> fmt::Result {
        writeln!(output, "schema_version\t{CODEGEN_STATS_SCHEMA_VERSION}")?;
        for (metric, value) in self.metrics() {
            writeln!(output, "{metric}\t{value}")?;
        }
        Ok(())
    }

    /// Render deterministic tab-separated key/value rows.
    pub fn to_tsv(&self) -> String {
        let mut output = String::new();
        self.write_tsv(&mut output)
            .expect("writing codegen statistics to a String cannot fail");
        output
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SectionBucket {
    Text,
    ReadOnlyData,
    WritableData,
    Bss,
    TlsData,
    TlsBss,
    Unwind,
    Debug,
    Metadata,
    Other,
}

fn section_bucket(kind: SectionKind, name: &str) -> SectionBucket {
    if is_unwind_section(name) {
        return SectionBucket::Unwind;
    }
    if matches!(kind, SectionKind::Debug | SectionKind::DebugString) || is_debug_section(name) {
        return SectionBucket::Debug;
    }
    match kind {
        SectionKind::Text => SectionBucket::Text,
        SectionKind::ReadOnlyData
        | SectionKind::ReadOnlyDataWithRel
        | SectionKind::ReadOnlyString => SectionBucket::ReadOnlyData,
        SectionKind::Data => SectionBucket::WritableData,
        SectionKind::UninitializedData | SectionKind::Common => SectionBucket::Bss,
        SectionKind::Tls | SectionKind::TlsVariables => SectionBucket::TlsData,
        SectionKind::UninitializedTls => SectionBucket::TlsBss,
        SectionKind::Metadata | SectionKind::Linker | SectionKind::Note => SectionBucket::Metadata,
        _ => SectionBucket::Other,
    }
}

fn is_unwind_section(name: &str) -> bool {
    matches!(
        name,
        ".eh_frame"
            | "__eh_frame"
            | ".eh_frame_hdr"
            | "__eh_frame_hdr"
            | ".compact_unwind"
            | "__compact_unwind"
            | ".unwind_info"
            | "__unwind_info"
            | ".pdata"
            | ".xdata"
    ) || name.starts_with(".gcc_except_table")
        || name.starts_with("__gcc_except_tab")
}

fn is_debug_section(name: &str) -> bool {
    name.starts_with(".debug_")
        || name.starts_with("__debug_")
        || name.starts_with(".zdebug_")
        || name.starts_with("__zdebug_")
}

fn count(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tsv_schema_is_versioned_and_deterministic() {
        let stats = CodegenStats {
            post_inline_ir: IrStats {
                functions: 2,
                instructions: 17,
                ..IrStats::default()
            },
            primary_object: PrimaryObjectStats {
                file_bytes: 4096,
                text_bytes: 128,
                ..PrimaryObjectStats::default()
            },
        };
        let first = stats.to_tsv();
        assert_eq!(first, stats.to_tsv());
        assert!(first.starts_with("schema_version\t1\npost_inline_ir.functions\t2\n"));
        assert!(first.contains("post_inline_ir.instructions\t17\n"));
        assert!(first.contains("primary_object.file_bytes\t4096\n"));
        assert!(first.ends_with("primary_object.other_section_bytes\t0\n"));
        assert_eq!(first.lines().count(), 29);
    }

    #[test]
    fn section_name_fallbacks_are_cross_format_and_disjoint() {
        for name in [
            ".eh_frame",
            "__compact_unwind",
            ".gcc_except_table.foo",
            ".pdata",
        ] {
            assert_eq!(
                section_bucket(SectionKind::ReadOnlyData, name),
                SectionBucket::Unwind
            );
        }
        for name in [".debug_info", "__debug_line", ".zdebug_str"] {
            assert_eq!(
                section_bucket(SectionKind::Other, name),
                SectionBucket::Debug
            );
        }
        assert_eq!(
            section_bucket(SectionKind::UninitializedTls, ".tbss"),
            SectionBucket::TlsBss
        );
        assert_eq!(
            section_bucket(SectionKind::Metadata, ".symtab"),
            SectionBucket::Metadata
        );
    }
}
