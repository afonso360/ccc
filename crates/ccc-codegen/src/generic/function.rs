use super::data::{low_mask, scalar_constant_bits, string_unit_bytes};
use super::*;

pub(super) struct FunctionReferences {
    functions: HashMap<u32, FunctionReference>,
    globals: HashMap<u32, DataReference>,
    strings: HashMap<u32, ir::GlobalValue>,
    call_helper: Option<ir::FuncRef>,
}

#[derive(Clone, Copy)]
pub(super) enum DefinitionAbi<'a> {
    Native(&'a ccc_abi::NativeBoundaryPlan),
    Variadic(&'a ccc_abi::BridgeBoundaryPlan),
}

#[derive(Clone, Copy)]
struct FunctionReference {
    address: ir::FuncRef,
    direct_call: ir::FuncRef,
}

#[derive(Clone, Copy)]
struct DataReference {
    value: ir::GlobalValue,
    tls: bool,
}

pub(super) fn declare_function_references(
    declarations: &Declarations,
    object_module: &mut ObjectModule,
    function: &mut ir::Function,
) -> FunctionReferences {
    let mut function_declarations = declarations.functions.iter().collect::<Vec<_>>();
    function_declarations.sort_unstable_by_key(|(raw, _)| **raw);
    let mut functions = HashMap::with_capacity(function_declarations.len());
    for (raw, id) in function_declarations {
        let address = object_module.declare_func_in_func(*id, function);
        let mut direct_call_data = function.dfg.ext_funcs[address].clone();
        // A native C direct call uses the target's PC-relative call
        // relocation. Keep a distinct reference for address materialization,
        // whose linkage and preemption semantics remain module-defined.
        direct_call_data.colocated = true;
        let direct_call = function.import_function(direct_call_data);
        functions.insert(
            *raw,
            FunctionReference {
                address,
                direct_call,
            },
        );
    }
    let mut global_declarations = declarations.globals.iter().collect::<Vec<_>>();
    global_declarations.sort_unstable_by_key(|(raw, _)| **raw);
    let mut globals = HashMap::with_capacity(global_declarations.len());
    for (raw, declaration) in global_declarations {
        globals.insert(
            *raw,
            DataReference {
                value: object_module.declare_data_in_func(declaration.id, function),
                tls: declaration.tls,
            },
        );
    }
    let mut string_declarations = declarations.strings.iter().collect::<Vec<_>>();
    string_declarations.sort_unstable_by_key(|(raw, _)| **raw);
    let mut strings = HashMap::with_capacity(string_declarations.len());
    for (raw, id) in string_declarations {
        strings.insert(*raw, object_module.declare_data_in_func(*id, function));
    }
    let call_helper = declarations
        .call_helper
        .map(|id| object_module.declare_func_in_func(id, function));
    FunctionReferences {
        functions,
        globals,
        strings,
        call_helper,
    }
}

pub(super) fn lower_function(
    module: &gir::FullModule,
    function: &gir::FullFunction,
    config: &EffectiveCompilationConfig,
    abi_plan: ccc_abi::VerifiedModuleAbiPlan<'_>,
    definition_plan: DefinitionAbi<'_>,
    references: &FunctionReferences,
    clif_function: &mut ir::Function,
) -> Result<(), CodegenError> {
    let entry = function.entry.ok_or_else(|| {
        error(format!(
            "function definition `{}` has no entry block",
            function.symbol_name
        ))
    })?;
    let mut builder_context = FunctionBuilderContext::new();
    let mut builder = FunctionBuilder::new(clif_function, &mut builder_context);
    let mut blocks = HashMap::with_capacity(function.blocks.len());
    for block in &function.blocks {
        if blocks.insert(block.id.0, builder.create_block()).is_some() {
            return Err(error(format!(
                "function `{}` contains duplicate block {}",
                function.symbol_name, block.id.0
            )));
        }
    }
    for block in &function.blocks {
        let clif_block = block_ref(&blocks, block.id.0)?;
        if block.id == entry {
            builder.append_block_params_for_function_params(clif_block);
        } else {
            for parameter in &block.parameters {
                let ty = value_type(function, *parameter)?;
                builder.append_block_param(
                    clif_block,
                    value_representation_type(&module.types, ty, config)?,
                );
            }
        }
    }

    let mut storage = HashMap::new();
    for object in &function.storage {
        if object.location != gir::StorageLocation::Automatic {
            continue;
        }
        let layout = object_layout(&module.types, object.ty, config)?;
        let size = u32::try_from(layout.size).map_err(|_| {
            error(format!(
                "automatic object `{}` is too large for a Cranelift stack slot",
                object.name
            ))
        })?;
        let align_shift = u8::try_from(layout.align.trailing_zeros())
            .map_err(|_| error("stack object alignment is too large"))?;
        let slot = builder.create_sized_stack_slot(StackSlotData::new(
            StackSlotKind::ExplicitSlot,
            size,
            align_shift,
        ));
        if storage.insert(object.id.0, slot).is_some() {
            return Err(error(format!(
                "function `{}` contains duplicate storage id {}",
                function.symbol_name, object.id.0
            )));
        }
    }

    let mut state = FunctionState {
        module,
        function,
        config,
        abi_plan,
        definition_plan,
        references,
        blocks,
        storage,
        values: vec![None; function.value_types.len()],
        sret: None,
        variadic_state: None,
        variadic_frame: None,
    };
    for block in &function.blocks {
        let clif_block = state.block(block.id.0)?;
        if block.id == entry {
            continue;
        }
        for (parameter, clif_value) in block
            .parameters
            .iter()
            .zip(builder.block_params(clif_block).iter().copied())
        {
            state.set_value(*parameter, clif_value)?;
        }
    }

    let entry_block = function
        .blocks
        .iter()
        .find(|block| block.id == entry)
        .ok_or_else(|| error(format!("entry block {} is absent", entry.0)))?;
    let ordered_blocks = std::iter::once(entry_block)
        .chain(function.blocks.iter().filter(|block| block.id != entry))
        .collect::<Vec<_>>();
    for block in ordered_blocks {
        builder.switch_to_block(state.block(block.id.0)?);
        if block.id == entry {
            let entry_values = builder.block_params(state.block(entry.0)?).to_vec();
            state.bind_entry_parameters(&mut builder, &entry_values)?;
        }
        for instruction in &block.instructions {
            let result = state.lower_instruction(&mut builder, instruction)?;
            match (instruction.result, result) {
                (Some(id), Some(value)) => state.set_value(id, value)?,
                (None, None) => {}
                (Some(id), None) => {
                    return Err(error(format!(
                        "instruction {} declares v{} but produces no value",
                        instruction.id.0, id.0
                    )));
                }
                (None, Some(_)) => {
                    return Err(error(format!(
                        "instruction {} produces an unbound value",
                        instruction.id.0
                    )));
                }
            }
        }
        let terminator = block.terminator.as_ref().ok_or_else(|| {
            error(format!(
                "block {} in `{}` has no terminator",
                block.id.0, function.symbol_name
            ))
        })?;
        state.lower_terminator(&mut builder, terminator)?;
    }
    builder.seal_all_blocks();
    builder.finalize();
    Ok(())
}

struct FunctionState<'a> {
    module: &'a gir::FullModule,
    function: &'a gir::FullFunction,
    config: &'a EffectiveCompilationConfig,
    abi_plan: ccc_abi::VerifiedModuleAbiPlan<'a>,
    definition_plan: DefinitionAbi<'a>,
    references: &'a FunctionReferences,
    blocks: HashMap<u32, ir::Block>,
    storage: HashMap<u32, StackSlot>,
    values: Vec<Option<ir::Value>>,
    sret: Option<ir::Value>,
    variadic_state: Option<ir::Value>,
    variadic_frame: Option<ir::Value>,
}

