use std::{collections::BTreeSet, fmt};

use cranelift_codegen::ir;
use cranelift_codegen::ir::instructions::InstructionMapper;
use object::{ObjectSection as _, ObjectSymbol as _, SectionKind};

/// Version of the stable key/value schema emitted by [`CodegenStats::write_tsv`].
pub const CODEGEN_STATS_SCHEMA_VERSION: u64 = 3;

/// Aggregate structure of the post-inlining Cranelift IR.
///
/// These counters describe the IR handed to Cranelift's own optimization and
/// machine-code lowering passes. Removed instructions which remain allocated in
/// Cranelift's data-flow graph are deliberately excluded. `values` counts the
/// parameters of blocks in the final layout plus the results of instructions in
/// those blocks; detached blocks, instructions, and their values do not count.
///
/// The `unused_*` counters are observational. They count allocated imported
/// entities which are not reachable from a live layout instruction or a
/// Cranelift function-level semantic root; CCC does not remove or otherwise
/// optimize those entities.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct IrStats {
    pub functions: u64,
    pub blocks: u64,
    pub values: u64,
    pub instructions: u64,
    pub call_instructions: u64,
    pub fixed_stack_slots: u64,
    pub fixed_stack_bytes: u64,
    pub dynamic_stack_slots: u64,
    pub signatures: u64,
    pub unused_signatures: u64,
    pub external_functions: u64,
    pub unused_external_functions: u64,
    pub global_values: u64,
    pub unused_global_values: u64,
    pub constants: u64,
    pub jump_tables: u64,
}

impl IrStats {
    pub(crate) fn record_function(&mut self, function: &ir::Function) {
        let mut blocks = 0_u64;
        let mut values = 0_u64;
        let mut instructions = 0_u64;
        let mut call_instructions = 0_u64;
        for block in function.layout.blocks() {
            blocks = blocks.saturating_add(1);
            values = values.saturating_add(count(function.dfg.block_params(block).len()));
            for instruction in function.layout.block_insts(block) {
                instructions = instructions.saturating_add(1);
                values = values.saturating_add(count(function.dfg.inst_results(instruction).len()));
                if function.dfg.insts[instruction].opcode().is_call() {
                    call_instructions = call_instructions.saturating_add(1);
                }
            }
        }

        self.functions = self.functions.saturating_add(1);
        self.blocks = self.blocks.saturating_add(blocks);
        self.values = self.values.saturating_add(values);
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
        let references = ImportedEntityReferences::from_function(function);
        self.signatures = self
            .signatures
            .saturating_add(count(function.dfg.signatures.len()));
        self.unused_signatures = self
            .unused_signatures
            .saturating_add(references.unused_signatures(function));
        self.external_functions = self
            .external_functions
            .saturating_add(count(function.dfg.ext_funcs.len()));
        self.unused_external_functions = self
            .unused_external_functions
            .saturating_add(references.unused_external_functions(function));
        self.global_values = self
            .global_values
            .saturating_add(count(function.global_values.len()));
        self.unused_global_values = self
            .unused_global_values
            .saturating_add(references.unused_global_values(function));
        self.constants = self
            .constants
            .saturating_add(count(function.dfg.constants.len()));
        self.jump_tables = self
            .jump_tables
            .saturating_add(count(function.dfg.jump_tables.len()));
    }
}

/// Imported entities reachable from the final CLIF layout.
///
/// Instructions are visited through Cranelift's generated
/// [`InstructionMapper`] interface so every instruction format's entity fields
/// participate without a CCC-maintained opcode list. A referenced external
/// function keeps its signature live. Global values additionally use
/// Cranelift's function-level `stack_limit` root and live dynamic stack-slot
/// scales, then follow `Load`/`IAddImm` base edges transitively.
#[derive(Default)]
struct ImportedEntityReferences {
    signatures: BTreeSet<ir::SigRef>,
    external_functions: BTreeSet<ir::FuncRef>,
    global_values: BTreeSet<ir::GlobalValue>,
    dynamic_stack_slots: BTreeSet<ir::DynamicStackSlot>,
}

