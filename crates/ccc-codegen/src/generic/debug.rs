//! Source-level DWARF emission for compiler-generated object files.

use std::collections::{BTreeSet, HashMap};

use ccc_ir::generic as gir;
use ccc_sema::generic::{Linkage, ObjectDefinitionPolicy, StorageDuration};
use ccc_session::{SourceMap, Span};
use ccc_target::{AbiIdentity, EffectiveCompilationConfig};
use ccc_types::{
    ArrayLength, BuiltinType, FunctionParameters, LayoutShape, QualifiedType, RecordKind, TypeId,
    TypeKind, TypeQualifiers, TypeStore,
};
use cranelift_codegen::ir::{SourceLoc, ValueLabel};
use cranelift_codegen::isa::TargetIsa;
use cranelift_codegen::{Context, LabelValueLoc};
use cranelift_module::FuncId;
use cranelift_object::ObjectProduct;
use gimli::constants;
use gimli::write::{
    Address, AttributeValue, DwarfUnit, EndianVec, Expression, LineProgram, LineString, Location,
    LocationList, Range, RangeList, RelocateWriter, Relocation, RelocationTarget, Sections,
    UnitEntryId, Writer,
};
use gimli::{Encoding, Format, LineEncoding, Register, SectionId};
use object::write::{Relocation as ObjectRelocation, StandardSegment, SymbolId, SymbolSection};
use object::{BinaryFormat, RelocationEncoding, RelocationFlags, RelocationKind, SectionKind};

use super::function::FunctionDebugLayout;
use super::{Declarations, error};
use crate::CodegenError;

/// Interns exact source spans into Cranelift's opaque 32-bit source-location
/// field. The reverse table remains owned by this compilation invocation.
#[derive(Default)]
pub(super) struct SourceLocationRegistry {
    spans: Vec<Span>,
    ids: HashMap<Span, u32>,
}

impl SourceLocationRegistry {
    pub(super) fn intern(&mut self, span: Span) -> Result<SourceLoc, CodegenError> {
        if let Some(id) = self.ids.get(&span).copied() {
            return Ok(SourceLoc::new(id));
        }
        let id = u32::try_from(self.spans.len()).map_err(|_| {
            error("debug source-location table exceeds Cranelift's 32-bit identity space")
        })?;
        // All-ones is Cranelift's reserved unknown location.
        if id == u32::MAX {
            return Err(error(
                "debug source-location table exhausted Cranelift's source identities",
            ));
        }
        self.spans.push(span);
        self.ids.insert(span, id);
        Ok(SourceLoc::new(id))
    }

    fn resolve(&self, location: SourceLoc) -> Option<Span> {
        (!location.is_default())
            .then(|| self.spans.get(location.bits() as usize).copied())
            .flatten()
    }
}

#[derive(Clone)]
struct FunctionRecord {
    function_index: usize,
    id: FuncId,
    code_size: u64,
    rows: Vec<(u64, Span)>,
    frame_locations: HashMap<u32, i64>,
    parameter_locations: HashMap<u32, Vec<ValueLocationRange>>,
}

#[derive(Clone, Copy)]
struct ValueLocationRange {
    start: u64,
    end: u64,
    location: ValueLocation,
}

#[derive(Clone, Copy)]
enum ValueLocation {
    Register(Register),
    CfaOffset(i64),
}

pub(super) struct DebugEmitter<'a> {
    module: &'a gir::FullModule,
    config: &'a EffectiveCompilationConfig,
    sources: &'a SourceMap,
    pub(super) locations: SourceLocationRegistry,
    functions: Vec<FunctionRecord>,
}

impl<'a> DebugEmitter<'a> {
    pub(super) fn new(
        module: &'a gir::FullModule,
        config: &'a EffectiveCompilationConfig,
        sources: &'a SourceMap,
    ) -> Self {
        Self {
            module,
            config,
            sources,
            locations: SourceLocationRegistry::default(),
            functions: Vec::new(),
        }
    }

    pub(super) fn record_function(
        &mut self,
        function_index: usize,
        id: FuncId,
        context: &Context,
        lowering: &FunctionDebugLayout,
        isa: &dyn TargetIsa,
    ) -> Result<(), CodegenError> {
        let function = &self.module.functions[function_index];
        let compiled = context.compiled_code().ok_or_else(|| {
            error(format!(
                "function `{}` has no compiled machine code for debug emission",
                function.symbol_name
            ))
        })?;
        let code_size = u64::from(compiled.code_info().total_size);
        let mut rows = vec![(0, function.span)];
        for mapping in compiled.buffer.get_srclocs_sorted() {
            let Some(span) = self.locations.resolve(mapping.loc) else {
                continue;
            };
            let offset = u64::from(mapping.start);
            if rows.last().is_some_and(|(previous_offset, previous_span)| {
                *previous_offset == offset && *previous_span == span
            }) {
                continue;
            }
            rows.push((offset, span));
        }
        rows.sort_unstable_by_key(|row| row.0);
        rows.dedup_by(|right, left| {
            right.0 == left.0 || self.source_position(right.1) == self.source_position(left.1)
        });

        let mut frame_locations = HashMap::new();
        if let Some(frame) = compiled.buffer.frame_layout() {
            for (storage, slot) in &lowering.storage_slots {
                let Some(location) = frame.stackslots.get(*slot) else {
                    continue;
                };
                let offset = i64::from(location.offset) - i64::from(frame.frame_to_fp_offset);
                frame_locations.insert(*storage, offset);
            }
        }
        let mut parameter_locations = HashMap::new();
        for index in 0..function.parameters.len() {
            let index = u32::try_from(index)
                .map_err(|_| error("debug parameter index exceeds Cranelift's label space"))?;
            let Some(ranges) = compiled
                .value_labels_ranges
                .get(&ValueLabel::from_u32(index))
            else {
                continue;
            };
            let mut mapped = Vec::with_capacity(ranges.len());
            for range in ranges {
                let start = u64::from(range.start).min(code_size);
                let end = u64::from(range.end).min(code_size);
                if start >= end {
                    continue;
                }
                let location = match range.loc {
                    LabelValueLoc::Reg(register) => ValueLocation::Register(Register(
                        isa.map_regalloc_reg_to_dwarf(register).map_err(|failure| {
                            error(format!(
                                "cannot map a debug value register in `{}`: {failure}",
                                function.symbol_name
                            ))
                        })?,
                    )),
                    LabelValueLoc::CFAOffset(offset) => ValueLocation::CfaOffset(offset),
                };
                mapped.push(ValueLocationRange {
                    start,
                    end,
                    location,
                });
            }
            if !mapped.is_empty() {
                parameter_locations.insert(index, mapped);
            }
        }
        self.functions.push(FunctionRecord {
            function_index,
            id,
            code_size,
            rows,
            frame_locations,
            parameter_locations,
        });
        Ok(())
    }