impl FunctionState<'_> {
    fn block(&self, raw: u32) -> Result<ir::Block, CodegenError> {
        block_ref(&self.blocks, raw)
    }

    fn value(&self, id: gir::ValueId) -> Result<ir::Value, CodegenError> {
        self.values
            .get(id.0 as usize)
            .copied()
            .flatten()
            .ok_or_else(|| error(format!("IR value v{} is unavailable during lowering", id.0)))
    }

    fn value_ty(&self, id: gir::ValueId) -> Result<QualifiedType, CodegenError> {
        value_type(self.function, id)
    }

    fn set_value(&mut self, id: gir::ValueId, value: ir::Value) -> Result<(), CodegenError> {
        let slot = self
            .values
            .get_mut(id.0 as usize)
            .ok_or_else(|| error(format!("IR value v{} has no value slot", id.0)))?;
        if slot.replace(value).is_some() {
            return Err(error(format!(
                "IR value v{} is defined more than once",
                id.0
            )));
        }
        Ok(())
    }

    fn bind_entry_parameters(
        &mut self,
        builder: &mut FunctionBuilder<'_>,
        clif_values: &[ir::Value],
    ) -> Result<(), CodegenError> {
        match self.definition_plan {
            DefinitionAbi::Native(plan) => {
                self.bind_native_entry_parameters(builder, clif_values, plan)
            }
            DefinitionAbi::Variadic(plan) => {
                self.bind_variadic_entry_parameters(builder, clif_values, plan)
            }
        }
    }

    fn bind_native_entry_parameters(
        &mut self,
        builder: &mut FunctionBuilder<'_>,
        clif_values: &[ir::Value],
        plan: &ccc_abi::NativeBoundaryPlan,
    ) -> Result<(), CodegenError> {
        if clif_values.len() != plan.clif_parameters.len() {
            return Err(error(format!(
                "entry ABI supplies {} carriers for a {}-carrier plan",
                clif_values.len(),
                plan.clif_parameters.len()
            )));
        }
        if let ccc_abi::NativeResultPlan::Indirect {
            sret_parameter_index,
            ..
        } = plan.result
        {
            self.sret = Some(
                *clif_values
                    .get(sret_parameter_index as usize)
                    .ok_or_else(|| {
                        error("indirect result pointer is absent from function entry")
                    })?,
            );
        }
        for parameter in plan.parameters.clone() {
            let source = parameter.source_index as usize;
            let incoming = self
                .function
                .parameters
                .get(source)
                .and_then(|parameter| parameter.incoming)
                .ok_or_else(|| {
                    error(format!(
                        "ABI parameter {} has no typed-IR incoming value",
                        parameter.source_index
                    ))
                })?;
            let value = if parameter.classified.passing == ccc_abi::PassingMode::Scalar {
                let index = *parameter
                    .carrier_indices
                    .first()
                    .ok_or_else(|| error("scalar ABI parameter has no carrier"))?;
                *clif_values
                    .get(index as usize)
                    .ok_or_else(|| error("scalar ABI carrier is absent from function entry"))?
            } else {
                let padded = align_up_u64(parameter.classified.size, 8)?;
                let address = create_stack_backing(builder, padded, parameter.classified.align)?;
                zero_memory(builder, address, padded)?;
                for index in &parameter.carrier_indices {
                    let carrier = plan
                        .clif_parameters
                        .get(*index as usize)
                        .ok_or_else(|| error("aggregate ABI carrier index is invalid"))?;
                    let incoming_value = *clif_values.get(*index as usize).ok_or_else(|| {
                        error("aggregate ABI carrier is absent from function entry")
                    })?;
                    match carrier.purpose {
                        ccc_abi::NativePurpose::StructArgument(_) => copy_memory(
                            builder,
                            address,
                            incoming_value,
                            parameter.classified.size,
                            gir::MemoryAccess::default(),
                            gir::MemoryAccess::default(),
                        )?,
                        ccc_abi::NativePurpose::Normal => {
                            let destination =
                                address_offset(builder, address, carrier.source_offset)?;
                            builder
                                .ins()
                                .store(MemFlags::new(), incoming_value, destination, 0);
                        }
                        ccc_abi::NativePurpose::StructReturn => {
                            return Err(error("source parameter unexpectedly uses sret purpose"));
                        }
                    }
                }
                address
            };
            self.set_value(incoming, value)?;
        }
        Ok(())
    }

    fn bind_variadic_entry_parameters(
        &mut self,
        builder: &mut FunctionBuilder<'_>,
        clif_values: &[ir::Value],
        plan: &ccc_abi::BridgeBoundaryPlan,
    ) -> Result<(), CodegenError> {
        let [frame] = clif_values else {
            return Err(error(format!(
                "hidden variadic body expects one frame pointer, received {} values",
                clif_values.len()
            )));
        };
        self.variadic_frame = Some(*frame);
        self.variadic_state = Some(address_offset(builder, *frame, 8)?);
        if plan.hidden_return {
            let saved_rdi = address_offset(builder, *frame, 32)?;
            self.sret = Some(
                builder
                    .ins()
                    .load(ir::types::I64, MemFlags::new(), saved_rdi, 0),
            );
        }
        if plan.parameters.len() != self.function.parameters.len() {
            return Err(error(
                "variadic fixed-prefix plan does not match function parameters",
            ));
        }
        for (source_index, classified) in plan.parameters.iter().enumerate() {
            let incoming = self.function.parameters[source_index]
                .incoming
                .ok_or_else(|| error("variadic fixed parameter has no typed-IR incoming value"))?;
            let pieces = plan
                .parameter_pieces
                .iter()
                .filter(|piece| piece.source_index == Some(source_index as u32))
                .collect::<Vec<_>>();
            let value = if classified.passing == ccc_abi::PassingMode::Scalar {
                let piece = pieces
                    .first()
                    .ok_or_else(|| error("variadic scalar parameter has no physical piece"))?;
                let source =
                    variadic_parameter_piece_address(builder, *frame, plan, piece.location)?;
                builder.ins().load(
                    scalar_type(
                        &self.module.types,
                        self.function.parameters[source_index].ty,
                        self.config,
                    )?,
                    MemFlags::new(),
                    source,
                    0,
                )
            } else {
                let padded = align_up_u64(classified.size, 8)?;
                let result = create_stack_backing(builder, padded, classified.align)?;
                zero_memory(builder, result, padded)?;
                for piece in pieces {
                    let source =
                        variadic_parameter_piece_address(builder, *frame, plan, piece.location)?;
                    let destination = address_offset(builder, result, piece.piece.offset)?;
                    copy_memory(
                        builder,
                        destination,
                        source,
                        u64::from(piece.piece.valid_bytes),
                        gir::MemoryAccess::default(),
                        gir::MemoryAccess::default(),
                    )?;
                }
                result
            };
            self.set_value(incoming, value)?;
        }
        Ok(())
    }

    fn result_ty(
        &self,
        instruction: &gir::FullInstruction,
    ) -> Result<Option<QualifiedType>, CodegenError> {
        instruction.result.map(|id| self.value_ty(id)).transpose()
    }

    fn lower_instruction(
        &self,
        builder: &mut FunctionBuilder<'_>,
        instruction: &gir::FullInstruction,
    ) -> Result<Option<ir::Value>, CodegenError> {
        let lowered = (|| {
            use gir::FullInstructionKind as I;
            let result_ty = self.result_ty(instruction)?;
            match &instruction.kind {
                I::Constant(constant) => Ok(Some(lower_constant(
                    builder,
                    &self.module.types,
                    result_ty.ok_or_else(|| error("constant instruction has no result"))?,
                    *constant,
                    self.config,
                )?)),
                I::AddressConstant {
                    target,
                    addend,
                    one_past: _,
                } => Ok(Some(self.address_constant(builder, *target, *addend)?)),
                I::AddressOfGlobal { global } => Ok(Some(self.global_address(builder, global.0)?)),
                I::AddressOfFunction {
                    function,
                    signature,
                } => {
                    if self.module.types.function_signature(*signature).is_none() {
                        return Err(error(format!(
                            "address of function {} carries a non-function type",
                            function.0
                        )));
                    }
                    let reference = self
                        .references
                        .functions
                        .get(&function.0)
                        .copied()
                        .ok_or_else(|| {
                            error(format!("reference to undeclared function {}", function.0))
                        })?;
                    Ok(Some(
                        builder.ins().func_addr(ir::types::I64, reference.address),
                    ))
                }
                I::AddressOfString { string } => {
                    let reference =
                        self.references
                            .strings
                            .get(&string.0)
                            .copied()
                            .ok_or_else(|| {
                                error(format!("reference to undeclared string {}", string.0))
                            })?;
                    Ok(Some(builder.ins().global_value(ir::types::I64, reference)))
                }
                I::AddressOfStorage { storage } => {
                    let slot = self.storage.get(&storage.0).copied().ok_or_else(|| {
                    error(format!(
                        "storage {} is not an automatic stack object; static storage must use a data id",
                        storage.0
                    ))
                })?;
                    Ok(Some(builder.ins().stack_addr(ir::types::I64, slot, 0)))
                }
                I::ProjectField {
                    base,
                    record,
                    field_index,
                    field_name,
                } => {
                    let layout = object_layout(&self.module.types, *record, self.config)?;
                    let LayoutShape::Record(record_layout) = layout.shape else {
                        return Err(error("field projection uses a non-record type"));
                    };
                    let field = record_layout.fields.get(*field_index).ok_or_else(|| {
                        error(format!("record projection references field {field_index}"))
                    })?;
                    if let Some(name) = field_name {
                        let TypeKind::Record(id) = self.module.types.kind(record.ty) else {
                            unreachable!()
                        };
                        let actual = self
                            .module
                            .types
                            .record(*id)
                            .and_then(|record| record.fields.as_ref())
                            .and_then(|fields| fields.get(*field_index))
                            .and_then(|field| field.name.as_deref());
                        if actual != Some(name.as_str()) {
                            return Err(error(format!(
                                "record projection field {field_index} does not match `{name}`"
                            )));
                        }
                    }
                    Ok(Some(address_offset(
                        builder,
                        self.value(*base)?,
                        field.offset,
                    )?))
                }
                I::PointerOffset {
                    base,
                    index,
                    element,
                    subtract,
                } => {
                    let size = object_layout(&self.module.types, *element, self.config)?.size;
                    let index_value = self.value(*index)?;
                    let index_ty = builder.func.dfg.value_type(index_value);
                    let index = coerce_integer(
                        builder,
                        index_value,
                        index_ty,
                        ir::types::I64,
                        is_signed(&self.module.types, self.value_ty(*index)?, self.config)?,
                    );
                    let size = builder.ins().iconst(
                        ir::types::I64,
                        i64::try_from(size).map_err(|_| {
                            error("pointer element size exceeds signed address range")
                        })?,
                    );
                    let displacement = builder.ins().imul(index, size);
                    let base = self.value(*base)?;
                    Ok(Some(if *subtract {
                        builder.ins().isub(base, displacement)
                    } else {
                        builder.ins().iadd(base, displacement)
                    }))
                }
                I::PointerDifference {
                    left,
                    right,
                    element,
                } => {
                    let size = object_layout(&self.module.types, *element, self.config)?.size;
                    if size == 0 {
                        return Err(error("pointer difference uses a zero-sized element"));
                    }
                    let bytes = builder.ins().isub(self.value(*left)?, self.value(*right)?);
                    let size = builder.ins().iconst(
                        ir::types::I64,
                        i64::try_from(size).map_err(|_| {
                            error("pointer element size exceeds signed address range")
                        })?,
                    );
                    let difference = builder.ins().sdiv(bytes, size);
                    let destination = scalar_type(
                        &self.module.types,
                        result_ty.ok_or_else(|| error("pointer difference has no result"))?,
                        self.config,
                    )?;
                    Ok(Some(coerce_integer(
                        builder,
                        difference,
                        ir::types::I64,
                        destination,
                        true,
                    )))
                }
                I::Load {
                    address,
                    object,
                    access,
                } => Ok(Some(lower_load(
                    builder,
                    self.value(*address)?,
                    scalar_type(&self.module.types, *object, self.config)?,
                    *access,
                )?)),
                I::Store {
                    address,
                    value,
                    object,
                    access,
                } => {
                    let ty = scalar_type(&self.module.types, *object, self.config)?;
                    let value_ty = self.value_ty(*value)?;
                    let signed = if is_float(&self.module.types, value_ty) {
                        false
                    } else {
                        is_signed(&self.module.types, value_ty, self.config)?
                    };
                    let value = coerce_value(builder, self.value(*value)?, ty, signed)?;
                    lower_store(builder, self.value(*address)?, value, *access)?;
                    Ok(None)
                }
                I::BitfieldLoad {
                    address,
                    descriptor,
                    access,
                } => Ok(Some(self.bitfield_load(
                    builder,
                    self.value(*address)?,
                    *descriptor,
                    *access,
                    result_ty.ok_or_else(|| error("bitfield load has no result"))?,
                )?)),
                I::BitfieldStore {
                    address,
                    value,
                    descriptor,
                    access,
                } => {
                    self.bitfield_store(
                        builder,
                        self.value(*address)?,
                        self.value(*value)?,
                        *descriptor,
                        *access,
                    )?;
                    Ok(None)
                }
                I::ZeroInitialize {
                    destination,
                    object,
                } => {
                    let size = object_layout(&self.module.types, *object, self.config)?.size;
                    zero_memory(builder, self.value(*destination)?, size)?;
                    Ok(None)
                }
                I::StringInitialize {
                    destination,
                    string,
                    object,
                    copy_code_units,
                } => {
                    let destination_size =
                        object_layout(&self.module.types, *object, self.config)?.size;
                    let source =
                        self.references
                            .strings
                            .get(&string.0)
                            .copied()
                            .ok_or_else(|| {
                                error(format!("reference to undeclared string {}", string.0))
                            })?;
                    let source = builder.ins().global_value(ir::types::I64, source);
                    let string = self
                        .module
                        .strings
                        .iter()
                        .find(|candidate| candidate.id == *string)
                        .ok_or_else(|| error(format!("unknown string {}", string.0)))?;
                    if *copy_code_units > string.code_units.len() as u64 {
                        return Err(error(format!(
                            "string initialization requests {} unavailable code units",
                            copy_code_units
                        )));
                    }
                    let copy_size = copy_code_units
                        .checked_mul(string_unit_bytes(string.encoding) as u64)
                        .ok_or_else(|| error("string initialization byte count overflow"))?;
                    if copy_size > destination_size {
                        return Err(error(format!(
                            "string initialization needs {copy_size} bytes for a {destination_size}-byte object"
                        )));
                    }
                    zero_memory(builder, self.value(*destination)?, destination_size)?;
                    copy_memory(
                        builder,
                        self.value(*destination)?,
                        source,
                        copy_size,
                        gir::MemoryAccess::default(),
                        gir::MemoryAccess::default(),
                    )?;
                    Ok(None)
                }
                I::AggregateCopy {
                    destination,
                    source,
                    destination_object,
                    source_object: _,
                    destination_access,
                    source_access,
                    overlap: gir::AggregateOverlap::MayOverlap,
                } => {
                    let size =
                        object_layout(&self.module.types, *destination_object, self.config)?.size;
                    copy_memory(
                        builder,
                        self.value(*destination)?,
                        self.value(*source)?,
                        size,
                        *destination_access,
                        *source_access,
                    )?;
                    Ok(None)
                }
                I::AggregateSnapshot {
                    source,
                    object,
                    access,
                } => {
                    validate_access(*access)?;
                    let layout = object_layout(&self.module.types, *object, self.config)?;
                    let snapshot = create_stack_backing(builder, layout.size, layout.align)?;
                    copy_memory(
                        builder,
                        snapshot,
                        self.value(*source)?,
                        layout.size,
                        gir::MemoryAccess::default(),
                        *access,
                    )?;
                    Ok(Some(snapshot))
                }
                I::AggregateProject {
                    base,
                    aggregate,
                    projections,
                } => Ok(Some(self.aggregate_project(
                    builder,
                    self.value(*base)?,
                    *aggregate,
                    projections,
                )?)),
                I::Convert {
                    kind,
                    operand,
                    from,
                    to,
                } => lower_conversion(
                    builder,
                    &self.module.types,
                    self.value(*operand)?,
                    *kind,
                    *from,
                    *to,
                    self.config,
                ),
                I::Unary { operator, operand } => Ok(Some(lower_unary(
                    builder,
                    &self.module.types,
                    *operator,
                    self.value(*operand)?,
                    self.value_ty(*operand)?,
                    result_ty.ok_or_else(|| error("unary instruction has no result"))?,
                    self.config,
                )?)),
                I::Binary {
                    operator,
                    left,
                    right,
                } => {
                    let operand_ty = self.value_ty(*left)?;
                    let floating = is_float(&self.module.types, operand_ty);
                    let signed = if floating {
                        false
                    } else {
                        is_signed(&self.module.types, operand_ty, self.config)?
                    };
                    let result = scalar_type(
                        &self.module.types,
                        result_ty.ok_or_else(|| error("binary instruction has no result"))?,
                        self.config,
                    )?;
                    Ok(Some(lower_binary(
                        builder,
                        *operator,
                        self.value(*left)?,
                        self.value(*right)?,
                        floating,
                        signed,
                        result,
                    )?))
                }
                I::DirectCall {
                    function,
                    signature,
                    arguments,
                    variadic_boundary,
                    effects: _,
                } => self.direct_call(
                    builder,
                    instruction.id,
                    function.0,
                    *signature,
                    arguments,
                    *variadic_boundary,
                ),
                I::IndirectCall {
                    callee,
                    signature,
                    arguments,
                    variadic_boundary,
                    effects: _,
                } => self.indirect_call(
                    builder,
                    instruction.id,
                    self.value(*callee)?,
                    *signature,
                    arguments,
                    *variadic_boundary,
                ),
                I::MemoryFence {
                    order: gir::MemoryOrder::SequentiallyConsistent,
                } => {
                    builder.ins().fence();
                    Ok(None)
                }
                I::VaStart {
                    list,
                    last_named_parameter: _,
                } => {
                    self.va_start(builder, self.value(*list)?)?;
                    Ok(None)
                }
                I::VaArg { list, requested } => Ok(Some(self.va_arg(
                    builder,
                    instruction.id,
                    self.value(*list)?,
                    *requested,
                )?)),
                I::VaCopy {
                    destination,
                    source,
                } => {
                    copy_memory(
                        builder,
                        self.value(*destination)?,
                        self.value(*source)?,
                        24,
                        gir::MemoryAccess::default(),
                        gir::MemoryAccess::default(),
                    )?;
                    Ok(None)
                }
                I::VaEnd { list: _ } => Ok(None),
            }
        })();
        lowered.map_err(|error| error.with_span_if_none(instruction.span))
    }

    fn global_address(
        &self,
        builder: &mut FunctionBuilder<'_>,
        raw: u32,
    ) -> Result<ir::Value, CodegenError> {
        let reference = self
            .references
            .globals
            .get(&raw)
            .copied()
            .ok_or_else(|| error(format!("reference to undeclared data object {raw}")))?;
        Ok(if reference.tls {
            builder.ins().tls_value(ir::types::I64, reference.value)
        } else {
            builder.ins().global_value(ir::types::I64, reference.value)
        })
    }

    fn address_constant(
        &self,
        builder: &mut FunctionBuilder<'_>,
        target: gir::RelocationTarget,
        addend: i128,
    ) -> Result<ir::Value, CodegenError> {
        let address = match target {
            gir::RelocationTarget::Object(id) => self.global_address(builder, id.0)?,
            gir::RelocationTarget::Function(id) => {
                let reference = self
                    .references
                    .functions
                    .get(&id.0)
                    .copied()
                    .ok_or_else(|| error(format!("reference to undeclared function {}", id.0)))?;
                builder.ins().func_addr(ir::types::I64, reference.address)
            }
            gir::RelocationTarget::String(id) => {
                let reference = self
                    .references
                    .strings
                    .get(&id.0)
                    .copied()
                    .ok_or_else(|| error(format!("reference to undeclared string {}", id.0)))?;
                builder.ins().global_value(ir::types::I64, reference)
            }
        };
        let addend = i64::try_from(addend)
            .map_err(|_| error("address constant addend does not fit in 64 bits"))?;
        Ok(if addend == 0 {
            address
        } else {
            builder.ins().iadd_imm(address, addend)
        })
    }

    fn aggregate_project(
        &self,
        builder: &mut FunctionBuilder<'_>,
        mut address: ir::Value,
        aggregate: QualifiedType,
        projections: &[gir::AggregateProjection],
    ) -> Result<ir::Value, CodegenError> {
        let mut current = aggregate;
        for (position, projection) in projections.iter().enumerate() {
            match projection {
                gir::AggregateProjection::Field {
                    index,
                    name,
                    bitfield,
                } => {
                    let layout = object_layout(&self.module.types, current, self.config)?;
                    let LayoutShape::Record(record_layout) = layout.shape else {
                        return Err(error(
                            "aggregate field projection reached a non-record type",
                        ));
                    };
                    let field_layout = record_layout.fields.get(*index).ok_or_else(|| {
                        error(format!("aggregate projection references field {index}"))
                    })?;
                    let TypeKind::Record(record_id) = self.module.types.kind(current.ty) else {
                        unreachable!()
                    };
                    let field = self
                        .module
                        .types
                        .record(*record_id)
                        .and_then(|record| record.fields.as_ref())
                        .and_then(|fields| fields.get(*index))
                        .ok_or_else(|| error("aggregate projection field metadata is absent"))?;
                    if let Some(expected) = name
                        && field.name.as_deref() != Some(expected.as_str())
                    {
                        return Err(error(format!(
                            "aggregate projection field {index} does not match `{expected}`"
                        )));
                    }
                    match (field_layout.bitfield, bitfield) {
                        (Some(layout), Some(descriptor)) => {
                            if position + 1 != projections.len() {
                                return Err(error(
                                    "bitfield must be the final aggregate projection",
                                ));
                            }
                            let storage_offset = layout
                                .storage_offset
                                .checked_sub(field_layout.offset)
                                .ok_or_else(|| error("bitfield storage precedes its field"))?;
                            if descriptor.field_index != *index
                                || descriptor.storage_offset != storage_offset
                                || descriptor.storage_size != layout.storage_size
                                || descriptor.storage_align != layout.storage_align
                                || descriptor.bit_offset != layout.bit_offset
                                || descriptor.width != layout.width
                                || descriptor.signed
                                    != is_signed(&self.module.types, field.ty, self.config)?
                            {
                                return Err(error(
                                    "aggregate bitfield projection descriptor disagrees with layout",
                                ));
                            }
                        }
                        (Some(_), None) => {
                            return Err(error(
                                "aggregate projection cannot expose a bitfield address",
                            ));
                        }
                        (None, Some(_)) => {
                            return Err(error(
                                "aggregate projection marks a non-bitfield field as a bitfield",
                            ));
                        }
                        (None, None) => {}
                    }
                    address = address_offset(builder, address, field_layout.offset)?;
                    current = field.ty;
                }
                gir::AggregateProjection::Index { index } => {
                    let TypeKind::Array(array) = self.module.types.kind(current.ty) else {
                        return Err(error("aggregate index projection reached a non-array type"));
                    };
                    let layout = object_layout(&self.module.types, current, self.config)?;
                    let LayoutShape::Array { stride, .. } = layout.shape else {
                        unreachable!()
                    };
                    let index_value = self.value(*index)?;
                    let index_type = builder.func.dfg.value_type(index_value);
                    let index_value = coerce_integer(
                        builder,
                        index_value,
                        index_type,
                        ir::types::I64,
                        is_signed(&self.module.types, self.value_ty(*index)?, self.config)?,
                    );
                    let scaled = if stride == 1 {
                        index_value
                    } else {
                        builder.ins().imul_imm(
                            index_value,
                            i64::try_from(stride)
                                .map_err(|_| error("aggregate array stride is too large"))?,
                        )
                    };
                    address = builder.ins().iadd(address, scaled);
                    current = array.element;
                }
            }
        }
        Ok(address)
    }

    fn va_start(
        &self,
        builder: &mut FunctionBuilder<'_>,
        destination: ir::Value,
    ) -> Result<(), CodegenError> {
        let initial = self.variadic_state.ok_or_else(|| {
            error("`va_start` is unavailable outside a generated variadic function body")
        })?;
        copy_memory(
            builder,
            destination,
            initial,
            24,
            gir::MemoryAccess::default(),
            gir::MemoryAccess::default(),
        )
    }

    fn va_arg(
        &self,
        builder: &mut FunctionBuilder<'_>,
        instruction: gir::InstructionId,
        list: ir::Value,
        requested: QualifiedType,
    ) -> Result<ir::Value, CodegenError> {
        let plan = self
            .abi_plan
            .plan()
            .va_args
            .get(&(self.function.id, instruction))
            .ok_or_else(|| {
                error(format!(
                    "va_arg instruction {} has no ABI plan",
                    instruction.0
                ))
            })?;
        if plan.classified.ty != requested.ty {
            return Err(error(
                "va_arg plan type does not match the typed instruction",
            ));
        }
        let result = create_stack_backing(builder, plan.result_size, plan.result_align)?;
        zero_memory(builder, result, plan.result_size)?;
        if plan.classified.passing == ccc_abi::PassingMode::Memory {
            self.va_arg_overflow(builder, list, result, plan)?;
            return va_arg_result(builder, &self.module.types, requested, result, self.config);
        }

        let gp_offset_address = list;
        let fp_offset_address = address_offset(builder, list, 4)?;
        let gp_offset = builder
            .ins()
            .load(ir::types::I32, MemFlags::new(), gp_offset_address, 0);
        let fp_offset = builder
            .ins()
            .load(ir::types::I32, MemFlags::new(), fp_offset_address, 0);
        let gp_limit = 48u32
            .checked_sub(u32::from(plan.gp_slots) * 8)
            .ok_or_else(|| error("va_arg GP slot requirement exceeds the save area"))?;
        let fp_limit = 176u32
            .checked_sub(u32::from(plan.sse_slots) * 16)
            .ok_or_else(|| error("va_arg SSE slot requirement exceeds the save area"))?;
        let gp_available = if plan.gp_slots == 0 {
            builder.ins().iconst(ir::types::I8, 1)
        } else {
            builder.ins().icmp_imm(
                IntCC::UnsignedLessThanOrEqual,
                gp_offset,
                i64::from(gp_limit),
            )
        };
        let fp_available = if plan.sse_slots == 0 {
            builder.ins().iconst(ir::types::I8, 1)
        } else {
            builder.ins().icmp_imm(
                IntCC::UnsignedLessThanOrEqual,
                fp_offset,
                i64::from(fp_limit),
            )
        };
        let registers_available = builder.ins().band(gp_available, fp_available);
        let register_block = builder.create_block();
        let overflow_block = builder.create_block();
        let merge_block = builder.create_block();
        builder.ins().brif(
            registers_available,
            register_block,
            &[],
            overflow_block,
            &[],
        );

        builder.switch_to_block(register_block);
        let save_area_address = address_offset(builder, list, 16)?;
        let save_area = builder
            .ins()
            .load(ir::types::I64, MemFlags::new(), save_area_address, 0);
        let mut next_gp = gp_offset;
        let mut next_fp = fp_offset;
        for piece in &plan.classified.pieces {
            let source = match piece.class {
                ccc_abi::AbiClass::Integer => {
                    let offset = builder.ins().uextend(ir::types::I64, next_gp);
                    next_gp = builder.ins().iadd_imm(next_gp, 8);
                    builder.ins().iadd(save_area, offset)
                }
                ccc_abi::AbiClass::Sse | ccc_abi::AbiClass::SseUp => {
                    let offset = builder.ins().uextend(ir::types::I64, next_fp);
                    next_fp = builder.ins().iadd_imm(next_fp, 16);
                    builder.ins().iadd(save_area, offset)
                }
                class => {
                    return Err(error(format!(
                        "va_arg register path contains unsupported class {class:?}"
                    )));
                }
            };
            let destination = address_offset(builder, result, piece.offset)?;
            copy_memory(
                builder,
                destination,
                source,
                u64::from(piece.valid_bytes),
                gir::MemoryAccess::default(),
                gir::MemoryAccess::default(),
            )?;
        }
        // Neither cursor becomes observable until all required register files
        // have been read successfully.
        if plan.gp_slots != 0 {
            builder
                .ins()
                .store(MemFlags::new(), next_gp, gp_offset_address, 0);
        }
        if plan.sse_slots != 0 {
            builder
                .ins()
                .store(MemFlags::new(), next_fp, fp_offset_address, 0);
        }
        builder.ins().jump(merge_block, &[]);

        builder.switch_to_block(overflow_block);
        self.va_arg_overflow(builder, list, result, plan)?;
        builder.ins().jump(merge_block, &[]);

        builder.switch_to_block(merge_block);
        va_arg_result(builder, &self.module.types, requested, result, self.config)
    }

    fn va_arg_overflow(
        &self,
        builder: &mut FunctionBuilder<'_>,
        list: ir::Value,
        result: ir::Value,
        plan: &ccc_abi::VaArgPlan,
    ) -> Result<(), CodegenError> {
        let overflow_address = address_offset(builder, list, 8)?;
        let overflow = builder
            .ins()
            .load(ir::types::I64, MemFlags::new(), overflow_address, 0);
        let aligned = if plan.overflow_align <= 1 {
            overflow
        } else {
            let added = builder.ins().iadd_imm(
                overflow,
                i64::try_from(plan.overflow_align - 1)
                    .map_err(|_| error("va_arg overflow alignment is too large"))?,
            );
            builder.ins().band_imm(
                added,
                -i64::try_from(plan.overflow_align)
                    .map_err(|_| error("va_arg overflow alignment is too large"))?,
            )
        };
        copy_memory(
            builder,
            result,
            aligned,
            plan.result_size,
            gir::MemoryAccess::default(),
            gir::MemoryAccess::default(),
        )?;
        let next = builder.ins().iadd_imm(
            aligned,
            i64::try_from(plan.overflow_size)
                .map_err(|_| error("va_arg overflow size is too large"))?,
        );
        builder
            .ins()
            .store(MemFlags::new(), next, overflow_address, 0);
        Ok(())
    }

    fn bitfield_load(
        &self,
        builder: &mut FunctionBuilder<'_>,
        address: ir::Value,
        descriptor: gir::BitfieldDescriptor,
        access: gir::MemoryAccess,
        result: QualifiedType,
    ) -> Result<ir::Value, CodegenError> {
        validate_bitfield(descriptor)?;
        validate_access(access)?;
        let storage_ty = integer_type_for_size(descriptor.storage_size, "bitfield storage")?;
        let address = address_offset(builder, address, descriptor.storage_offset)?;
        if access.volatile {
            builder.ins().fence();
        }
        let unit = builder.ins().load(storage_ty, MemFlags::new(), address, 0);
        let shifted = if descriptor.bit_offset == 0 {
            unit
        } else {
            builder
                .ins()
                .ushr_imm(unit, i64::from(descriptor.bit_offset))
        };
        let masked = builder
            .ins()
            .band_imm(shifted, low_mask(descriptor.width) as i64);
        let normalized = if descriptor.signed && descriptor.width != 0 {
            let shift = storage_ty.bits() - descriptor.width;
            if shift == 0 {
                masked
            } else {
                let shifted = builder.ins().ishl_imm(masked, i64::from(shift));
                builder.ins().sshr_imm(shifted, i64::from(shift))
            }
        } else {
            masked
        };
        if access.volatile {
            builder.ins().fence();
        }
        let result_ty = scalar_type(&self.module.types, result, self.config)?;
        Ok(coerce_integer(
            builder,
            normalized,
            storage_ty,
            result_ty,
            descriptor.signed,
        ))
    }

    fn bitfield_store(
        &self,
        builder: &mut FunctionBuilder<'_>,
        address: ir::Value,
        value: ir::Value,
        descriptor: gir::BitfieldDescriptor,
        access: gir::MemoryAccess,
    ) -> Result<(), CodegenError> {
        validate_bitfield(descriptor)?;
        validate_access(access)?;
        let storage_ty = integer_type_for_size(descriptor.storage_size, "bitfield storage")?;
        let address = address_offset(builder, address, descriptor.storage_offset)?;
        if access.volatile {
            builder.ins().fence();
        }
        let old = builder.ins().load(storage_ty, MemFlags::new(), address, 0);
        let value_ty = builder.func.dfg.value_type(value);
        let value = coerce_integer(builder, value, value_ty, storage_ty, descriptor.signed);
        let value_mask = low_mask(descriptor.width);
        let field_mask = value_mask.checked_shl(descriptor.bit_offset).unwrap_or(0);
        let retained = builder.ins().band_imm(old, (!field_mask) as i64);
        let value = builder.ins().band_imm(value, value_mask as i64);
        let value = if descriptor.bit_offset == 0 {
            value
        } else {
            builder
                .ins()
                .ishl_imm(value, i64::from(descriptor.bit_offset))
        };
        let combined = builder.ins().bor(retained, value);
        builder.ins().store(MemFlags::new(), combined, address, 0);
        if access.volatile {
            builder.ins().fence();
        }
        Ok(())
    }

    fn direct_call(
        &self,
        builder: &mut FunctionBuilder<'_>,
        instruction: gir::InstructionId,
        function: u32,
        signature: TypeId,
        arguments: &[gir::ValueId],
        variadic_boundary: usize,
    ) -> Result<Option<ir::Value>, CodegenError> {
        let boundary = self.call_boundary(instruction, signature, arguments, variadic_boundary)?;
        let reference = self
            .references
            .functions
            .get(&function)
            .copied()
            .ok_or_else(|| error(format!("call references undeclared function {function}")))?;
        match boundary {
            ccc_abi::BoundaryPlan::Native(plan) => {
                let (arguments, result_storage) =
                    self.marshal_native_call_arguments(builder, plan, arguments)?;
                let call = builder.ins().call(reference.direct_call, &arguments);
                self.finish_native_call(builder, call, plan, result_storage)
            }
            ccc_abi::BoundaryPlan::Bridge(plan) => {
                let target = builder.ins().func_addr(ir::types::I64, reference.address);
                self.bridge_call(builder, target, plan, arguments)
            }
        }
    }

    fn indirect_call(
        &self,
        builder: &mut FunctionBuilder<'_>,
        instruction: gir::InstructionId,
        callee: ir::Value,
        signature: TypeId,
        arguments: &[gir::ValueId],
        variadic_boundary: usize,
    ) -> Result<Option<ir::Value>, CodegenError> {
        let boundary = self.call_boundary(instruction, signature, arguments, variadic_boundary)?;
        match boundary {
            ccc_abi::BoundaryPlan::Native(plan) => {
                let signature = crate::signature(plan).map_err(error)?;
                let signature = builder.func.import_signature(signature);
                let (arguments, result_storage) =
                    self.marshal_native_call_arguments(builder, plan, arguments)?;
                let call = builder.ins().call_indirect(signature, callee, &arguments);
                self.finish_native_call(builder, call, plan, result_storage)
            }
            ccc_abi::BoundaryPlan::Bridge(plan) => {
                self.bridge_call(builder, callee, plan, arguments)
            }
        }
    }

    fn call_boundary(
        &self,
        instruction: gir::InstructionId,
        signature: TypeId,
        arguments: &[gir::ValueId],
        variadic_boundary: usize,
    ) -> Result<&ccc_abi::BoundaryPlan, CodegenError> {
        let boundary = self
            .abi_plan
            .plan()
            .calls
            .get(&(self.function.id, instruction))
            .ok_or_else(|| {
                error(format!(
                    "call instruction {} has no ABI plan",
                    instruction.0
                ))
            })?;
        let boundary = &boundary.boundary;
        let planned_signature =
            self.module
                .types
                .function_signature(signature)
                .ok_or_else(|| {
                    error(format!(
                        "call instruction {} has a non-function signature",
                        instruction.0
                    ))
                })?;
        let parameter_count = match boundary {
            ccc_abi::BoundaryPlan::Native(plan) => plan.parameters.len(),
            ccc_abi::BoundaryPlan::Bridge(plan) => plan.parameters.len(),
        };
        let boundary_matches = match &planned_signature.parameters {
            ccc_types::FunctionParameters::Unspecified => variadic_boundary == 0,
            ccc_types::FunctionParameters::Prototype(_) if planned_signature.variadic => {
                variadic_boundary <= arguments.len()
            }
            ccc_types::FunctionParameters::Prototype(_) => variadic_boundary == arguments.len(),
        };
        if parameter_count != arguments.len() || !boundary_matches {
            return Err(error(format!(
                "call signature expects {} fixed arguments but IR carries {} arguments with boundary {variadic_boundary}",
                parameter_count,
                arguments.len()
            )));
        }
        Ok(boundary)
    }

    fn bridge_call(
        &self,
        builder: &mut FunctionBuilder<'_>,
        target: ir::Value,
        plan: &ccc_abi::BridgeBoundaryPlan,
        arguments: &[gir::ValueId],
    ) -> Result<Option<ir::Value>, CodegenError> {
        let helper = self
            .references
            .call_helper
            .ok_or_else(|| error("variadic call bridge has no translation-unit helper"))?;
        let frame_size = 256u64
            .checked_add(u64::from(plan.stack_size))
            .ok_or_else(|| error("variadic call frame size overflow"))?;
        let frame = create_stack_backing(builder, frame_size, 16)?;
        zero_memory(builder, frame, frame_size)?;
        store_integer(builder, frame, 0, ir::types::I32, 0x4642_4343)?;
        store_integer(builder, frame, 4, ir::types::I16, 1)?;
        store_integer(builder, frame, 6, ir::types::I16, 32)?;
        store_value(builder, frame, 8, target)?;
        store_integer(
            builder,
            frame,
            16,
            ir::types::I32,
            i64::from(plan.stack_size),
        )?;
        store_integer(
            builder,
            frame,
            20,
            ir::types::I32,
            i64::try_from(frame_size).map_err(|_| error("bridge frame is too large"))?,
        )?;
        store_integer(builder, frame, 24, ir::types::I8, i64::from(plan.gp_used))?;
        store_integer(builder, frame, 25, ir::types::I8, i64::from(plan.xmm_used))?;
        store_integer(
            builder,
            frame,
            26,
            ir::types::I8,
            i64::from(plan.variadic_sse_count),
        )?;
        let gp_results = plan
            .result_pieces
            .iter()
            .filter(|piece| piece.piece.class == ccc_abi::AbiClass::Integer)
            .count();
        let xmm_results = plan.result_pieces.len().saturating_sub(gp_results);
        store_integer(builder, frame, 27, ir::types::I8, gp_results as i64)?;
        store_integer(builder, frame, 28, ir::types::I8, xmm_results as i64)?;

        let result_storage = if plan.hidden_return {
            let result = create_stack_backing(builder, plan.result.size, plan.result.align)?;
            zero_memory(builder, result, plan.result.size)?;
            store_value(builder, frame, 32, result)?;
            Some(result)
        } else {
            None
        };
        for piece in &plan.parameter_pieces {
            let source_index = piece
                .source_index
                .ok_or_else(|| error("variadic argument bridge piece has no source index"))?;
            let argument_id = *arguments
                .get(source_index as usize)
                .ok_or_else(|| error("variadic bridge source argument is absent"))?;
            let classified = plan
                .parameters
                .get(source_index as usize)
                .ok_or_else(|| error("variadic bridge parameter plan is absent"))?;
            let destination = bridge_argument_piece_address(builder, frame, piece.location)?;
            if classified.passing == ccc_abi::PassingMode::Scalar {
                let mut value = self.value(argument_id)?;
                let value_type = builder.func.dfg.value_type(value);
                if value_type.is_int() && value_type.bits() < 32 {
                    value = coerce_integer(
                        builder,
                        value,
                        value_type,
                        ir::types::I32,
                        match piece.extension {
                            ccc_abi::IntegerExtension::Signed => true,
                            ccc_abi::IntegerExtension::Unsigned => false,
                            ccc_abi::IntegerExtension::None => {
                                return Err(error(
                                    "narrow bridge integer has no planned extension",
                                ));
                            }
                        },
                    );
                }
                store_value(builder, destination, 0, value)?;
            } else {
                let source = address_offset(builder, self.value(argument_id)?, piece.piece.offset)?;
                copy_memory(
                    builder,
                    destination,
                    source,
                    u64::from(piece.piece.valid_bytes),
                    gir::MemoryAccess::default(),
                    gir::MemoryAccess::default(),
                )?;
            }
        }
        let call = builder.ins().call(helper, &[frame]);
        if !builder.inst_results(call).is_empty() {
            return Err(error(
                "generic variadic call helper unexpectedly returns a CLIF value",
            ));
        }
        match plan.result.passing {
            ccc_abi::PassingMode::Void => Ok(None),
            ccc_abi::PassingMode::Scalar => {
                let piece = plan
                    .result_pieces
                    .first()
                    .ok_or_else(|| error("variadic scalar result has no bridge piece"))?;
                let source = bridge_result_piece_address(builder, frame, piece.location)?;
                Ok(Some(builder.ins().load(
                    scalar_type(
                        &self.module.types,
                        QualifiedType::unqualified(plan.result.ty),
                        self.config,
                    )?,
                    MemFlags::new(),
                    source,
                    0,
                )))
            }
            ccc_abi::PassingMode::Registers => {
                let result = create_stack_backing(builder, plan.result.size, plan.result.align)?;
                zero_memory(builder, result, plan.result.size)?;
                for piece in &plan.result_pieces {
                    let source = bridge_result_piece_address(builder, frame, piece.location)?;
                    let destination = address_offset(builder, result, piece.piece.offset)?;
                    copy_memory(
                        builder,
                        destination,
                        source,
                        u64::from(piece.piece.valid_bytes),
                        gir::MemoryAccess::default(),
                        gir::MemoryAccess::default(),
                    )?;
                }
                Ok(Some(result))
            }
            ccc_abi::PassingMode::Memory => {
                Ok(Some(result_storage.ok_or_else(|| {
                    error("indirect variadic result has no owned backing storage")
                })?))
            }
        }
    }

    fn marshal_native_call_arguments(
        &self,
        builder: &mut FunctionBuilder<'_>,
        plan: &ccc_abi::NativeBoundaryPlan,
        arguments: &[gir::ValueId],
    ) -> Result<(Vec<ir::Value>, Option<ir::Value>), CodegenError> {
        let mut lowered = Vec::with_capacity(plan.clif_parameters.len());
        let mut staged = HashMap::<u32, ir::Value>::new();
        let mut result_storage = None;
        for carrier in &plan.clif_parameters {
            if carrier.purpose == ccc_abi::NativePurpose::StructReturn {
                let ccc_abi::NativeResultPlan::Indirect { classified, .. } = &plan.result else {
                    return Err(error("sret carrier has no indirect result plan"));
                };
                let address = create_stack_backing(builder, classified.size, classified.align)?;
                zero_memory(builder, address, classified.size)?;
                result_storage = Some(address);
                lowered.push(address);
                continue;
            }
            let source_index = carrier
                .source_index
                .ok_or_else(|| error("call carrier has no source argument"))?;
            let source_value = *arguments
                .get(source_index as usize)
                .ok_or_else(|| error("call carrier source argument is absent"))?;
            let parameter = plan
                .parameters
                .get(source_index as usize)
                .ok_or_else(|| error("call parameter plan is absent"))?;
            if parameter.classified.passing == ccc_abi::PassingMode::Scalar {
                lowered.push(self.value(source_value)?);
                continue;
            }
            let stage = if let Some(stage) = staged.get(&source_index) {
                *stage
            } else {
                let padded = align_up_u64(parameter.classified.size, 8)?;
                let stage = create_stack_backing(builder, padded, parameter.classified.align)?;
                zero_memory(builder, stage, padded)?;
                copy_memory(
                    builder,
                    stage,
                    self.value(source_value)?,
                    parameter.classified.size,
                    gir::MemoryAccess::default(),
                    gir::MemoryAccess::default(),
                )?;
                staged.insert(source_index, stage);
                stage
            };
            match carrier.purpose {
                ccc_abi::NativePurpose::StructArgument(_) => lowered.push(stage),
                ccc_abi::NativePurpose::Normal => {
                    let address = address_offset(builder, stage, carrier.source_offset)?;
                    lowered.push(builder.ins().load(
                        native_carrier_type(carrier.carrier),
                        MemFlags::new(),
                        address,
                        0,
                    ));
                }
                ccc_abi::NativePurpose::StructReturn => unreachable!(),
            }
        }
        Ok((lowered, result_storage))
    }

    fn finish_native_call(
        &self,
        builder: &mut FunctionBuilder<'_>,
        call: ir::Inst,
        plan: &ccc_abi::NativeBoundaryPlan,
        result_storage: Option<ir::Value>,
    ) -> Result<Option<ir::Value>, CodegenError> {
        match &plan.result {
            ccc_abi::NativeResultPlan::Void => call_result(builder, call, false),
            ccc_abi::NativeResultPlan::Scalar { .. } => call_result(builder, call, true),
            ccc_abi::NativeResultPlan::Indirect { .. } => {
                if !builder.inst_results(call).is_empty() {
                    return Err(error(
                        "indirect aggregate call unexpectedly produced CLIF results",
                    ));
                }
                Ok(Some(result_storage.ok_or_else(|| {
                    error("indirect aggregate call has no owned result storage")
                })?))
            }
            ccc_abi::NativeResultPlan::RegisterAggregate { classified, .. } => {
                let results = builder.inst_results(call).to_vec();
                if results.len() != plan.clif_results.len() {
                    return Err(error(
                        "register aggregate call result carrier count differs from its plan",
                    ));
                }
                let padded = align_up_u64(classified.size, 8)?;
                let address = create_stack_backing(builder, padded, classified.align)?;
                zero_memory(builder, address, padded)?;
                for (carrier, value) in plan.clif_results.iter().zip(results) {
                    let destination = address_offset(builder, address, carrier.source_offset)?;
                    builder.ins().store(MemFlags::new(), value, destination, 0);
                }
                Ok(Some(address))
            }
        }
    }

    fn lower_return(
        &self,
        builder: &mut FunctionBuilder<'_>,
        value: Option<gir::ValueId>,
    ) -> Result<(), CodegenError> {
        match self.definition_plan {
            DefinitionAbi::Native(plan) => self.lower_native_return(builder, value, plan),
            DefinitionAbi::Variadic(plan) => self.lower_variadic_return(builder, value, plan),
        }
    }

    fn lower_native_return(
        &self,
        builder: &mut FunctionBuilder<'_>,
        value: Option<gir::ValueId>,
        plan: &ccc_abi::NativeBoundaryPlan,
    ) -> Result<(), CodegenError> {
        match (&plan.result, value) {
            (ccc_abi::NativeResultPlan::Void, None) => {
                builder.ins().return_(&[]);
            }
            (ccc_abi::NativeResultPlan::Scalar { .. }, Some(value)) => {
                builder.ins().return_(&[self.value(value)?]);
            }
            (ccc_abi::NativeResultPlan::RegisterAggregate { classified, .. }, Some(value)) => {
                let padded = align_up_u64(classified.size, 8)?;
                let stage = create_stack_backing(builder, padded, classified.align)?;
                zero_memory(builder, stage, padded)?;
                copy_memory(
                    builder,
                    stage,
                    self.value(value)?,
                    classified.size,
                    gir::MemoryAccess::default(),
                    gir::MemoryAccess::default(),
                )?;
                let mut results = Vec::with_capacity(plan.clif_results.len());
                for carrier in &plan.clif_results {
                    let source = address_offset(builder, stage, carrier.source_offset)?;
                    results.push(builder.ins().load(
                        native_carrier_type(carrier.carrier),
                        MemFlags::new(),
                        source,
                        0,
                    ));
                }
                builder.ins().return_(&results);
            }
            (ccc_abi::NativeResultPlan::Indirect { classified, .. }, Some(value)) => {
                let destination = self
                    .sret
                    .ok_or_else(|| error("aggregate return has no sret destination"))?;
                copy_memory(
                    builder,
                    destination,
                    self.value(value)?,
                    classified.size,
                    gir::MemoryAccess::default(),
                    gir::MemoryAccess::default(),
                )?;
                builder.ins().return_(&[]);
            }
            _ => {
                return Err(error(
                    "typed return value does not match the function ABI result plan",
                ));
            }
        }
        Ok(())
    }

    fn lower_variadic_return(
        &self,
        builder: &mut FunctionBuilder<'_>,
        value: Option<gir::ValueId>,
        plan: &ccc_abi::BridgeBoundaryPlan,
    ) -> Result<(), CodegenError> {
        let frame = self
            .variadic_frame
            .ok_or_else(|| error("variadic hidden body has no entry frame"))?;
        match (plan.result.passing, value) {
            (ccc_abi::PassingMode::Void, None) => {}
            (ccc_abi::PassingMode::Scalar, Some(value)) => {
                let piece = plan
                    .result_pieces
                    .first()
                    .ok_or_else(|| error("variadic scalar result has no bridge piece"))?;
                let destination = variadic_result_piece_address(builder, frame, piece.location)?;
                builder
                    .ins()
                    .store(MemFlags::new(), self.value(value)?, destination, 0);
            }
            (ccc_abi::PassingMode::Registers, Some(value)) => {
                let source = self.value(value)?;
                for piece in &plan.result_pieces {
                    let piece_source = address_offset(builder, source, piece.piece.offset)?;
                    let destination =
                        variadic_result_piece_address(builder, frame, piece.location)?;
                    copy_memory(
                        builder,
                        destination,
                        piece_source,
                        u64::from(piece.piece.valid_bytes),
                        gir::MemoryAccess::default(),
                        gir::MemoryAccess::default(),
                    )?;
                }
            }
            (ccc_abi::PassingMode::Memory, Some(value)) => {
                let destination = self
                    .sret
                    .ok_or_else(|| error("variadic aggregate return has no sret destination"))?;
                copy_memory(
                    builder,
                    destination,
                    self.value(value)?,
                    plan.result.size,
                    gir::MemoryAccess::default(),
                    gir::MemoryAccess::default(),
                )?;
            }
            _ => {
                return Err(error(
                    "typed return value does not match the variadic result plan",
                ));
            }
        }
        builder.ins().return_(&[]);
        Ok(())
    }

    fn lower_terminator(
        &self,
        builder: &mut FunctionBuilder<'_>,
        terminator: &gir::FullTerminator,
    ) -> Result<(), CodegenError> {
        match terminator {
            gir::FullTerminator::Branch(edge) => self.jump(builder, edge),
            gir::FullTerminator::Conditional {
                condition,
                then_edge,
                else_edge,
            } => {
                let then_arguments = self.edge_arguments(then_edge)?;
                let else_arguments = self.edge_arguments(else_edge)?;
                builder.ins().brif(
                    self.value(*condition)?,
                    self.block(then_edge.target.0)?,
                    &then_arguments,
                    self.block(else_edge.target.0)?,
                    &else_arguments,
                );
                Ok(())
            }
            gir::FullTerminator::Switch {
                selector,
                cases,
                default,
            } => {
                let selector = self.value(*selector)?;
                let selector_ty = builder.func.dfg.value_type(selector);
                for case in cases {
                    let next = builder.create_block();
                    let constant = builder.ins().iconst(selector_ty, case.value as i64);
                    let matches = builder.ins().icmp(IntCC::Equal, selector, constant);
                    let arguments = self.edge_arguments(&case.edge)?;
                    builder.ins().brif(
                        matches,
                        self.block(case.edge.target.0)?,
                        &arguments,
                        next,
                        &[],
                    );
                    builder.seal_block(next);
                    builder.switch_to_block(next);
                }
                self.jump(builder, default)
            }
            gir::FullTerminator::Return(value) => self.lower_return(builder, *value),
            gir::FullTerminator::Unreachable => {
                builder.ins().trap(TrapCode::unwrap_user(1));
                Ok(())
            }
        }
    }

    fn jump(
        &self,
        builder: &mut FunctionBuilder<'_>,
        edge: &gir::FullEdge,
    ) -> Result<(), CodegenError> {
        let arguments = self.edge_arguments(edge)?;
        builder.ins().jump(self.block(edge.target.0)?, &arguments);
        Ok(())
    }

    fn edge_arguments(&self, edge: &gir::FullEdge) -> Result<Vec<BlockArg>, CodegenError> {
        edge.arguments
            .iter()
            .map(|argument| self.value(*argument).map(BlockArg::from))
            .collect()
    }
}