impl ImportedEntityReferences {
    fn from_function(function: &ir::Function) -> Self {
        let mut references = Self::default();
        for block in function.layout.blocks() {
            for instruction in function.layout.block_insts(block) {
                let _ = function.dfg.insts[instruction].map(&mut references);
                // `call_signature` also follows a live try-call's exception
                // table. Its signature is the only imported entity kind held
                // by exception-table data; blocks and values are local IR.
                if let Some(signature) = function.dfg.call_signature(instruction) {
                    references.signatures.insert(signature);
                }
            }
        }

        for function_reference in references.external_functions.iter().copied() {
            references
                .signatures
                .insert(function.dfg.ext_funcs[function_reference].signature);
        }
        if let Some(stack_limit) = function.stack_limit {
            references.global_values.insert(stack_limit);
        }
        for slot in references.dynamic_stack_slots.iter().copied() {
            references
                .global_values
                .insert(function.get_dynamic_slot_scale(slot));
        }
        let mut pending = references.global_values.iter().copied().collect::<Vec<_>>();
        while let Some(global_value) = pending.pop() {
            let base = match function.global_values[global_value] {
                ir::GlobalValueData::Load { base, .. }
                | ir::GlobalValueData::IAddImm { base, .. } => Some(base),
                ir::GlobalValueData::VMContext
                | ir::GlobalValueData::Symbol { .. }
                | ir::GlobalValueData::DynScaleTargetConst { .. } => None,
            };
            if let Some(base) = base
                && references.global_values.insert(base)
            {
                pending.push(base);
            }
        }
        references
    }

    fn unused_signatures(&self, function: &ir::Function) -> u64 {
        unused(function.dfg.signatures.len(), self.signatures.len())
    }

    fn unused_external_functions(&self, function: &ir::Function) -> u64 {
        unused(function.dfg.ext_funcs.len(), self.external_functions.len())
    }

    fn unused_global_values(&self, function: &ir::Function) -> u64 {
        unused(function.global_values.len(), self.global_values.len())
    }
}

impl InstructionMapper for ImportedEntityReferences {
    fn map_value(&mut self, value: ir::Value) -> ir::Value {
        value
    }

    fn map_value_list(&mut self, value_list: ir::ValueList) -> ir::ValueList {
        value_list
    }

    fn map_global_value(&mut self, global_value: ir::GlobalValue) -> ir::GlobalValue {
        self.global_values.insert(global_value);
        global_value
    }

    fn map_jump_table(&mut self, jump_table: ir::JumpTable) -> ir::JumpTable {
        jump_table
    }

    fn map_exception_table(&mut self, exception_table: ir::ExceptionTable) -> ir::ExceptionTable {
        exception_table
    }

    fn map_block_call(&mut self, block_call: ir::BlockCall) -> ir::BlockCall {
        block_call
    }

    fn map_block(&mut self, block: ir::Block) -> ir::Block {
        block
    }

    fn map_func_ref(&mut self, func_ref: ir::FuncRef) -> ir::FuncRef {
        self.external_functions.insert(func_ref);
        func_ref
    }

    fn map_sig_ref(&mut self, sig_ref: ir::SigRef) -> ir::SigRef {
        self.signatures.insert(sig_ref);
        sig_ref
    }

    fn map_stack_slot(&mut self, stack_slot: ir::StackSlot) -> ir::StackSlot {
        stack_slot
    }

    fn map_dynamic_stack_slot(
        &mut self,
        dynamic_stack_slot: ir::DynamicStackSlot,
    ) -> ir::DynamicStackSlot {
        self.dynamic_stack_slots.insert(dynamic_stack_slot);
        dynamic_stack_slot
    }

    fn map_constant(&mut self, constant: ir::Constant) -> ir::Constant {
        constant
    }