    pub(super) fn emit(
        self,
        product: &mut ObjectProduct,
        declarations: &Declarations,
    ) -> Result<(), CodegenError> {
        let encoding = Encoding {
            format: Format::Dwarf32,
            version: 4,
            address_size: 8,
        };
        let Some(primary_span) = self
            .functions
            .first()
            .map(|function| function.rows[0].1)
            .or_else(|| self.module.globals.first().map(|global| global.span))
            .or_else(|| self.module.functions.first().map(|function| function.span))
        else {
            return Ok(());
        };
        let primary_name = self
            .sources
            .presumed_location(primary_span.file, primary_span.start)
            .map_or("<unknown>", |location| location.file_name)
            .to_owned();
        let working_directory = source_directory();
        let mut dwarf = DwarfUnit::new(encoding);
        let mut line_program = LineProgram::new(
            encoding,
            LineEncoding::default(),
            LineString::String(path_bytes(&working_directory)),
            None,
            LineString::String(path_bytes(&primary_name)),
            None,
        );
        let mut file_ids = HashMap::new();
        let primary_file = line_program.add_file(
            LineString::String(path_bytes(&primary_name)),
            line_program.default_directory(),
            None,
        );
        file_ids.insert(primary_name.clone(), primary_file);

        let mut targets = Vec::new();
        let mut function_targets = HashMap::new();
        for record in &self.functions {
            let symbol = product.function_symbol(record.id);
            let index = targets.len();
            targets.push(absolute_relocation_destination(product, symbol)?);
            function_targets.insert(record.function_index, index);
        }

        for record in &self.functions {
            let target = function_targets[&record.function_index];
            line_program.begin_sequence(Some(Address::Symbol {
                symbol: target,
                addend: 0,
            }));
            let mut last_offset = None;
            for &(offset, span) in &record.rows {
                if offset > record.code_size || last_offset == Some(offset) {
                    continue;
                }
                let Some(location) = self.sources.presumed_location(span.file, span.start) else {
                    continue;
                };
                let file = ensure_line_file(&mut line_program, &mut file_ids, location.file_name);
                let row = line_program.row();
                row.address_offset = offset;
                row.file = file;
                row.line = location.line as u64;
                row.column = location.column as u64;
                row.is_statement = true;
                line_program.generate_row();
                last_offset = Some(offset);
            }
            line_program.end_sequence(record.code_size);
        }
        for function in &self.module.functions {
            ensure_span_file(
                &mut line_program,
                &mut file_ids,
                self.sources,
                function.span,
            );
            for parameter in &function.parameters {
                ensure_span_file(
                    &mut line_program,
                    &mut file_ids,
                    self.sources,
                    parameter.span,
                );
            }
            for storage in &function.storage {
                ensure_span_file(&mut line_program, &mut file_ids, self.sources, storage.span);
            }
        }
        for global in &self.module.globals {
            ensure_span_file(&mut line_program, &mut file_ids, self.sources, global.span);
        }
        dwarf.unit.line_program = line_program;

        let root = dwarf.unit.root();
        let root_entry = dwarf.unit.get_mut(root);
        set_string(root_entry, constants::DW_AT_name, &primary_name);
        set_string(root_entry, constants::DW_AT_comp_dir, &working_directory);
        set_string(root_entry, constants::DW_AT_producer, "ccc");
        root_entry.set(
            constants::DW_AT_language,
            AttributeValue::Language(constants::DW_LANG_C11),
        );
        root_entry.set(constants::DW_AT_use_UTF8, AttributeValue::Flag(true));
        if !self.functions.is_empty() && product.object.format() == BinaryFormat::MachO {
            let first = self
                .functions
                .iter()
                .min_by_key(|record| targets[function_targets[&record.function_index]].addend)
                .expect("nonempty function records have a first entry");
            let first_target = function_targets[&first.function_index];
            let start = targets[first_target].addend;
            let end = self.functions.iter().try_fold(start, |end, record| {
                let target = targets[function_targets[&record.function_index]].addend;
                let size = i64::try_from(record.code_size)
                    .map_err(|_| error("Mach-O debug function size exceeds signed range"))?;
                target
                    .checked_add(size)
                    .map(|function_end| end.max(function_end))
                    .ok_or_else(|| error("Mach-O debug compilation-unit range overflow"))
            })?;
            let length = u64::try_from(end - start)
                .map_err(|_| error("Mach-O debug compilation-unit range is invalid"))?;
            let entry = dwarf.unit.get_mut(root);
            entry.set(
                constants::DW_AT_low_pc,
                AttributeValue::Address(Address::Symbol {
                    symbol: first_target,
                    addend: 0,
                }),
            );
            entry.set(constants::DW_AT_high_pc, AttributeValue::Udata(length));
        } else if !self.functions.is_empty() {
            let ranges = self
                .functions
                .iter()
                .map(|record| Range::StartLength {
                    begin: Address::Symbol {
                        symbol: function_targets[&record.function_index],
                        addend: 0,
                    },
                    length: record.code_size,
                })
                .collect();
            let ranges = dwarf.unit.ranges.add(RangeList(ranges));
            dwarf.unit.get_mut(root).set(
                constants::DW_AT_ranges,
                AttributeValue::RangeListRef(ranges),
            );
        }

        let mut types = TypeEmitter::new(&mut dwarf.unit, &self.module.types, self.config, root);
        types.emit_all()?;

        let frame_register = frame_pointer_register(self.config.target.abi);
        for record in &self.functions {
            let function = &self.module.functions[record.function_index];
            let target = function_targets[&record.function_index];
            let subprogram = types.unit.add(root, constants::DW_TAG_subprogram);
            let result_type = (function.result_type.ty != TypeId::VOID)
                .then(|| types.qualified(function.result_type));
            let function_type = self
                .module
                .types
                .function_signature(function.signature)
                .ok_or_else(|| {
                    error(format!(
                        "function `{}` has a non-function debug signature",
                        function.name
                    ))
                })?;
            {
                let entry = types.unit.get_mut(subprogram);
                set_string(entry, constants::DW_AT_name, &function.name);
                if function.symbol_name != function.name {
                    set_string(entry, constants::DW_AT_linkage_name, &function.symbol_name);
                }
                entry.set(
                    constants::DW_AT_external,
                    AttributeValue::Flag(function.linkage == Linkage::External),
                );
                if matches!(&function_type.parameters, FunctionParameters::Prototype(_)) {
                    entry.set(constants::DW_AT_prototyped, AttributeValue::Flag(true));
                }
                entry.set(
                    constants::DW_AT_low_pc,
                    AttributeValue::Address(Address::Symbol {
                        symbol: target,
                        addend: 0,
                    }),
                );
                entry.set(
                    constants::DW_AT_high_pc,
                    AttributeValue::Udata(record.code_size),
                );
                if let Some(result_type) = result_type {
                    entry.set(constants::DW_AT_type, AttributeValue::UnitRef(result_type));
                }
                set_decl_location(entry, self.sources, &file_ids, function.span);
                let mut frame_base = Expression::new();
                frame_base.op(constants::DW_OP_call_frame_cfa);
                entry.set(
                    constants::DW_AT_frame_base,
                    AttributeValue::Exprloc(frame_base),
                );
            }

            let parameter_locals = function
                .parameters
                .iter()
                .map(|parameter| parameter.local)
                .collect::<BTreeSet<_>>();
            for (parameter_index, parameter) in function.parameters.iter().enumerate() {
                let entry_id = types
                    .unit
                    .add(subprogram, constants::DW_TAG_formal_parameter);
                let type_id = types.qualified(parameter.ty);
                let frame_location = parameter
                    .storage
                    .and_then(|storage| record.frame_locations.get(&storage.0).copied());
                let value_location = if frame_location.is_none() {
                    let parameter_index = u32::try_from(parameter_index).map_err(|_| {
                        error("debug parameter index exceeds Cranelift's label space")
                    })?;
                    if let Some(ranges) = record.parameter_locations.get(&parameter_index) {
                        let list = value_location_list(
                            product.object.format(),
                            target,
                            targets[target].addend,
                            ranges,
                        )?;
                        Some(types.unit.locations.add(list))
                    } else {
                        None
                    }
                } else {
                    None
                };
                let entry = types.unit.get_mut(entry_id);
                set_string(entry, constants::DW_AT_name, &parameter.name);
                entry.set(constants::DW_AT_type, AttributeValue::UnitRef(type_id));
                set_decl_location(entry, self.sources, &file_ids, parameter.span);
                if let Some(offset) = frame_location {
                    entry.set(
                        constants::DW_AT_location,
                        AttributeValue::Exprloc(frame_expression(frame_register, offset)),
                    );
                } else if let Some(location) = value_location {
                    entry.set(
                        constants::DW_AT_location,
                        AttributeValue::LocationListRef(location),
                    );
                }
            }
            if function_type.variadic {
                types
                    .unit
                    .add(subprogram, constants::DW_TAG_unspecified_parameters);
            }

            let lexical = types.unit.add(subprogram, constants::DW_TAG_lexical_block);
            {
                let entry = types.unit.get_mut(lexical);
                entry.set(
                    constants::DW_AT_low_pc,
                    AttributeValue::Address(Address::Symbol {
                        symbol: target,
                        addend: 0,
                    }),
                );
                entry.set(
                    constants::DW_AT_high_pc,
                    AttributeValue::Udata(record.code_size),
                );
            }
            for storage in &function.storage {
                if parameter_locals.contains(&storage.local)
                    || storage.duration != StorageDuration::Automatic
                {
                    continue;
                }
                let variable = types.unit.add(lexical, constants::DW_TAG_variable);
                let type_id = types.qualified(storage.ty);
                let entry = types.unit.get_mut(variable);
                set_string(entry, constants::DW_AT_name, &storage.name);
                entry.set(constants::DW_AT_type, AttributeValue::UnitRef(type_id));
                set_decl_location(entry, self.sources, &file_ids, storage.span);
                if let Some(offset) = record.frame_locations.get(&storage.id.0).copied() {
                    entry.set(
                        constants::DW_AT_location,
                        AttributeValue::Exprloc(frame_expression(frame_register, offset)),
                    );
                }
            }
        }

        for global in self
            .module
            .globals
            .iter()
            .filter(|global| global.emission.definition != ObjectDefinitionPolicy::Declaration)
        {
            let Some(declaration) = declarations.globals.get(&global.id.0) else {
                continue;
            };
            let relocation_kind = if declaration.tls {
                if self.config.target.abi != AbiIdentity::SysvAmd64Lp64 {
                    return Err(error(format!(
                        "thread-local debug locations are unsupported for target ABI `{}`",
                        self.config.target.abi.name()
                    )));
                }
                DwarfRelocationKind::X86TlsOffset
            } else {
                DwarfRelocationKind::Absolute
            };
            let variable = types.unit.add(root, constants::DW_TAG_variable);
            let type_id = types.qualified(global.ty);
            let entry = types.unit.get_mut(variable);
            set_string(entry, constants::DW_AT_name, &global.name);
            entry.set(constants::DW_AT_type, AttributeValue::UnitRef(type_id));
            entry.set(
                constants::DW_AT_external,
                AttributeValue::Flag(global.linkage == Linkage::External),
            );
            set_decl_location(entry, self.sources, &file_ids, global.span);
            let target = targets.len();
            let symbol = product.data_symbol(declaration.id);
            targets.push(if relocation_kind == DwarfRelocationKind::Absolute {
                absolute_relocation_destination(product, symbol)?
            } else {
                RelocationDestination {
                    symbol,
                    addend: 0,
                    kind: relocation_kind,
                }
            });
            let mut location = Expression::new();
            location.op_addr(Address::Symbol {
                symbol: target,
                addend: 0,
            });
            if declaration.tls {
                location.op(constants::DW_OP_form_tls_address);
            }
            entry.set(constants::DW_AT_location, AttributeValue::Exprloc(location));
        }

        let mut sections = Sections::new(DwarfSection::default());
        dwarf
            .write(&mut sections)
            .map_err(|failure| error(format!("failed to encode DWARF: {failure}")))?;
        write_sections(product, sections, &targets).map_err(error)
    }