fn call_result(
    builder: &FunctionBuilder<'_>,
    call: ir::Inst,
    expected: bool,
) -> Result<Option<ir::Value>, CodegenError> {
    let results = builder.inst_results(call);
    match (expected, results) {
        (false, []) => Ok(None),
        (true, [result]) => Ok(Some(*result)),
        (false, _) => Err(error("void call unexpectedly produced a result")),
        (true, _) => Err(error("scalar call did not produce exactly one result")),
    }
}

fn native_carrier_type(carrier: ccc_abi::AbiCarrier) -> ir::Type {
    match carrier {
        ccc_abi::AbiCarrier::I8 => ir::types::I8,
        ccc_abi::AbiCarrier::I16 => ir::types::I16,
        ccc_abi::AbiCarrier::I32 => ir::types::I32,
        ccc_abi::AbiCarrier::I64 => ir::types::I64,
        ccc_abi::AbiCarrier::F32 => ir::types::F32,
        ccc_abi::AbiCarrier::F64 => ir::types::F64,
    }
}

fn variadic_parameter_piece_address(
    builder: &mut FunctionBuilder<'_>,
    frame: ir::Value,
    plan: &ccc_abi::BridgeBoundaryPlan,
    location: ccc_abi::BridgeLocation,
) -> Result<ir::Value, CodegenError> {
    match location {
        ccc_abi::BridgeLocation::Gp(register) => {
            let index = match register {
                ccc_abi::GpRegister::Rdi => 0,
                ccc_abi::GpRegister::Rsi => 1,
                ccc_abi::GpRegister::Rdx => 2,
                ccc_abi::GpRegister::Rcx => 3,
                ccc_abi::GpRegister::R8 => 4,
                ccc_abi::GpRegister::R9 => 5,
                ccc_abi::GpRegister::Rax => {
                    return Err(error("RAX is not an incoming variadic argument register"));
                }
            };
            address_offset(builder, frame, 32 + index * 8)
        }
        ccc_abi::BridgeLocation::Sse(register) => {
            let index = match register {
                ccc_abi::SseRegister::Xmm0 => 0,
                ccc_abi::SseRegister::Xmm1 => 1,
                ccc_abi::SseRegister::Xmm2 => 2,
                ccc_abi::SseRegister::Xmm3 => 3,
                ccc_abi::SseRegister::Xmm4 => 4,
                ccc_abi::SseRegister::Xmm5 => 5,
                ccc_abi::SseRegister::Xmm6 => 6,
                ccc_abi::SseRegister::Xmm7 => 7,
            };
            address_offset(builder, frame, 80 + index * 16)
        }
        ccc_abi::BridgeLocation::Stack { offset } => {
            let overflow_slot = address_offset(builder, frame, 16)?;
            let overflow = builder
                .ins()
                .load(ir::types::I64, MemFlags::new(), overflow_slot, 0);
            let fixed_stack_base = if plan.overflow_arg_offset == 0 {
                overflow
            } else {
                builder
                    .ins()
                    .iadd_imm(overflow, -i64::from(plan.overflow_arg_offset))
            };
            address_offset(builder, fixed_stack_base, u64::from(offset))
        }
    }
}