    fn map_immediate(&mut self, immediate: ir::Immediate) -> ir::Immediate {
        immediate
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
    pub fn metrics(&self) -> [(&'static str, u64); 32] {
        [
            ("post_inline_ir.functions", self.post_inline_ir.functions),
            ("post_inline_ir.blocks", self.post_inline_ir.blocks),
            ("post_inline_ir.values", self.post_inline_ir.values),
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
                "post_inline_ir.unused_signatures",
                self.post_inline_ir.unused_signatures,
            ),
            (
                "post_inline_ir.external_functions",
                self.post_inline_ir.external_functions,
            ),
            (
                "post_inline_ir.unused_external_functions",
                self.post_inline_ir.unused_external_functions,
            ),
            (
                "post_inline_ir.global_values",
                self.post_inline_ir.global_values,
            ),
            (
                "post_inline_ir.unused_global_values",
                self.post_inline_ir.unused_global_values,
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

fn unused(allocated: usize, referenced: usize) -> u64 {
    count(
        allocated
            .checked_sub(referenced)
            .expect("referenced imported entities must belong to their allocated table"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ir_value_count_excludes_detached_dfg_entities() {
        let mut function = ir::Function::new();
        let live_block = function.dfg.make_block();
        function.dfg.append_block_param(live_block, ir::types::I32);
        function.layout.append_block(live_block);
        let live_instruction = function.dfg.make_inst(ir::InstructionData::UnaryImm {
            opcode: ir::Opcode::Iconst,
            imm: 7_i64.into(),
        });
        function
            .dfg
            .make_inst_results(live_instruction, ir::types::I32);
        function.layout.append_inst(live_instruction, live_block);

        let detached_block = function.dfg.make_block();
        function
            .dfg
            .append_block_param(detached_block, ir::types::I64);
        let detached_instruction = function.dfg.make_inst(ir::InstructionData::UnaryImm {
            opcode: ir::Opcode::Iconst,
            imm: 11_i64.into(),
        });
        function
            .dfg
            .make_inst_results(detached_instruction, ir::types::I64);

        let mut stats = IrStats::default();
        stats.record_function(&function);
        assert_eq!(stats.functions, 1);
        assert_eq!(stats.blocks, 1);
        assert_eq!(stats.instructions, 1);
        assert_eq!(stats.values, 2);
    }

    #[test]
    fn unused_imports_follow_only_live_cranelift_entity_references() {
        use cranelift_codegen::isa::CallConv;

        let mut function = ir::Function::new();
        let live_signature = function.import_signature(ir::Signature::new(CallConv::SystemV));
        let indirect_signature = function.import_signature(ir::Signature::new(CallConv::SystemV));
        let try_signature = function.import_signature(ir::Signature::new(CallConv::SystemV));
        let unused_function_signature =
            function.import_signature(ir::Signature::new(CallConv::SystemV));
        let _detached_signature = function.import_signature(ir::Signature::new(CallConv::SystemV));
        let live_function = function.import_function(ir::ExtFuncData {
            name: ir::ExternalName::testcase("live"),
            signature: live_signature,
            colocated: false,
            patchable: false,
        });
        let unused_function = function.import_function(ir::ExtFuncData {
            name: ir::ExternalName::testcase("unused"),
            signature: unused_function_signature,
            colocated: false,
            patchable: false,
        });

        let live_base = function.create_global_value(ir::GlobalValueData::Symbol {
            name: ir::ExternalName::testcase("live_data"),
            offset: 0_i64.into(),
            colocated: false,
            tls: false,
        });
        let live_derived = function.create_global_value(ir::GlobalValueData::IAddImm {
            base: live_base,
            offset: 8_i64.into(),
            global_type: ir::types::I64,
        });
        let stack_limit = function.create_global_value(ir::GlobalValueData::Symbol {
            name: ir::ExternalName::testcase("stack_limit"),
            offset: 0_i64.into(),
            colocated: false,
            tls: false,
        });
        let unused_global = function.create_global_value(ir::GlobalValueData::Symbol {
            name: ir::ExternalName::testcase("unused_data"),
            offset: 0_i64.into(),
            colocated: false,
            tls: false,
        });
        let live_dynamic_scale =
            function.create_global_value(ir::GlobalValueData::DynScaleTargetConst {
                vector_type: ir::types::I32X4,
            });
        let duplicate_dynamic_scale =
            function.create_global_value(ir::GlobalValueData::DynScaleTargetConst {
                vector_type: ir::types::I32X4,
            });
        let live_dynamic_type = function.dfg.make_dynamic_ty(ir::DynamicTypeData::new(
            ir::types::I32X4,
            live_dynamic_scale,
        ));
        let duplicate_dynamic_type = function.dfg.make_dynamic_ty(ir::DynamicTypeData::new(
            ir::types::I32X4,
            duplicate_dynamic_scale,
        ));
        let live_dynamic_slot = function.create_dynamic_stack_slot(ir::DynamicStackSlotData::new(
            ir::StackSlotKind::ExplicitDynamicSlot,
            live_dynamic_type,
        ));
        let duplicate_dynamic_slot =
            function.create_dynamic_stack_slot(ir::DynamicStackSlotData::new(
                ir::StackSlotKind::ExplicitDynamicSlot,
                duplicate_dynamic_type,
            ));
        function.stack_limit = Some(stack_limit);

        let block = function.dfg.make_block();
        function.dfg.append_block_param(block, ir::types::I32X4XN);
        function.layout.append_block(block);
        let call = function.dfg.make_inst(ir::InstructionData::Call {
            opcode: ir::Opcode::Call,
            args: ir::ValueList::new(),
            func_ref: live_function,
        });
        function.layout.append_inst(call, block);
        let callee_address = function.dfg.make_inst(ir::InstructionData::UnaryImm {
            opcode: ir::Opcode::Iconst,
            imm: 0_i64.into(),
        });
        function
            .dfg
            .make_inst_results(callee_address, ir::types::I64);
        function.layout.append_inst(callee_address, block);
        let mut indirect_arguments = ir::ValueList::new();
        indirect_arguments.push(
            function.dfg.inst_results(callee_address)[0],
            &mut function.dfg.value_lists,
        );
        let indirect_call = function.dfg.make_inst(ir::InstructionData::CallIndirect {
            opcode: ir::Opcode::CallIndirect,
            args: indirect_arguments,
            sig_ref: indirect_signature,
        });
        function.layout.append_inst(indirect_call, block);
        let try_return = function.dfg.make_block();
        let normal_return = ir::BlockCall::new(try_return, [], &mut function.dfg.value_lists);
        let exception = function
            .dfg
            .exception_tables
            .push(ir::ExceptionTableData::new(
                try_signature,
                normal_return,
                [],
            ));
        let mut try_arguments = ir::ValueList::new();
        try_arguments.push(
            function.dfg.inst_results(callee_address)[0],
            &mut function.dfg.value_lists,
        );
        let try_call = function
            .dfg
            .make_inst(ir::InstructionData::TryCallIndirect {
                opcode: ir::Opcode::TryCallIndirect,
                args: try_arguments,
                exception,
            });
        function.layout.append_inst(try_call, block);
        let address = function
            .dfg
            .make_inst(ir::InstructionData::UnaryGlobalValue {
                opcode: ir::Opcode::SymbolValue,
                global_value: live_derived,
            });
        function.dfg.make_inst_results(address, ir::types::I64);
        function.layout.append_inst(address, block);
        let dynamic_address = function
            .dfg
            .make_inst(ir::InstructionData::DynamicStackAddr {
                opcode: ir::Opcode::DynamicStackAddr,
                dynamic_stack_slot: live_dynamic_slot,
            });
        function
            .dfg
            .make_inst_results(dynamic_address, ir::types::I64);
        function.layout.append_inst(dynamic_address, block);

        let _detached_function_address = function.dfg.make_inst(ir::InstructionData::FuncAddr {
            opcode: ir::Opcode::FuncAddr,
            func_ref: unused_function,
        });
        let _detached_global_address =
            function
                .dfg
                .make_inst(ir::InstructionData::UnaryGlobalValue {
                    opcode: ir::Opcode::SymbolValue,
                    global_value: unused_global,
                });
        let _detached_dynamic_address =
            function
                .dfg
                .make_inst(ir::InstructionData::DynamicStackAddr {
                    opcode: ir::Opcode::DynamicStackAddr,
                    dynamic_stack_slot: duplicate_dynamic_slot,
                });

        let mut stats = IrStats::default();
        stats.record_function(&function);
        assert_eq!(stats.signatures, 5);
        assert_eq!(stats.unused_signatures, 2);
        assert!(stats.unused_signatures <= stats.signatures);
        assert_eq!(stats.external_functions, 2);
        assert_eq!(stats.unused_external_functions, 1);
        assert!(stats.unused_external_functions <= stats.external_functions);
        assert_eq!(stats.global_values, 6);
        assert_eq!(stats.unused_global_values, 2);
        assert!(stats.unused_global_values <= stats.global_values);
    }

    #[test]
    fn tsv_schema_is_versioned_and_deterministic() {
        let stats = CodegenStats {
            post_inline_ir: IrStats {
                functions: 2,
                values: 23,
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
        assert!(first.starts_with("schema_version\t3\npost_inline_ir.functions\t2\n"));
        assert!(first.contains("post_inline_ir.values\t23\n"));
        assert!(first.contains("post_inline_ir.instructions\t17\n"));
        assert!(first.contains("post_inline_ir.unused_signatures\t0\n"));
        assert!(first.contains("post_inline_ir.unused_external_functions\t0\n"));
        assert!(first.contains("post_inline_ir.unused_global_values\t0\n"));
        assert!(first.contains("primary_object.file_bytes\t4096\n"));
        assert!(first.ends_with("primary_object.other_section_bytes\t0\n"));
        assert_eq!(first.lines().count(), 33);
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