    fn source_position(&self, span: Span) -> Option<(String, usize, usize)> {
        self.sources
            .presumed_location(span.file, span.start)
            .map(|location| {
                (
                    location.file_name.to_owned(),
                    location.line,
                    location.column,
                )
            })
    }
}

fn source_directory() -> String {
    std::env::current_dir()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|_| ".".to_owned())
}

fn path_bytes(path: &str) -> Vec<u8> {
    path.as_bytes()
        .iter()
        .copied()
        .filter(|byte| *byte != 0)
        .collect()
}

fn ensure_line_file(
    program: &mut LineProgram,
    files: &mut HashMap<String, gimli::write::FileId>,
    name: &str,
) -> gimli::write::FileId {
    *files.entry(name.to_owned()).or_insert_with(|| {
        program.add_file(
            LineString::String(path_bytes(name)),
            program.default_directory(),
            None,
        )
    })
}

fn ensure_span_file(
    program: &mut LineProgram,
    files: &mut HashMap<String, gimli::write::FileId>,
    sources: &SourceMap,
    span: Span,
) {
    if let Some(location) = sources.presumed_location(span.file, span.start) {
        ensure_line_file(program, files, location.file_name);
    }
}

fn set_string(
    entry: &mut gimli::write::DebuggingInformationEntry,
    name: constants::DwAt,
    value: &str,
) {
    entry.set(name, AttributeValue::String(path_bytes(value)));
}