fn variadic_result_piece_address(
    builder: &mut FunctionBuilder<'_>,
    frame: ir::Value,
    location: ccc_abi::BridgeLocation,
) -> Result<ir::Value, CodegenError> {
    let offset = match location {
        ccc_abi::BridgeLocation::Gp(ccc_abi::GpRegister::Rax) => 208,
        ccc_abi::BridgeLocation::Gp(ccc_abi::GpRegister::Rdx) => 216,
        ccc_abi::BridgeLocation::Sse(ccc_abi::SseRegister::Xmm0) => 224,
        ccc_abi::BridgeLocation::Sse(ccc_abi::SseRegister::Xmm1) => 240,
        _ => return Err(error("unsupported variadic result bridge location")),
    };
    address_offset(builder, frame, offset)
}

fn bridge_argument_piece_address(
    builder: &mut FunctionBuilder<'_>,
    frame: ir::Value,
    location: ccc_abi::BridgeLocation,
) -> Result<ir::Value, CodegenError> {
    let offset = match location {
        ccc_abi::BridgeLocation::Gp(register) => {
            32 + match register {
                ccc_abi::GpRegister::Rdi => 0,
                ccc_abi::GpRegister::Rsi => 8,
                ccc_abi::GpRegister::Rdx => 16,
                ccc_abi::GpRegister::Rcx => 24,
                ccc_abi::GpRegister::R8 => 32,
                ccc_abi::GpRegister::R9 => 40,
                ccc_abi::GpRegister::Rax => {
                    return Err(error("RAX is not a variadic argument location"));
                }
            }
        }
        ccc_abi::BridgeLocation::Sse(register) => {
            80 + match register {
                ccc_abi::SseRegister::Xmm0 => 0,
                ccc_abi::SseRegister::Xmm1 => 16,
                ccc_abi::SseRegister::Xmm2 => 32,
                ccc_abi::SseRegister::Xmm3 => 48,
                ccc_abi::SseRegister::Xmm4 => 64,
                ccc_abi::SseRegister::Xmm5 => 80,
                ccc_abi::SseRegister::Xmm6 => 96,
                ccc_abi::SseRegister::Xmm7 => 112,
            }
        }
        ccc_abi::BridgeLocation::Stack { offset } => 256 + u64::from(offset),
    };
    address_offset(builder, frame, offset)
}