fn set_decl_location(
    entry: &mut gimli::write::DebuggingInformationEntry,
    sources: &SourceMap,
    files: &HashMap<String, gimli::write::FileId>,
    span: Span,
) {
    let Some(location) = sources.presumed_location(span.file, span.start) else {
        return;
    };
    if let Some(file) = files.get(location.file_name).copied() {
        entry.set(
            constants::DW_AT_decl_file,
            AttributeValue::FileIndex(Some(file)),
        );
    }
    entry.set(
        constants::DW_AT_decl_line,
        AttributeValue::Udata(location.line as u64),
    );
    entry.set(
        constants::DW_AT_decl_column,
        AttributeValue::Udata(location.column as u64),
    );
}

fn frame_pointer_register(abi: AbiIdentity) -> Register {
    Register(match abi {
        AbiIdentity::SysvAmd64Lp64 => 6,
        AbiIdentity::Aapcs64Lp64 | AbiIdentity::DarwinArm64 => 29,
        AbiIdentity::RiscvLp64d => 8,
    })
}

fn frame_expression(register: Register, offset: i64) -> Expression {
    let mut expression = Expression::new();
    expression.op_breg(register, offset);
    expression
}

fn value_location_expression(location: ValueLocation) -> Expression {
    let mut expression = Expression::new();
    match location {
        ValueLocation::Register(register) => expression.op_reg(register),
        // Every subprogram defines its frame base as the canonical frame
        // address, so a regalloc spill location can use the compact fbreg form.
        ValueLocation::CfaOffset(offset) => expression.op_fbreg(offset),
    }
    expression
}

fn value_location_list(
    format: BinaryFormat,
    target: usize,
    function_offset: i64,
    ranges: &[ValueLocationRange],
) -> Result<LocationList, CodegenError> {
    if format == BinaryFormat::MachO {
        // dsymutil rebases raw input-section addresses through the linker's
        // debug map.  This is the conventional Darwin DWARF v4 representation
        // and avoids relocatable base-address-selection entries that dsymutil
        // can discard while coalescing location lists.
        let function_offset = u64::try_from(function_offset)
            .map_err(|_| error("Mach-O debug function offset is negative"))?;
        let mut locations = Vec::with_capacity(ranges.len());
        for range in ranges {
            let begin = function_offset
                .checked_add(range.start)
                .ok_or_else(|| error("Mach-O debug value range start overflow"))?;
            let end = function_offset
                .checked_add(range.end)
                .ok_or_else(|| error("Mach-O debug value range end overflow"))?;
            locations.push(Location::StartEnd {
                begin: Address::Constant(begin),
                end: Address::Constant(end),
                data: value_location_expression(range.location),
            });
        }
        return Ok(LocationList(locations));
    }

    let mut locations = Vec::with_capacity(ranges.len() + 1);
    locations.push(Location::BaseAddress {
        address: Address::Symbol {
            symbol: target,
            addend: 0,
        },
    });
    locations.extend(ranges.iter().map(|range| Location::OffsetPair {
        begin: range.start,
        end: range.end,
        data: value_location_expression(range.location),
    }));
    Ok(LocationList(locations))
}

struct TypeEmitter<'a> {
    unit: &'a mut gimli::write::Unit,
    store: &'a TypeStore,
    config: &'a EffectiveCompilationConfig,
    root: UnitEntryId,
    entries: HashMap<TypeId, UnitEntryId>,
    qualified_entries: HashMap<(TypeId, bool, bool, bool, bool), UnitEntryId>,
}

impl<'a> TypeEmitter<'a> {
    fn new(
        unit: &'a mut gimli::write::Unit,
        store: &'a TypeStore,
        config: &'a EffectiveCompilationConfig,
        root: UnitEntryId,
    ) -> Self {
        Self {
            unit,
            store,
            config,
            root,
            entries: HashMap::new(),
            qualified_entries: HashMap::new(),
        }
    }

    fn emit_all(&mut self) -> Result<(), CodegenError> {
        for (id, _) in self.store.iter_types() {
            self.entries.insert(id, self.unit.reserve());
        }
        for (id, kind) in self.store.iter_types() {
            self.emit_type(id, kind.clone())?;
        }
        Ok(())
    }