fn bridge_result_piece_address(
    builder: &mut FunctionBuilder<'_>,
    frame: ir::Value,
    location: ccc_abi::BridgeLocation,
) -> Result<ir::Value, CodegenError> {
    variadic_result_piece_address(builder, frame, location)
}

fn store_integer(
    builder: &mut FunctionBuilder<'_>,
    base: ir::Value,
    offset: u64,
    ty: ir::Type,
    value: i64,
) -> Result<(), CodegenError> {
    let value = builder.ins().iconst(ty, value);
    store_value(builder, base, offset, value)
}

fn store_value(
    builder: &mut FunctionBuilder<'_>,
    base: ir::Value,
    offset: u64,
    value: ir::Value,
) -> Result<(), CodegenError> {
    let destination = address_offset(builder, base, offset)?;
    builder.ins().store(MemFlags::new(), value, destination, 0);
    Ok(())
}

fn value_representation_type(
    types: &TypeStore,
    ty: QualifiedType,
    config: &EffectiveCompilationConfig,
) -> Result<ir::Type, CodegenError> {
    if matches!(
        types.try_kind(ty.ty),
        Some(TypeKind::Array(_) | TypeKind::Record(_))
    ) {
        Ok(ir::types::I64)
    } else {
        scalar_type(types, ty, config)
    }
}