    fn emit_type(&mut self, id: TypeId, kind: TypeKind) -> Result<(), CodegenError> {
        let entry = self.entries[&id];
        let tag = match &kind {
            TypeKind::Builtin(BuiltinType::Void) => constants::DW_TAG_unspecified_type,
            TypeKind::Builtin(_) => constants::DW_TAG_base_type,
            TypeKind::Pointer(_) => constants::DW_TAG_pointer_type,
            TypeKind::Array(_) => constants::DW_TAG_array_type,
            TypeKind::Function(_) => constants::DW_TAG_subroutine_type,
            TypeKind::Enum(_) => constants::DW_TAG_enumeration_type,
            TypeKind::Record(record) => {
                match self.store.record(*record).map(|record| record.kind) {
                    Some(RecordKind::Union) => constants::DW_TAG_union_type,
                    Some(RecordKind::Struct) | None => constants::DW_TAG_structure_type,
                }
            }
            TypeKind::AlignmentAdjusted(_) => constants::DW_TAG_typedef,
        };
        self.unit.add_reserved(entry, self.root, tag);
        match kind {
            TypeKind::Builtin(builtin) => self.emit_builtin(entry, id, builtin)?,
            TypeKind::Pointer(pointer) => {
                self.unit
                    .get_mut(entry)
                    .set(constants::DW_AT_byte_size, AttributeValue::Udata(8));
                let pointee = self.qualified(pointer.pointee);
                self.unit
                    .get_mut(entry)
                    .set(constants::DW_AT_type, AttributeValue::UnitRef(pointee));
            }
            TypeKind::Array(array) => {
                let element = self.qualified(array.element);
                self.unit
                    .get_mut(entry)
                    .set(constants::DW_AT_type, AttributeValue::UnitRef(element));
                if let Ok(layout) = self.store.layout_of(id, self.config) {
                    self.unit.get_mut(entry).set(
                        constants::DW_AT_byte_size,
                        AttributeValue::Udata(layout.size),
                    );
                }
                let subrange = self.unit.add(entry, constants::DW_TAG_subrange_type);
                if let ArrayLength::Constant(length) = array.length {
                    self.unit
                        .get_mut(subrange)
                        .set(constants::DW_AT_count, AttributeValue::Udata(length));
                }
            }
            TypeKind::Function(function) => {
                if function.result.ty != TypeId::VOID {
                    let result = self.qualified(function.result);
                    self.unit
                        .get_mut(entry)
                        .set(constants::DW_AT_type, AttributeValue::UnitRef(result));
                }
                match function.parameters {
                    FunctionParameters::Prototype(parameters) => {
                        self.unit
                            .get_mut(entry)
                            .set(constants::DW_AT_prototyped, AttributeValue::Flag(true));
                        for parameter in parameters {
                            let parameter_entry =
                                self.unit.add(entry, constants::DW_TAG_formal_parameter);
                            let ty = self.qualified(parameter);
                            self.unit
                                .get_mut(parameter_entry)
                                .set(constants::DW_AT_type, AttributeValue::UnitRef(ty));
                        }
                        if function.variadic {
                            self.unit
                                .add(entry, constants::DW_TAG_unspecified_parameters);
                        }
                    }
                    FunctionParameters::Unspecified => {
                        self.unit
                            .add(entry, constants::DW_TAG_unspecified_parameters);
                    }
                }
            }
            TypeKind::Enum(enum_id) => {
                let Some(enumeration) = self.store.enumeration(enum_id) else {
                    return Ok(());
                };
                if let Some(tag) = &enumeration.tag {
                    set_string(self.unit.get_mut(entry), constants::DW_AT_name, tag);
                }
                let Some(body) = &enumeration.body else {
                    self.unit
                        .get_mut(entry)
                        .set(constants::DW_AT_declaration, AttributeValue::Flag(true));
                    return Ok(());
                };
                if let Ok(layout) = self.store.layout_of(id, self.config) {
                    self.unit.get_mut(entry).set(
                        constants::DW_AT_byte_size,
                        AttributeValue::Udata(layout.size),
                    );
                }
                let underlying = self.entries[&body.underlying];
                self.unit
                    .get_mut(entry)
                    .set(constants::DW_AT_type, AttributeValue::UnitRef(underlying));
                for enumerator in &body.enumerators {
                    let child = self.unit.add(entry, constants::DW_TAG_enumerator);
                    set_string(
                        self.unit.get_mut(child),
                        constants::DW_AT_name,
                        &enumerator.name,
                    );
                    let value = i64::try_from(enumerator.value).unwrap_or_else(|_| {
                        if enumerator.value.is_negative() {
                            i64::MIN
                        } else {
                            i64::MAX
                        }
                    });
                    self.unit
                        .get_mut(child)
                        .set(constants::DW_AT_const_value, AttributeValue::Sdata(value));
                }
            }
            TypeKind::Record(record_id) => {
                let Some(record) = self.store.record(record_id) else {
                    return Ok(());
                };
                if let Some(tag) = &record.tag {
                    set_string(self.unit.get_mut(entry), constants::DW_AT_name, tag);
                }
                let Some(fields) = &record.fields else {
                    self.unit
                        .get_mut(entry)
                        .set(constants::DW_AT_declaration, AttributeValue::Flag(true));
                    return Ok(());
                };
                let layout = self.store.layout_of(id, self.config).map_err(|failure| {
                    error(format!(
                        "failed to lay out debug type {}: {failure}",
                        id.index()
                    ))
                })?;
                self.unit.get_mut(entry).set(
                    constants::DW_AT_byte_size,
                    AttributeValue::Udata(layout.size),
                );
                let LayoutShape::Record(record_layout) = layout.shape else {
                    return Err(error("record debug type did not produce record layout"));
                };
                for (field, field_layout) in fields.iter().zip(&record_layout.fields) {
                    let child = self.unit.add(entry, constants::DW_TAG_member);
                    if let Some(name) = &field.name {
                        set_string(self.unit.get_mut(child), constants::DW_AT_name, name);
                    }
                    let ty = self.qualified(field.ty);
                    let child_entry = self.unit.get_mut(child);
                    child_entry.set(constants::DW_AT_type, AttributeValue::UnitRef(ty));
                    child_entry.set(
                        constants::DW_AT_data_member_location,
                        AttributeValue::Udata(field_layout.offset),
                    );
                    if let Some(bitfield) = field_layout.bitfield {
                        child_entry.set(
                            constants::DW_AT_bit_size,
                            AttributeValue::Udata(u64::from(bitfield.width)),
                        );
                        child_entry.set(
                            constants::DW_AT_data_bit_offset,
                            AttributeValue::Udata(
                                bitfield.storage_offset * 8 + u64::from(bitfield.bit_offset),
                            ),
                        );
                    }
                }
            }
            TypeKind::AlignmentAdjusted(adjusted) => {
                let underlying = self.entries[&adjusted.underlying];
                self.unit
                    .get_mut(entry)
                    .set(constants::DW_AT_type, AttributeValue::UnitRef(underlying));
            }
        }
        Ok(())
    }

    fn emit_builtin(
        &mut self,
        entry: UnitEntryId,
        id: TypeId,
        builtin: BuiltinType,
    ) -> Result<(), CodegenError> {
        set_string(
            self.unit.get_mut(entry),
            constants::DW_AT_name,
            builtin.spelling(),
        );
        if builtin == BuiltinType::Void {
            return Ok(());
        }
        let layout = self
            .store
            .layout_of(id, self.config)
            .map_err(|failure| error(format!("failed to lay out debug base type: {failure}")))?;
        let encoding = match builtin {
            BuiltinType::Bool => constants::DW_ATE_boolean,
            BuiltinType::Float16
            | BuiltinType::Float
            | BuiltinType::Double
            | BuiltinType::LongDouble => constants::DW_ATE_float,
            BuiltinType::Char if self.config.target.data_layout.char_is_signed => {
                constants::DW_ATE_signed_char
            }
            BuiltinType::Char | BuiltinType::UnsignedChar => constants::DW_ATE_unsigned_char,
            BuiltinType::SignedChar => constants::DW_ATE_signed_char,
            BuiltinType::UnsignedShort
            | BuiltinType::UnsignedInt
            | BuiltinType::UnsignedLong
            | BuiltinType::UnsignedLongLong
            | BuiltinType::UnsignedInt128 => constants::DW_ATE_unsigned,
            BuiltinType::Void => return Ok(()),
            _ => constants::DW_ATE_signed,
        };
        let entry = self.unit.get_mut(entry);
        entry.set(
            constants::DW_AT_byte_size,
            AttributeValue::Udata(layout.size),
        );
        entry.set(
            constants::DW_AT_encoding,
            AttributeValue::Encoding(encoding),
        );
        Ok(())
    }