fn create_stack_backing(
    builder: &mut FunctionBuilder<'_>,
    size: u64,
    align: u64,
) -> Result<ir::Value, CodegenError> {
    let size = u32::try_from(size.max(1))
        .map_err(|_| error("ABI-owned stack backing is too large for Cranelift"))?;
    if align == 0 || !align.is_power_of_two() {
        return Err(error(format!("invalid stack backing alignment {align}")));
    }
    let align_shift = u8::try_from(align.trailing_zeros())
        .map_err(|_| error("stack backing alignment is too large"))?;
    let slot = builder.create_sized_stack_slot(StackSlotData::new(
        StackSlotKind::ExplicitSlot,
        size,
        align_shift,
    ));
    Ok(builder.ins().stack_addr(ir::types::I64, slot, 0))
}

fn align_up_u64(value: u64, align: u64) -> Result<u64, CodegenError> {
    if align == 0 || !align.is_power_of_two() {
        return Err(error(format!("invalid ABI alignment {align}")));
    }
    value
        .checked_add(align - 1)
        .map(|value| value & !(align - 1))
        .ok_or_else(|| error("ABI staging size overflow"))
}

fn va_arg_result(
    builder: &mut FunctionBuilder<'_>,
    types: &TypeStore,
    requested: QualifiedType,
    address: ir::Value,
    config: &EffectiveCompilationConfig,
) -> Result<ir::Value, CodegenError> {
    if matches!(
        types.try_kind(requested.ty),
        Some(TypeKind::Array(_) | TypeKind::Record(_))
    ) {
        Ok(address)
    } else {
        Ok(builder.ins().load(
            scalar_type(types, requested, config)?,
            MemFlags::new(),
            address,
            0,
        ))
    }
}

pub(super) fn scalar_type(
    types: &TypeStore,
    ty: QualifiedType,
    config: &EffectiveCompilationConfig,
) -> Result<ir::Type, CodegenError> {
    match types.try_kind(ty.ty) {
        Some(TypeKind::Builtin(BuiltinType::Void)) => {
            Err(error("`void` cannot be lowered to a Cranelift value"))
        }
        Some(TypeKind::Builtin(BuiltinType::Float)) => Ok(ir::types::F32),
        Some(TypeKind::Builtin(BuiltinType::Double)) => Ok(ir::types::F64),
        Some(TypeKind::Builtin(BuiltinType::LongDouble)) => Err(CodegenError {
            code: "CCC3509",
            message: "native `long double` values require the target long-double bridge capability"
                .to_owned(),
            span: None,
        }),
        Some(TypeKind::Builtin(_)) => {
            let layout = object_layout(types, ty, config)?;
            integer_type_for_size(layout.size, "integer")
        }
        Some(TypeKind::Enum(id)) => {
            let underlying = types
                .enumeration(*id)
                .and_then(|definition| definition.body.as_ref())
                .ok_or_else(|| error(format!("enum `{}` is incomplete", types.display(ty.ty))))?
                .underlying;
            scalar_type(types, QualifiedType::unqualified(underlying), config)
        }
        Some(TypeKind::Pointer(_)) => Ok(ir::types::I64),
        Some(TypeKind::Array(_) | TypeKind::Record(_)) => Err(CodegenError {
            code: "CCC3508",
            message: format!(
                "aggregate value type `{}` cannot be lowered as a scalar",
                types.display(ty.ty)
            ),
            span: None,
        }),
        Some(TypeKind::Function(_)) => Err(error(format!(
            "function type `{}` must be converted to a pointer before value lowering",
            types.display(ty.ty)
        ))),
        None => Err(error(format!("unknown type {}", ty.ty.index()))),
    }
}

fn integer_type_for_size(size: u64, class: &str) -> Result<ir::Type, CodegenError> {
    match size {
        1 => Ok(ir::types::I8),
        2 => Ok(ir::types::I16),
        4 => Ok(ir::types::I32),
        8 => Ok(ir::types::I64),
        _ => Err(error(format!("unsupported {class} size {size}"))),
    }
}

fn is_signed(
    types: &TypeStore,
    ty: QualifiedType,
    config: &EffectiveCompilationConfig,
) -> Result<bool, CodegenError> {
    match types.try_kind(ty.ty) {
        Some(TypeKind::Builtin(builtin)) => Ok(match builtin {
            BuiltinType::Char => config.target.data_layout.char_is_signed,
            BuiltinType::SignedChar
            | BuiltinType::Short
            | BuiltinType::Int
            | BuiltinType::Long
            | BuiltinType::LongLong => true,
            BuiltinType::Bool
            | BuiltinType::UnsignedChar
            | BuiltinType::UnsignedShort
            | BuiltinType::UnsignedInt
            | BuiltinType::UnsignedLong
            | BuiltinType::UnsignedLongLong => false,
            BuiltinType::Void
            | BuiltinType::Float
            | BuiltinType::Double
            | BuiltinType::LongDouble => {
                return Err(error(format!(
                    "type `{}` has no integer signedness",
                    types.display(ty.ty)
                )));
            }
        }),
        Some(TypeKind::Enum(id)) => {
            let underlying = types
                .enumeration(*id)
                .and_then(|definition| definition.body.as_ref())
                .ok_or_else(|| error(format!("enum `{}` is incomplete", types.display(ty.ty))))?
                .underlying;
            is_signed(types, QualifiedType::unqualified(underlying), config)
        }
        Some(TypeKind::Pointer(_)) => Ok(false),
        _ => Err(error(format!(
            "type `{}` has no integer signedness",
            types.display(ty.ty)
        ))),
    }
}

fn is_float(types: &TypeStore, ty: QualifiedType) -> bool {
    matches!(
        types.builtin_type(ty.ty),
        Some(BuiltinType::Float | BuiltinType::Double | BuiltinType::LongDouble)
    )
}

fn lower_constant(
    builder: &mut FunctionBuilder<'_>,
    types: &TypeStore,
    ty: QualifiedType,
    constant: gir::ScalarConstant,
    config: &EffectiveCompilationConfig,
) -> Result<ir::Value, CodegenError> {
    let clif_ty = scalar_type(types, ty, config)?;
    if types.builtin_type(ty.ty) == Some(BuiltinType::Bool) {
        let normalized = scalar_constant_bits(types, ty, constant, config)? as i64;
        return Ok(builder.ins().iconst(ir::types::I8, normalized));
    }
    match constant {
        gir::ScalarConstant::Signed(value) => Ok(builder.ins().iconst(clif_ty, value as i64)),
        gir::ScalarConstant::Unsigned(value) => Ok(builder.ins().iconst(clif_ty, value as i64)),
        gir::ScalarConstant::Floating(value) => match clif_ty {
            ir::types::F32 => Ok(builder
                .ins()
                .f32const(Ieee32::with_bits((value as f32).to_bits()))),
            ir::types::F64 => Ok(builder.ins().f64const(Ieee64::with_bits(value.to_bits()))),
            _ => Err(error("floating constant has a non-floating result type")),
        },
        gir::ScalarConstant::NullPointer => {
            if !matches!(types.try_kind(ty.ty), Some(TypeKind::Pointer(_))) {
                return Err(error("null pointer constant has a non-pointer result type"));
            }
            Ok(builder.ins().iconst(ir::types::I64, 0))
        }
    }
}

fn validate_access(access: gir::MemoryAccess) -> Result<(), CodegenError> {
    if access.atomic.is_some() {
        return Err(CodegenError {
            code: ATOMIC_ERROR,
            message: "atomic memory access requires native atomic instruction lowering".to_owned(),
            span: None,
        });
    }
    Ok(())
}

fn lower_load(
    builder: &mut FunctionBuilder<'_>,
    address: ir::Value,
    ty: ir::Type,
    access: gir::MemoryAccess,
) -> Result<ir::Value, CodegenError> {
    validate_access(access)?;
    if access.volatile {
        builder.ins().fence();
    }
    let value = builder.ins().load(ty, MemFlags::new(), address, 0);
    if access.volatile {
        builder.ins().fence();
    }
    Ok(value)
}

fn lower_store(
    builder: &mut FunctionBuilder<'_>,
    address: ir::Value,
    value: ir::Value,
    access: gir::MemoryAccess,
) -> Result<(), CodegenError> {
    validate_access(access)?;
    if access.volatile {
        builder.ins().fence();
    }
    builder.ins().store(MemFlags::new(), value, address, 0);
    if access.volatile {
        builder.ins().fence();
    }
    Ok(())
}

fn address_offset(
    builder: &mut FunctionBuilder<'_>,
    address: ir::Value,
    offset: u64,
) -> Result<ir::Value, CodegenError> {
    if offset == 0 {
        return Ok(address);
    }
    Ok(builder.ins().iadd_imm(
        address,
        i64::try_from(offset).map_err(|_| error("object offset exceeds signed address range"))?,
    ))
}

fn zero_memory(
    builder: &mut FunctionBuilder<'_>,
    destination: ir::Value,
    size: u64,
) -> Result<(), CodegenError> {
    let zero = builder.ins().iconst(ir::types::I8, 0);
    for offset in 0..size {
        let address = address_offset(builder, destination, offset)?;
        builder.ins().store(MemFlags::new(), zero, address, 0);
    }
    Ok(())
}

/// Loads the entire source before storing any destination byte. This is the
/// same observable overlap behavior as `memmove` and also lets volatile copies
/// keep every source read and destination write explicit in CLIF.
fn copy_memory(
    builder: &mut FunctionBuilder<'_>,
    destination: ir::Value,
    source: ir::Value,
    size: u64,
    destination_access: gir::MemoryAccess,
    source_access: gir::MemoryAccess,
) -> Result<(), CodegenError> {
    validate_access(destination_access)?;
    validate_access(source_access)?;
    let size = usize::try_from(size)
        .map_err(|_| error("aggregate copy is too large for function lowering"))?;
    let mut bytes = Vec::with_capacity(size);
    for offset in 0..size {
        let address = address_offset(builder, source, offset as u64)?;
        bytes.push(lower_load(builder, address, ir::types::I8, source_access)?);
    }
    for (offset, byte) in bytes.into_iter().enumerate() {
        let address = address_offset(builder, destination, offset as u64)?;
        lower_store(builder, address, byte, destination_access)?;
    }
    Ok(())
}