    fn qualified(&mut self, ty: QualifiedType) -> UnitEntryId {
        let key = (
            ty.ty,
            ty.qualifiers.contains(TypeQualifiers::CONST),
            ty.qualifiers.contains(TypeQualifiers::VOLATILE),
            ty.qualifiers.contains(TypeQualifiers::RESTRICT),
            ty.qualifiers.contains(TypeQualifiers::ATOMIC),
        );
        if let Some(entry) = self.qualified_entries.get(&key).copied() {
            return entry;
        }
        let mut current = self.entries[&ty.ty];
        for (present, tag) in [
            (key.1, constants::DW_TAG_const_type),
            (key.2, constants::DW_TAG_volatile_type),
            (key.3, constants::DW_TAG_restrict_type),
            (key.4, constants::DW_TAG_atomic_type),
        ] {
            if !present {
                continue;
            }
            let wrapper = self.unit.add(self.root, tag);
            self.unit
                .get_mut(wrapper)
                .set(constants::DW_AT_type, AttributeValue::UnitRef(current));
            current = wrapper;
        }
        self.qualified_entries.insert(key, current);
        current
    }
}

#[derive(Clone, Copy)]
struct RelocationDestination {
    symbol: SymbolId,
    addend: i64,
    kind: DwarfRelocationKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DwarfRelocationKind {
    Absolute,
    X86TlsOffset,
}

/// Apple debug maps retain section contributions and their linked addresses,
/// not a complete mapping for every private symbol in an input object.  A
/// DWARF relocation against a private function symbol can therefore collapse
/// to the first atom in the section when `dsymutil` resolves it.  Express
/// defined Mach-O addresses as the containing section plus the symbol's exact
/// section-relative offset, which is also the form emitted by Apple's compiler
/// toolchain.
fn absolute_relocation_destination(
    product: &mut ObjectProduct,
    symbol: SymbolId,
) -> Result<RelocationDestination, CodegenError> {
    let (section, value) = {
        let symbol = product.object.symbol(symbol);
        (symbol.section, symbol.value)
    };
    if product.object.format() == BinaryFormat::MachO {
        if let SymbolSection::Section(section) = section {
            let addend = i64::try_from(value)
                .map_err(|_| error("Mach-O debug symbol offset exceeds signed relocation range"))?;
            return Ok(RelocationDestination {
                symbol: product.object.section_symbol(section),
                addend,
                kind: DwarfRelocationKind::Absolute,
            });
        }
    }
    Ok(RelocationDestination {
        symbol,
        addend: 0,
        kind: DwarfRelocationKind::Absolute,
    })
}

struct DwarfSection {
    writer: EndianVec<gimli::LittleEndian>,
    relocations: Vec<Relocation>,
}

impl Default for DwarfSection {
    fn default() -> Self {
        Self {
            writer: EndianVec::new(gimli::LittleEndian),
            relocations: Vec::new(),
        }
    }
}

impl Clone for DwarfSection {
    fn clone(&self) -> Self {
        debug_assert_eq!(self.writer.slice(), &[]);
        debug_assert!(self.relocations.is_empty());
        Self::default()
    }
}

impl RelocateWriter for DwarfSection {
    type Writer = EndianVec<gimli::LittleEndian>;

    fn writer(&self) -> &Self::Writer {
        &self.writer
    }

    fn writer_mut(&mut self) -> &mut Self::Writer {
        &mut self.writer
    }