fn validate_bitfield(descriptor: gir::BitfieldDescriptor) -> Result<(), CodegenError> {
    if !matches!(descriptor.storage_size, 1 | 2 | 4 | 8) {
        return Err(error(format!(
            "bitfield {} uses unsupported storage size {}",
            descriptor.field_index, descriptor.storage_size
        )));
    }
    let bits = u32::try_from(descriptor.storage_size * 8)
        .map_err(|_| error("bitfield storage width overflow"))?;
    if descriptor.width == 0
        || descriptor.width > bits
        || descriptor.bit_offset + descriptor.width > bits
    {
        return Err(error(format!(
            "bitfield {} has invalid storage geometry",
            descriptor.field_index
        )));
    }
    Ok(())
}

fn lower_conversion(
    builder: &mut FunctionBuilder<'_>,
    types: &TypeStore,
    operand: ir::Value,
    kind: gir::ScalarConversion,
    from: QualifiedType,
    to: QualifiedType,
    config: &EffectiveCompilationConfig,
) -> Result<Option<ir::Value>, CodegenError> {
    if kind == gir::ScalarConversion::ToVoid {
        return Ok(None);
    }
    let destination = scalar_type(types, to, config)?;
    if kind == gir::ScalarConversion::ToBoolean
        || types.builtin_type(to.ty) == Some(BuiltinType::Bool)
    {
        return Ok(Some(normalize_bool(
            builder,
            operand,
            from,
            destination,
            types,
        )?));
    }
    let source = builder.func.dfg.value_type(operand);
    let value = match kind {
        gir::ScalarConversion::ArrayToPointer
        | gir::ScalarConversion::FunctionToPointer
        | gir::ScalarConversion::QualificationAdjustment => {
            coerce_value(builder, operand, destination, false)?
        }
        gir::ScalarConversion::PointerConversion => {
            let signed = if types.is_integer(from.ty) {
                is_signed(types, from, config)?
            } else {
                false
            };
            coerce_value(builder, operand, destination, signed)?
        }
        gir::ScalarConversion::IntegerPromotion | gir::ScalarConversion::IntegerConversion => {
            coerce_integer(
                builder,
                operand,
                source,
                destination,
                is_signed(types, from, config)?,
            )
        }
        gir::ScalarConversion::FloatingConversion => match (source, destination) {
            (ir::types::F32, ir::types::F32) | (ir::types::F64, ir::types::F64) => operand,
            (ir::types::F32, ir::types::F64) => builder.ins().fpromote(destination, operand),
            (ir::types::F64, ir::types::F32) => builder.ins().fdemote(destination, operand),
            _ => return Err(error("invalid floating conversion types")),
        },
        gir::ScalarConversion::IntegerToFloating => {
            if is_signed(types, from, config)? {
                builder.ins().fcvt_from_sint(destination, operand)
            } else {
                builder.ins().fcvt_from_uint(destination, operand)
            }
        }
        gir::ScalarConversion::FloatingToInteger => {
            if is_signed(types, to, config)? {
                builder.ins().fcvt_to_sint(destination, operand)
            } else {
                builder.ins().fcvt_to_uint(destination, operand)
            }
        }
        gir::ScalarConversion::ToBoolean | gir::ScalarConversion::ToVoid => unreachable!(),
    };
    Ok(Some(value))
}

fn normalize_bool(
    builder: &mut FunctionBuilder<'_>,
    operand: ir::Value,
    from: QualifiedType,
    destination: ir::Type,
    types: &TypeStore,
) -> Result<ir::Value, CodegenError> {
    let boolean = if is_float(types, from) {
        let source = builder.func.dfg.value_type(operand);
        let zero = match source {
            ir::types::F32 => builder.ins().f32const(Ieee32::with_bits(0)),
            ir::types::F64 => builder.ins().f64const(Ieee64::with_bits(0)),
            _ => return Err(error("floating boolean conversion has invalid source type")),
        };
        builder.ins().fcmp(FloatCC::NotEqual, operand, zero)
    } else {
        builder.ins().icmp_imm(IntCC::NotEqual, operand, 0)
    };
    let source = builder.func.dfg.value_type(boolean);
    Ok(coerce_integer(builder, boolean, source, destination, false))
}

fn coerce_value(
    builder: &mut FunctionBuilder<'_>,
    value: ir::Value,
    destination: ir::Type,
    signed: bool,
) -> Result<ir::Value, CodegenError> {
    let source = builder.func.dfg.value_type(value);
    if source == destination {
        return Ok(value);
    }
    if source.is_int() && destination.is_int() {
        return Ok(coerce_integer(builder, value, source, destination, signed));
    }
    Err(error(format!(
        "cannot coerce Cranelift value from {source} to {destination}"
    )))
}

fn coerce_integer(
    builder: &mut FunctionBuilder<'_>,
    value: ir::Value,
    source: ir::Type,
    destination: ir::Type,
    signed: bool,
) -> ir::Value {
    if source == destination {
        value
    } else if source.bits() > destination.bits() {
        builder.ins().ireduce(destination, value)
    } else if signed {
        builder.ins().sextend(destination, value)
    } else {
        builder.ins().uextend(destination, value)
    }
}

fn lower_unary(
    builder: &mut FunctionBuilder<'_>,
    types: &TypeStore,
    operator: gir::UnaryOperation,
    operand: ir::Value,
    operand_ty: QualifiedType,
    result_ty: QualifiedType,
    config: &EffectiveCompilationConfig,
) -> Result<ir::Value, CodegenError> {
    let result = scalar_type(types, result_ty, config)?;
    match operator {
        gir::UnaryOperation::Plus => coerce_value(
            builder,
            operand,
            result,
            if is_float(types, operand_ty) {
                false
            } else {
                is_signed(types, operand_ty, config)?
            },
        ),
        gir::UnaryOperation::Negate if is_float(types, operand_ty) => {
            Ok(builder.ins().fneg(operand))
        }
        gir::UnaryOperation::Negate => Ok(builder.ins().ineg(operand)),
        gir::UnaryOperation::BitwiseNot => {
            if is_float(types, operand_ty) {
                return Err(error(
                    "bitwise complement cannot be applied to floating values",
                ));
            }
            Ok(builder.ins().bnot(operand))
        }
        gir::UnaryOperation::LogicalNot => {
            let boolean = if is_float(types, operand_ty) {
                let source = builder.func.dfg.value_type(operand);
                let zero = match source {
                    ir::types::F32 => builder.ins().f32const(Ieee32::with_bits(0)),
                    ir::types::F64 => builder.ins().f64const(Ieee64::with_bits(0)),
                    _ => return Err(error("floating logical-not has invalid source type")),
                };
                builder.ins().fcmp(FloatCC::Equal, operand, zero)
            } else {
                builder.ins().icmp_imm(IntCC::Equal, operand, 0)
            };
            let source = builder.func.dfg.value_type(boolean);
            Ok(coerce_integer(builder, boolean, source, result, false))
        }
    }
}

fn lower_binary(
    builder: &mut FunctionBuilder<'_>,
    operator: gir::BinaryOperation,
    left: ir::Value,
    right: ir::Value,
    floating: bool,
    signed: bool,
    result: ir::Type,
) -> Result<ir::Value, CodegenError> {
    let comparison = matches!(
        operator,
        gir::BinaryOperation::Less
            | gir::BinaryOperation::LessEqual
            | gir::BinaryOperation::Greater
            | gir::BinaryOperation::GreaterEqual
            | gir::BinaryOperation::Equal
            | gir::BinaryOperation::NotEqual
    );
    if comparison {
        let boolean = if floating {
            let condition = match operator {
                gir::BinaryOperation::Less => FloatCC::LessThan,
                gir::BinaryOperation::LessEqual => FloatCC::LessThanOrEqual,
                gir::BinaryOperation::Greater => FloatCC::GreaterThan,
                gir::BinaryOperation::GreaterEqual => FloatCC::GreaterThanOrEqual,
                gir::BinaryOperation::Equal => FloatCC::Equal,
                gir::BinaryOperation::NotEqual => FloatCC::NotEqual,
                _ => unreachable!(),
            };
            builder.ins().fcmp(condition, left, right)
        } else {
            let condition = match operator {
                gir::BinaryOperation::Less if signed => IntCC::SignedLessThan,
                gir::BinaryOperation::Less => IntCC::UnsignedLessThan,
                gir::BinaryOperation::LessEqual if signed => IntCC::SignedLessThanOrEqual,
                gir::BinaryOperation::LessEqual => IntCC::UnsignedLessThanOrEqual,
                gir::BinaryOperation::Greater if signed => IntCC::SignedGreaterThan,
                gir::BinaryOperation::Greater => IntCC::UnsignedGreaterThan,
                gir::BinaryOperation::GreaterEqual if signed => IntCC::SignedGreaterThanOrEqual,
                gir::BinaryOperation::GreaterEqual => IntCC::UnsignedGreaterThanOrEqual,
                gir::BinaryOperation::Equal => IntCC::Equal,
                gir::BinaryOperation::NotEqual => IntCC::NotEqual,
                _ => unreachable!(),
            };
            builder.ins().icmp(condition, left, right)
        };
        let source = builder.func.dfg.value_type(boolean);
        return Ok(coerce_integer(builder, boolean, source, result, false));
    }
    if floating {
        return match operator {
            gir::BinaryOperation::Multiply => Ok(builder.ins().fmul(left, right)),
            gir::BinaryOperation::Divide => Ok(builder.ins().fdiv(left, right)),
            gir::BinaryOperation::Add => Ok(builder.ins().fadd(left, right)),
            gir::BinaryOperation::Subtract => Ok(builder.ins().fsub(left, right)),
            _ => Err(error(format!(
                "operator {operator:?} is invalid for floating values"
            ))),
        };
    }
    Ok(match operator {
        gir::BinaryOperation::Multiply => builder.ins().imul(left, right),
        gir::BinaryOperation::Divide if signed => builder.ins().sdiv(left, right),
        gir::BinaryOperation::Divide => builder.ins().udiv(left, right),
        gir::BinaryOperation::Remainder if signed => builder.ins().srem(left, right),
        gir::BinaryOperation::Remainder => builder.ins().urem(left, right),
        gir::BinaryOperation::Add => builder.ins().iadd(left, right),
        gir::BinaryOperation::Subtract => builder.ins().isub(left, right),
        gir::BinaryOperation::LeftShift => builder.ins().ishl(left, right),
        gir::BinaryOperation::RightShift if signed => builder.ins().sshr(left, right),
        gir::BinaryOperation::RightShift => builder.ins().ushr(left, right),
        gir::BinaryOperation::BitwiseAnd => builder.ins().band(left, right),
        gir::BinaryOperation::BitwiseXor => builder.ins().bxor(left, right),
        gir::BinaryOperation::BitwiseOr => builder.ins().bor(left, right),
        _ => unreachable!(),
    })
}

fn value_type(
    function: &gir::FullFunction,
    id: gir::ValueId,
) -> Result<QualifiedType, CodegenError> {
    function
        .value_types
        .get(id.0 as usize)
        .copied()
        .map(QualifiedType::unqualified)
        .ok_or_else(|| error(format!("IR value v{} has no type", id.0)))
}

fn block_ref(blocks: &HashMap<u32, ir::Block>, raw: u32) -> Result<ir::Block, CodegenError> {
    blocks
        .get(&raw)
        .copied()
        .ok_or_else(|| error(format!("reference to unknown block {raw}")))
}