    fn relocate(&mut self, relocation: Relocation) {
        self.relocations.push(relocation);
    }
}

fn write_sections(
    product: &mut ObjectProduct,
    mut sections: Sections<DwarfSection>,
    targets: &[RelocationDestination],
) -> Result<(), String> {
    rewrite_tls_location_opcodes(&mut sections.debug_info.0, targets)?;
    let sections = vec![
        (SectionId::DebugAbbrev, sections.debug_abbrev.0),
        (SectionId::DebugStr, sections.debug_str.0),
        (SectionId::DebugLineStr, sections.debug_line_str.0),
        (SectionId::DebugLine, sections.debug_line.0),
        (SectionId::DebugRanges, sections.debug_ranges.0),
        (SectionId::DebugRngLists, sections.debug_rnglists.0),
        (SectionId::DebugLoc, sections.debug_loc.0),
        (SectionId::DebugLocLists, sections.debug_loclists.0),
        (SectionId::DebugInfo, sections.debug_info.0),
    ]
    .into_iter()
    .filter(|(_, section)| !section.writer.slice().is_empty())
    .collect::<Vec<_>>();

    let format = product.object.format();
    let segment = product.object.segment_name(StandardSegment::Debug).to_vec();
    let mut object_sections = HashMap::new();
    for (id, section) in &sections {
        let name = dwarf_section_name(format, *id);
        let kind = if matches!(*id, SectionId::DebugStr | SectionId::DebugLineStr) {
            SectionKind::DebugString
        } else {
            SectionKind::Debug
        };
        let object_section = product.object.add_section(segment.clone(), name, kind);
        product
            .object
            .append_section_data(object_section, section.writer.slice(), 1);
        object_sections.insert(*id, object_section);
    }

    for (id, section) in sections {
        let source = object_sections[&id];
        for relocation in section.relocations {
            if relocation.eh_pe.is_some() {
                return Err(format!("unexpected unwind relocation in `{}`", id.name()));
            }
            let offset = u64::try_from(relocation.offset)
                .map_err(|_| "DWARF relocation offset does not fit object format".to_owned())?;
            // Mach-O debug-map consumers treat DW_FORM_sec_offset values as
            // offsets within the corresponding input debug section.  Apple
            // objects therefore encode these values directly rather than
            // carrying relocations between __DWARF sections; dsymutil can
            // otherwise reinterpret a zero offset as a text address.
            if format == BinaryFormat::MachO
                && matches!(relocation.target, RelocationTarget::Section(_))
            {
                write_section_offset(
                    product.object.section_mut(source).data_mut(),
                    offset,
                    relocation.size,
                    relocation.addend,
                )?;
                continue;
            }
            let (symbol, addend, relocation_kind) = match relocation.target {
                RelocationTarget::Symbol(index) => {
                    let target = targets.get(index).ok_or_else(|| {
                        format!("DWARF relocation references unknown target {index}")
                    })?;
                    (target.symbol, target.addend, target.kind)
                }
                RelocationTarget::Section(section) => {
                    let target = object_sections.get(&section).copied().ok_or_else(|| {
                        format!("DWARF relocation references absent `{}`", section.name())
                    })?;
                    (
                        product.object.section_symbol(target),
                        0,
                        DwarfRelocationKind::Absolute,
                    )
                }
            };
            let addend = addend
                .checked_add(relocation.addend)
                .ok_or_else(|| "DWARF relocation addend overflow".to_owned())?;
            let flags = match relocation_kind {
                DwarfRelocationKind::Absolute => RelocationFlags::Generic {
                    kind: RelocationKind::Absolute,
                    encoding: RelocationEncoding::Generic,
                    size: relocation.size.saturating_mul(8),
                },
                DwarfRelocationKind::X86TlsOffset => {
                    if id != SectionId::DebugInfo
                        || format != BinaryFormat::Elf
                        || relocation.size != 8
                    {
                        return Err(
                            "x86 TLS debug relocation has an invalid section, format, or width"
                                .to_owned(),
                        );
                    }
                    RelocationFlags::Elf {
                        r_type: object::elf::R_X86_64_DTPOFF64,
                    }
                }
            };
            product
                .object
                .add_relocation(
                    source,
                    ObjectRelocation {
                        offset,
                        symbol,
                        addend,
                        flags,
                    },
                )
                .map_err(|failure| {
                    format!("failed to record relocation in `{}`: {failure}", id.name())
                })?;
        }
    }
    Ok(())
}

fn write_section_offset(
    section: &mut [u8],
    offset: u64,
    size: u8,
    value: i64,
) -> Result<(), String> {
    let offset = usize::try_from(offset)
        .map_err(|_| "DWARF section offset does not fit host address space".to_owned())?;
    let value = u64::try_from(value).map_err(|_| "DWARF section offset is negative".to_owned())?;
    let width = usize::from(size);
    let end = offset
        .checked_add(width)
        .ok_or_else(|| "DWARF section offset write overflow".to_owned())?;
    let destination = section
        .get_mut(offset..end)
        .ok_or_else(|| "DWARF section offset write is out of bounds".to_owned())?;
    match size {
        4 => destination.copy_from_slice(
            &u32::try_from(value)
                .map_err(|_| "DWARF32 section offset overflow".to_owned())?
                .to_le_bytes(),
        ),
        8 => destination.copy_from_slice(&value.to_le_bytes()),
        _ => {
            return Err(format!(
                "unsupported {size}-byte Mach-O DWARF section offset"
            ));
        }
    }
    Ok(())
}

fn rewrite_tls_location_opcodes(
    section: &mut DwarfSection,
    targets: &[RelocationDestination],
) -> Result<(), String> {
    let mut opcode_offsets = Vec::new();
    for relocation in &section.relocations {
        let RelocationTarget::Symbol(index) = relocation.target else {
            continue;
        };
        let target = targets
            .get(index)
            .ok_or_else(|| format!("DWARF TLS expression references unknown target {index}"))?;
        if target.kind != DwarfRelocationKind::X86TlsOffset {
            continue;
        }
        if relocation.size != 8 || relocation.offset == 0 {
            return Err("DWARF TLS expression has an invalid relocated operand".to_owned());
        }
        let following_opcode = relocation
            .offset
            .checked_add(usize::from(relocation.size))
            .ok_or_else(|| "DWARF TLS expression offset overflow".to_owned())?;
        let bytes = section.writer.slice();
        if bytes.get(relocation.offset - 1) != Some(&constants::DW_OP_addr.0)
            || bytes.get(following_opcode) != Some(&constants::DW_OP_form_tls_address.0)
        {
            return Err("DWARF TLS expression does not have the expected address form".to_owned());
        }
        opcode_offsets.push(relocation.offset - 1);
    }
    for offset in opcode_offsets {
        section
            .writer
            .write_u8_at(offset, constants::DW_OP_const8u.0)
            .map_err(|failure| format!("failed to encode DWARF TLS expression: {failure}"))?;
    }
    Ok(())
}

fn dwarf_section_name(format: BinaryFormat, id: SectionId) -> Vec<u8> {
    let name = id.name();
    if format == BinaryFormat::MachO {
        format!("__{}", name.trim_start_matches('.')).into_bytes()
    } else {
        name.as_bytes().to_vec()
    }
}
