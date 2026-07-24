use super::data::{low_mask_u128, scalar_constant_bits, string_unit_bytes};
use super::*;
use std::cell::{Cell, RefCell};

const F80_OP_ADD: i64 = 1;
const F80_OP_SUBTRACT: i64 = 2;
const F80_OP_MULTIPLY: i64 = 3;
const F80_OP_DIVIDE: i64 = 4;
const F80_OP_COMPARE_QUIET: i64 = 5;
const F80_OP_NEGATE: i64 = 6;
const F80_OP_FROM_I64: i64 = 7;
const F80_OP_FROM_U64: i64 = 8;
const F80_OP_FROM_F32: i64 = 9;
const F80_OP_FROM_F64: i64 = 10;
const F80_OP_TO_I64: i64 = 11;
const F80_OP_TO_U64: i64 = 12;
const F80_OP_TO_F32: i64 = 13;
const F80_OP_TO_F64: i64 = 14;
const F80_OP_VOLATILE_LOAD: i64 = 15;
const F80_OP_VOLATILE_STORE: i64 = 16;
const F80_OP_FROM_I128: i64 = 17;
const F80_OP_FROM_U128: i64 = 18;
const F80_OP_TO_I128: i64 = 19;
const F80_OP_TO_U128: i64 = 20;
const F80_OP_COMPARE_SIGNALING: i64 = 21;

/// Per-function imports, interned in deterministic lowering order.
///
/// Module declarations describe what may be referenced, but importing all of
/// them into every CLIF function creates unused signatures, function refs, and
/// global values. Keeping the declaration table separate from these caches
/// makes the first actual use the only point that creates a CLIF entity.
pub(super) struct FunctionReferences<'a> {
    declarations: &'a Declarations,
    object_module: RefCell<&'a mut ObjectModule>,
    functions: RefCell<HashMap<u32, FunctionReference>>,
    globals: RefCell<HashMap<u32, DataReference>>,
    strings: RefCell<HashMap<u32, ir::GlobalValue>>,
    call_helper: Cell<Option<ir::FuncRef>>,
    runtime_realloc: Cell<Option<ir::FuncRef>>,
    runtime_free: Cell<Option<ir::FuncRef>>,
    runtime_helpers: RefCell<HashMap<&'static str, ir::FuncRef>>,
    f80_support: Cell<Option<ir::FuncRef>>,
    inline_cpuid_support: Cell<Option<ir::FuncRef>>,
    inline_rdtsc_support: Cell<Option<ir::FuncRef>>,
}

#[derive(Clone, Copy)]
pub(super) enum DefinitionAbi<'a> {
    Native(&'a ccc_abi::NativeBoundaryPlan),
    Variadic(&'a ccc_abi::BridgeBoundaryPlan),
}

#[derive(Clone, Copy, Default)]
struct FunctionReference {
    address: Option<ir::FuncRef>,
    direct_call: Option<ir::FuncRef>,
}

#[derive(Clone, Copy)]
struct DataReference {
    value: Option<ir::GlobalValue>,
    tls_accessor: Option<ir::FuncRef>,
}

#[derive(Clone, Copy)]
struct StackStorage {
    slot: StackSlot,
    dynamic_alignment: Option<u64>,
}

/// Machine-stack identities retained for source-level debug locations.
///
/// Runtime-sized arena objects and dynamically realigned slots deliberately
/// have no entry: their address is computed at run time and cannot be
/// described by one fixed frame-relative expression.
pub(super) struct FunctionDebugLayout {
    pub(super) storage_slots: HashMap<u32, StackSlot>,
}

impl<'a> FunctionReferences<'a> {
    pub(super) fn new(declarations: &'a Declarations, object_module: &'a mut ObjectModule) -> Self {
        Self {
            declarations,
            object_module: RefCell::new(object_module),
            functions: RefCell::new(HashMap::new()),
            globals: RefCell::new(HashMap::new()),
            strings: RefCell::new(HashMap::new()),
            call_helper: Cell::new(None),
            runtime_realloc: Cell::new(None),
            runtime_free: Cell::new(None),
            runtime_helpers: RefCell::new(HashMap::new()),
            f80_support: Cell::new(None),
            inline_cpuid_support: Cell::new(None),
            inline_rdtsc_support: Cell::new(None),
        }
    }
}

impl FunctionReferences<'_> {
    fn import_function(&self, builder: &mut FunctionBuilder<'_>, id: FuncId) -> ir::FuncRef {
        self.object_module
            .borrow_mut()
            .declare_func_in_func(id, builder.func)
    }

    fn function_address(
        &self,
        builder: &mut FunctionBuilder<'_>,
        raw: u32,
    ) -> Result<ir::FuncRef, CodegenError> {
        if let Some(reference) = self
            .functions
            .borrow()
            .get(&raw)
            .and_then(|reference| reference.address)
        {
            return Ok(reference);
        }
        let id = self
            .declarations
            .functions
            .get(&raw)
            .copied()
            .ok_or_else(|| error(format!("reference to undeclared function {raw}")))?;
        let reference = self.import_function(builder, id);
        self.functions.borrow_mut().entry(raw).or_default().address = Some(reference);
        Ok(reference)
    }

    fn direct_function(
        &self,
        builder: &mut FunctionBuilder<'_>,
        raw: u32,
    ) -> Result<ir::FuncRef, CodegenError> {
        if let Some(reference) = self
            .functions
            .borrow()
            .get(&raw)
            .and_then(|reference| reference.direct_call)
        {
            return Ok(reference);
        }
        let id = self
            .declarations
            .functions
            .get(&raw)
            .copied()
            .ok_or_else(|| error(format!("call references undeclared function {raw}")))?;
        let reference = self.import_function(builder, id);
        // Native C direct calls use the target's PC-relative call relocation.
        // Address materialization is interned separately so that its linkage
        // and preemption semantics remain module-defined.
        builder.func.dfg.ext_funcs[reference].colocated = true;
        self.functions
            .borrow_mut()
            .entry(raw)
            .or_default()
            .direct_call = Some(reference);
        Ok(reference)
    }

    fn global(
        &self,
        builder: &mut FunctionBuilder<'_>,
        raw: u32,
    ) -> Result<DataReference, CodegenError> {
        if let Some(reference) = self.globals.borrow().get(&raw).copied() {
            return Ok(reference);
        }
        let declaration = self
            .declarations
            .globals
            .get(&raw)
            .copied()
            .ok_or_else(|| error(format!("reference to undeclared data object {raw}")))?;
        let reference = DataReference {
            value: (!declaration.tls).then(|| {
                self.object_module
                    .borrow_mut()
                    .declare_data_in_func(declaration.id, builder.func)
            }),
            tls_accessor: declaration
                .tls_accessor
                .map(|id| self.import_function(builder, id)),
        };
        self.globals.borrow_mut().insert(raw, reference);
        Ok(reference)
    }

    fn string(
        &self,
        builder: &mut FunctionBuilder<'_>,
        raw: u32,
    ) -> Result<ir::GlobalValue, CodegenError> {
        if let Some(reference) = self.strings.borrow().get(&raw).copied() {
            return Ok(reference);
        }
        let id = self
            .declarations
            .strings
            .get(&raw)
            .copied()
            .ok_or_else(|| error(format!("reference to undeclared string {raw}")))?;
        let reference = self
            .object_module
            .borrow_mut()
            .declare_data_in_func(id, builder.func);
        self.strings.borrow_mut().insert(raw, reference);
        Ok(reference)
    }

    fn optional_function(
        &self,
        builder: &mut FunctionBuilder<'_>,
        declaration: Option<FuncId>,
        cache: &Cell<Option<ir::FuncRef>>,
        missing: &'static str,
    ) -> Result<ir::FuncRef, CodegenError> {
        if let Some(reference) = cache.get() {
            return Ok(reference);
        }
        let id = declaration.ok_or_else(|| error(missing))?;
        let reference = self.import_function(builder, id);
        cache.set(Some(reference));
        Ok(reference)
    }

    fn call_helper(&self, builder: &mut FunctionBuilder<'_>) -> Result<ir::FuncRef, CodegenError> {
        self.optional_function(
            builder,
            self.declarations.call_helper,
            &self.call_helper,
            "variadic call bridge has no translation-unit helper",
        )
    }

    fn runtime_realloc(
        &self,
        builder: &mut FunctionBuilder<'_>,
    ) -> Result<ir::FuncRef, CodegenError> {
        self.optional_function(
            builder,
            self.declarations.runtime_realloc,
            &self.runtime_realloc,
            "runtime-sized storage has no realloc declaration",
        )
    }

    fn runtime_free(&self, builder: &mut FunctionBuilder<'_>) -> Result<ir::FuncRef, CodegenError> {
        self.optional_function(
            builder,
            self.declarations.runtime_free,
            &self.runtime_free,
            "runtime-sized storage has no free declaration",
        )
    }

    fn f80_support(&self, builder: &mut FunctionBuilder<'_>) -> Result<ir::FuncRef, CodegenError> {
        self.optional_function(
            builder,
            self.declarations.f80_support,
            &self.f80_support,
            "x87 operation has no generated support helper",
        )
    }

    fn inline_cpuid_support(
        &self,
        builder: &mut FunctionBuilder<'_>,
    ) -> Result<ir::FuncRef, CodegenError> {
        self.optional_function(
            builder,
            self.declarations.inline_cpuid_support,
            &self.inline_cpuid_support,
            "x86 CPUID operation has no generated native helper",
        )
    }

    fn inline_rdtsc_support(
        &self,
        builder: &mut FunctionBuilder<'_>,
    ) -> Result<ir::FuncRef, CodegenError> {
        self.optional_function(
            builder,
            self.declarations.inline_rdtsc_support,
            &self.inline_rdtsc_support,
            "x86 RDTSC operation has no generated native helper",
        )
    }

    fn runtime_helper(
        &self,
        builder: &mut FunctionBuilder<'_>,
        symbol: &'static str,
    ) -> Result<ir::FuncRef, CodegenError> {
        if let Some(reference) = self.runtime_helpers.borrow().get(symbol).copied() {
            return Ok(reference);
        }
        let id = self
            .declarations
            .runtime_helpers
            .get(symbol)
            .copied()
            .ok_or_else(|| {
                error(format!(
                    "runtime helper `{symbol}` has no target manifest entry"
                ))
            })?;
        let reference = self.import_function(builder, id);
        self.runtime_helpers.borrow_mut().insert(symbol, reference);
        Ok(reference)
    }
}

fn blocks_in_reverse_postorder(
    function: &gir::FullFunction,
    entry: gir::BlockId,
) -> Result<Vec<&gir::FullBlock>, CodegenError> {
    let by_id = function
        .blocks
        .iter()
        .map(|block| (block.id.0, block))
        .collect::<HashMap<_, _>>();
    let mut visited = HashSet::with_capacity(function.blocks.len());
    let mut ordered = Vec::with_capacity(function.blocks.len());

    for root in std::iter::once(entry).chain(function.blocks.iter().map(|block| block.id)) {
        if visited.contains(&root.0) {
            continue;
        }
        if !by_id.contains_key(&root.0) {
            return Err(error(format!(
                "function `{}` references absent entry block {}",
                function.symbol_name, root.0
            )));
        }

        let mut postorder = Vec::new();
        let mut pending = vec![(root, false)];
        while let Some((block, expanded)) = pending.pop() {
            if expanded {
                postorder.push(block);
                continue;
            }
            if !visited.insert(block.0) {
                continue;
            }
            pending.push((block, true));
            let definition = by_id.get(&block.0).copied().ok_or_else(|| {
                error(format!(
                    "function `{}` targets absent block {}",
                    function.symbol_name, block.0
                ))
            })?;
            let mut successors = definition
                .terminator
                .as_ref()
                .map(terminator_successors)
                .unwrap_or_default();
            successors.reverse();
            pending.extend(
                successors
                    .into_iter()
                    .filter(|successor| !visited.contains(&successor.0))
                    .map(|successor| (successor, false)),
            );
        }
        postorder.reverse();
        for block in postorder {
            let definition = by_id.get(&block.0).copied().ok_or_else(|| {
                error(format!(
                    "function `{}` targets absent block {}",
                    function.symbol_name, block.0
                ))
            })?;
            ordered.push(definition);
        }
    }
    Ok(ordered)
}

fn terminator_successors(terminator: &gir::FullTerminator) -> Vec<gir::BlockId> {
    match terminator {
        gir::FullTerminator::Branch(edge) => vec![edge.target],
        gir::FullTerminator::Conditional {
            then_edge,
            else_edge,
            ..
        } => vec![then_edge.target, else_edge.target],
        gir::FullTerminator::Switch { cases, default, .. } => cases
            .iter()
            .map(|case| case.edge.target)
            .chain(std::iter::once(default.target))
            .collect(),
        gir::FullTerminator::IndirectBranch { targets, .. } => {
            targets.iter().map(|edge| edge.target).collect()
        }
        gir::FullTerminator::Return(_) | gir::FullTerminator::Unreachable => Vec::new(),
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn lower_function(
    module: &gir::FullModule,
    function: &gir::FullFunction,
    config: &EffectiveCompilationConfig,
    abi_plan: ccc_abi::VerifiedModuleAbiPlan<'_>,
    definition_plan: DefinitionAbi<'_>,
    references: &FunctionReferences<'_>,
    frontend_config: backend::FrontendConfig,
    clif_function: &mut ir::Function,
    mut debug_locations: Option<&mut super::debug::SourceLocationRegistry>,
) -> Result<FunctionDebugLayout, CodegenError> {
    let entry = function.entry.ok_or_else(|| {
        error(format!(
            "function definition `{}` has no entry block",
            function.symbol_name
        ))
    })?;
    let collect_debug_values = debug_locations.is_some();
    if collect_debug_values {
        clif_function.collect_debug_info();
    }
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
    let mut runtime_storage = HashMap::new();
    for object in &function.storage {
        if object.location == gir::StorageLocation::RuntimeSized {
            let slot = builder.create_sized_stack_slot(StackSlotData::new(
                StackSlotKind::ExplicitSlot,
                16,
                3,
            ));
            if runtime_storage.insert(object.id.0, slot).is_some() {
                return Err(error(format!(
                    "function `{}` contains duplicate runtime storage id {}",
                    function.symbol_name, object.id.0
                )));
            }
            continue;
        } else if object.location != gir::StorageLocation::Automatic {
            continue;
        }
        let layout = object_layout(&module.types, object.ty, config)?;
        let alignment = super::data::requested_alignment(object.requested_alignment, layout.align)?;
        // Cranelift's stack slots express an alignment requirement, but the x86-64
        // backend does not realign the frame above the ABI's 16-byte guarantee.
        // Reserve padding and align the address within the slot for over-aligned
        // automatic objects instead.
        const ABI_STACK_ALIGNMENT: u64 = 16;
        let dynamic_alignment = (alignment > ABI_STACK_ALIGNMENT).then_some(alignment);
        let padded_size = if dynamic_alignment.is_some() {
            layout
                .size
                .checked_add(alignment - 1)
                .ok_or_else(|| error("automatic object stack allocation size overflow"))?
        } else {
            layout.size
        };
        let size = u32::try_from(padded_size).map_err(|_| {
            error(format!(
                "automatic object `{}` is too large for a Cranelift stack slot",
                object.name
            ))
        })?;
        let slot_alignment = alignment.min(ABI_STACK_ALIGNMENT);
        let align_shift = u8::try_from(slot_alignment.trailing_zeros())
            .map_err(|_| error("stack object alignment is too large"))?;
        let slot = builder.create_sized_stack_slot(StackSlotData::new(
            StackSlotKind::ExplicitSlot,
            size,
            align_shift,
        ));
        if storage
            .insert(
                object.id.0,
                StackStorage {
                    slot,
                    dynamic_alignment,
                },
            )
            .is_some()
        {
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
        frontend_config,
        blocks,
        storage,
        runtime_storage,
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

    let ordered_blocks = blocks_in_reverse_postorder(function, entry)?;
    for block in ordered_blocks {
        builder.switch_to_block(state.block(block.id.0)?);
        if block.id == entry {
            if let Some(locations) = debug_locations.as_deref_mut() {
                builder.set_srcloc(locations.intern(function.span)?);
            }
            let entry_values = builder.block_params(state.block(entry.0)?).to_vec();
            state.initialize_runtime_storage(&mut builder);
            state.bind_entry_parameters(&mut builder, &entry_values)?;
            if collect_debug_values {
                let parameters = state
                    .function
                    .parameters
                    .iter()
                    .enumerate()
                    .filter_map(|(index, parameter)| {
                        (parameter.storage.is_none()).then_some((index, parameter.incoming?))
                    })
                    .collect::<Vec<_>>();
                for (index, incoming) in parameters {
                    let index = u32::try_from(index).map_err(|_| {
                        error("debug parameter index exceeds Cranelift label space")
                    })?;
                    builder.set_val_label(state.value(incoming)?, ir::ValueLabel::from_u32(index));
                }
            }
        }
        for instruction in &block.instructions {
            if let Some(locations) = debug_locations.as_deref_mut() {
                builder.set_srcloc(locations.intern(instruction.span)?);
            }
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
    let storage_slots = state
        .storage
        .iter()
        .filter_map(|(id, storage)| {
            storage
                .dynamic_alignment
                .is_none()
                .then_some((*id, storage.slot))
        })
        .collect();
    builder.seal_all_blocks();
    backend::finalize_frontend(builder, frontend_config);
    Ok(FunctionDebugLayout { storage_slots })
}

struct FunctionState<'a, 'references, 'object> {
    module: &'a gir::FullModule,
    function: &'a gir::FullFunction,
    config: &'a EffectiveCompilationConfig,
    abi_plan: ccc_abi::VerifiedModuleAbiPlan<'a>,
    definition_plan: DefinitionAbi<'a>,
    references: &'references FunctionReferences<'object>,
    frontend_config: backend::FrontendConfig,
    blocks: HashMap<u32, ir::Block>,
    storage: HashMap<u32, StackStorage>,
    runtime_storage: HashMap<u32, StackSlot>,
    values: Vec<Option<ir::Value>>,
    sret: Option<ir::Value>,
    variadic_state: Option<ir::Value>,
    variadic_frame: Option<ir::Value>,
}

impl FunctionState<'_, '_, '_> {
    fn f80_support_call(
        &self,
        builder: &mut FunctionBuilder<'_>,
        opcode: i64,
        output: ir::Value,
        left: ir::Value,
        right: Option<ir::Value>,
    ) -> Result<(), CodegenError> {
        let helper = self.references.f80_support(builder)?;
        let frame = create_stack_backing(builder, 32, 16)?;
        zero_memory(builder, frame, 32)?;
        store_integer(builder, frame, 0, ir::types::I32, opcode)?;
        store_value(builder, frame, 8, output)?;
        store_value(builder, frame, 16, left)?;
        if let Some(right) = right {
            store_value(builder, frame, 24, right)?;
        }
        let call = builder.ins().call(helper, &[frame]);
        if !builder.inst_results(call).is_empty() {
            return Err(error(
                "x87 support helper unexpectedly returned a CLIF value",
            ));
        }
        Ok(())
    }

    fn lower_f80_binary(
        &self,
        builder: &mut FunctionBuilder<'_>,
        operator: gir::BinaryOperation,
        left: ir::Value,
        right: ir::Value,
        result_ty: QualifiedType,
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
            let slot = create_stack_backing(builder, 4, 4)?;
            zero_memory(builder, slot, 4)?;
            let opcode = match operator {
                // C equality operators use a quiet comparison. Relational
                // operators are signaling under the enabled Annex-F parity
                // contract, including for a quiet NaN operand.
                gir::BinaryOperation::Equal | gir::BinaryOperation::NotEqual => {
                    F80_OP_COMPARE_QUIET
                }
                gir::BinaryOperation::Less
                | gir::BinaryOperation::LessEqual
                | gir::BinaryOperation::Greater
                | gir::BinaryOperation::GreaterEqual => F80_OP_COMPARE_SIGNALING,
                _ => unreachable!(),
            };
            self.f80_support_call(builder, opcode, slot, left, Some(right))?;
            let ordering =
                builder
                    .ins()
                    .load(ir::types::I32, backend::empty_memory_flags(), slot, 0);
            let boolean = match operator {
                gir::BinaryOperation::Less => builder.ins().icmp_imm_s(IntCC::Equal, ordering, -1),
                gir::BinaryOperation::LessEqual => {
                    builder
                        .ins()
                        .icmp_imm_s(IntCC::SignedLessThanOrEqual, ordering, 0)
                }
                gir::BinaryOperation::Greater => {
                    builder.ins().icmp_imm_s(IntCC::Equal, ordering, 1)
                }
                gir::BinaryOperation::GreaterEqual => {
                    let nonnegative =
                        builder
                            .ins()
                            .icmp_imm_s(IntCC::SignedGreaterThanOrEqual, ordering, 0);
                    let ordered = builder.ins().icmp_imm_s(IntCC::NotEqual, ordering, 2);
                    builder.ins().band(nonnegative, ordered)
                }
                gir::BinaryOperation::Equal => builder.ins().icmp_imm_s(IntCC::Equal, ordering, 0),
                gir::BinaryOperation::NotEqual => {
                    builder.ins().icmp_imm_s(IntCC::NotEqual, ordering, 0)
                }
                _ => unreachable!(),
            };
            let destination = scalar_type(&self.module.types, result_ty, self.config)?;
            return Ok(coerce_integer(
                builder,
                boolean,
                builder.func.dfg.value_type(boolean),
                destination,
                false,
            ));
        }
        let opcode = match operator {
            gir::BinaryOperation::Add => F80_OP_ADD,
            gir::BinaryOperation::Subtract => F80_OP_SUBTRACT,
            gir::BinaryOperation::Multiply => F80_OP_MULTIPLY,
            gir::BinaryOperation::Divide => F80_OP_DIVIDE,
            _ => {
                return Err(error(format!(
                    "operator {operator:?} is invalid for x87 long double"
                )));
            }
        };
        let result = create_stack_backing(builder, 16, 16)?;
        zero_memory(builder, result, 16)?;
        self.f80_support_call(builder, opcode, result, left, Some(right))?;
        Ok(result)
    }

    fn lower_f80_unary(
        &self,
        builder: &mut FunctionBuilder<'_>,
        operator: gir::UnaryOperation,
        operand: ir::Value,
        result_ty: QualifiedType,
    ) -> Result<ir::Value, CodegenError> {
        match operator {
            gir::UnaryOperation::Plus => Ok(operand),
            gir::UnaryOperation::Negate => {
                let result = create_stack_backing(builder, 16, 16)?;
                zero_memory(builder, result, 16)?;
                self.f80_support_call(builder, F80_OP_NEGATE, result, operand, None)?;
                Ok(result)
            }
            gir::UnaryOperation::LogicalNot => {
                let zero = create_stack_backing(builder, 16, 16)?;
                zero_memory(builder, zero, 16)?;
                let slot = create_stack_backing(builder, 4, 4)?;
                zero_memory(builder, slot, 4)?;
                self.f80_support_call(builder, F80_OP_COMPARE_QUIET, slot, operand, Some(zero))?;
                let ordering =
                    builder
                        .ins()
                        .load(ir::types::I32, backend::empty_memory_flags(), slot, 0);
                let boolean = builder.ins().icmp_imm_s(IntCC::Equal, ordering, 0);
                let destination = scalar_type(&self.module.types, result_ty, self.config)?;
                Ok(coerce_integer(
                    builder,
                    boolean,
                    builder.func.dfg.value_type(boolean),
                    destination,
                    false,
                ))
            }
            gir::UnaryOperation::BitwiseNot => {
                Err(error("bitwise complement cannot be applied to long double"))
            }
        }
    }

    fn lower_f80_conversion(
        &self,
        builder: &mut FunctionBuilder<'_>,
        operand: ir::Value,
        kind: gir::ScalarConversion,
        from: QualifiedType,
        to: QualifiedType,
    ) -> Result<Option<ir::Value>, CodegenError> {
        if kind == gir::ScalarConversion::ToVoid {
            return Ok(None);
        }
        let from_f80 = is_x87_f80(&self.module.types, from, self.config);
        let to_f80 = is_x87_f80(&self.module.types, to, self.config);
        if from_f80 && to_f80 {
            return Ok(Some(operand));
        }
        if from_f80
            && (kind == gir::ScalarConversion::ToBoolean
                || self.module.types.builtin_type(to.ty) == Some(BuiltinType::Bool))
        {
            let zero = create_stack_backing(builder, 16, 16)?;
            zero_memory(builder, zero, 16)?;
            let slot = create_stack_backing(builder, 4, 4)?;
            zero_memory(builder, slot, 4)?;
            self.f80_support_call(builder, F80_OP_COMPARE_QUIET, slot, operand, Some(zero))?;
            let ordering =
                builder
                    .ins()
                    .load(ir::types::I32, backend::empty_memory_flags(), slot, 0);
            let boolean = builder.ins().icmp_imm_s(IntCC::NotEqual, ordering, 0);
            let destination = scalar_type(&self.module.types, to, self.config)?;
            return Ok(Some(coerce_integer(
                builder,
                boolean,
                builder.func.dfg.value_type(boolean),
                destination,
                false,
            )));
        }
        if to_f80 {
            let source_type = self.module.types.builtin_type(from.ty);
            let opcode = match source_type {
                Some(BuiltinType::Float16) => F80_OP_FROM_F32,
                Some(BuiltinType::Float) => F80_OP_FROM_F32,
                Some(BuiltinType::Double) => F80_OP_FROM_F64,
                Some(BuiltinType::Int128) => F80_OP_FROM_I128,
                Some(BuiltinType::UnsignedInt128) => F80_OP_FROM_U128,
                Some(builtin) if builtin.is_integer() => {
                    if is_signed(&self.module.types, from, self.config)? {
                        F80_OP_FROM_I64
                    } else {
                        F80_OP_FROM_U64
                    }
                }
                _ => return Err(error("invalid conversion to x87 long double")),
            };
            let operand = if source_type == Some(BuiltinType::Float16) {
                float16_to_f32(builder, operand)
            } else {
                operand
            };
            let source_ty = builder.func.dfg.value_type(operand);
            let staged_ty = match opcode {
                F80_OP_FROM_F32 => ir::types::F32,
                F80_OP_FROM_F64 => ir::types::F64,
                F80_OP_FROM_I128 | F80_OP_FROM_U128 => ir::types::I128,
                _ => ir::types::I64,
            };
            let staged_value = if source_ty == staged_ty {
                operand
            } else if staged_ty == ir::types::I64 {
                coerce_integer(
                    builder,
                    operand,
                    source_ty,
                    staged_ty,
                    is_signed(&self.module.types, from, self.config)?,
                )
            } else {
                return Err(error("invalid source carrier for x87 conversion"));
            };
            let source = create_stack_backing(
                builder,
                u64::from(staged_ty.bytes()),
                if staged_ty == ir::types::I128 { 16 } else { 8 },
            )?;
            builder
                .ins()
                .store(backend::empty_memory_flags(), staged_value, source, 0);
            let result = create_stack_backing(builder, 16, 16)?;
            zero_memory(builder, result, 16)?;
            self.f80_support_call(builder, opcode, result, source, None)?;
            return Ok(Some(result));
        }
        if from_f80 {
            let destination_builtin = self.module.types.builtin_type(to.ty);
            if destination_builtin == Some(BuiltinType::Float16) {
                return Ok(Some(f80_to_float16(builder, operand)));
            }
            let (opcode, stored_ty, stored_size) = match destination_builtin {
                Some(BuiltinType::Float) => (F80_OP_TO_F32, ir::types::F32, 4),
                Some(BuiltinType::Double) => (F80_OP_TO_F64, ir::types::F64, 8),
                Some(BuiltinType::Int128) => (F80_OP_TO_I128, ir::types::I128, 16),
                Some(BuiltinType::UnsignedInt128) => (F80_OP_TO_U128, ir::types::I128, 16),
                Some(builtin) if builtin.is_integer() => {
                    let opcode = if is_signed(&self.module.types, to, self.config)? {
                        F80_OP_TO_I64
                    } else {
                        F80_OP_TO_U64
                    };
                    (opcode, ir::types::I64, 8)
                }
                _ => return Err(error("invalid conversion from x87 long double")),
            };
            let output = create_stack_backing(
                builder,
                stored_size,
                if stored_ty == ir::types::I128 { 16 } else { 8 },
            )?;
            zero_memory(builder, output, stored_size)?;
            self.f80_support_call(builder, opcode, output, operand, None)?;
            let value = builder
                .ins()
                .load(stored_ty, backend::empty_memory_flags(), output, 0);
            let destination = scalar_type(&self.module.types, to, self.config)?;
            let value = if stored_ty.is_int() && stored_ty != destination {
                coerce_integer(
                    builder,
                    value,
                    stored_ty,
                    destination,
                    is_signed(&self.module.types, to, self.config)?,
                )
            } else {
                value
            };
            return Ok(Some(value));
        }
        Err(error(
            "x87 conversion did not contain an x87 operand or result",
        ))
    }

    fn initialize_runtime_storage(&self, builder: &mut FunctionBuilder<'_>) {
        if self.runtime_storage.is_empty() {
            return;
        }
        let zero = builder.ins().iconst(ir::types::I64, 0);
        let mut slots = self.runtime_storage.iter().collect::<Vec<_>>();
        slots.sort_unstable_by_key(|(storage, _)| **storage);
        for (_, slot) in slots {
            let state = builder.ins().stack_addr(ir::types::I64, *slot, 0);
            builder
                .ins()
                .store(backend::empty_memory_flags(), zero, state, 0);
            builder
                .ins()
                .store(backend::empty_memory_flags(), zero, state, 8);
        }
    }

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
                let value = *clif_values
                    .get(index as usize)
                    .ok_or_else(|| error("scalar ABI carrier is absent from function entry"))?;
                let expected = scalar_type(
                    &self.module.types,
                    self.function.parameters[source].ty,
                    self.config,
                )?;
                coerce_carrier_value(builder, value, expected, false)?
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
                        ccc_abi::NativePurpose::IndirectArgument => copy_memory(
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
                            builder.ins().store(
                                backend::empty_memory_flags(),
                                incoming_value,
                                destination,
                                0,
                            );
                        }
                        ccc_abi::NativePurpose::StructReturn => {
                            return Err(error("source parameter unexpectedly uses sret purpose"));
                        }
                        ccc_abi::NativePurpose::Padding => {
                            return Err(error("source parameter unexpectedly uses ABI padding"));
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
            let saved_sret = address_offset(
                builder,
                *frame,
                bridge_frame_layout(plan.abi_identity).entry_indirect_result,
            )?;
            self.sret = Some(builder.ins().load(
                ir::types::I64,
                backend::empty_memory_flags(),
                saved_sret,
                0,
            ));
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
            let value = if classified.passing == ccc_abi::PassingMode::Scalar
                && is_x87_f80(
                    &self.module.types,
                    QualifiedType::unqualified(classified.ty),
                    self.config,
                ) {
                let result = create_stack_backing(builder, 16, 16)?;
                zero_memory(builder, result, 16)?;
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
            } else if classified.passing == ccc_abi::PassingMode::Scalar {
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
                    backend::empty_memory_flags(),
                    source,
                    0,
                )
            } else {
                let padded = align_up_u64(classified.size, 8)?;
                let result = create_stack_backing(builder, padded, classified.align)?;
                zero_memory(builder, result, padded)?;
                if let Some(piece) = pieces.iter().find(|piece| piece.indirect) {
                    let slot =
                        variadic_parameter_piece_address(builder, *frame, plan, piece.location)?;
                    let source =
                        builder
                            .ins()
                            .load(ir::types::I64, backend::empty_memory_flags(), slot, 0);
                    copy_memory(
                        builder,
                        result,
                        source,
                        classified.size,
                        gir::MemoryAccess::default(),
                        gir::MemoryAccess::default(),
                    )?;
                    self.set_value(incoming, result)?;
                    continue;
                }
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
                    let reference = self.references.function_address(builder, function.0)?;
                    Ok(Some(builder.ins().func_addr(ir::types::I64, reference)))
                }
                I::AddressOfString { string } => {
                    let reference = self.references.string(builder, string.0)?;
                    Ok(Some(backend::materialize_symbol(
                        builder,
                        ir::types::I64,
                        reference,
                    )))
                }
                I::AddressOfStorage { storage } => {
                    let storage = self.storage.get(&storage.0).copied().ok_or_else(|| {
                    error(format!(
                        "storage {} is not an automatic stack object; static storage must use a data id",
                        storage.0
                    ))
                })?;
                    let address = builder.ins().stack_addr(ir::types::I64, storage.slot, 0);
                    let Some(alignment) = storage.dynamic_alignment else {
                        return Ok(Some(address));
                    };
                    let bias = builder.ins().iconst(ir::types::I64, (alignment - 1) as i64);
                    let biased = builder.ins().iadd(address, bias);
                    let mask = builder
                        .ins()
                        .iconst(ir::types::I64, alignment.wrapping_neg() as i64);
                    Ok(Some(builder.ins().band(biased, mask)))
                }
                I::RuntimeSize {
                    extents,
                    element,
                    constant_factor,
                } => Ok(Some(self.runtime_size(
                    builder,
                    extents,
                    *element,
                    *constant_factor,
                )?)),
                I::RuntimeSizedAllocate {
                    storage,
                    size,
                    element,
                    requested_alignment,
                } => Ok(Some(self.runtime_sized_allocate(
                    builder,
                    *storage,
                    *size,
                    *element,
                    *requested_alignment,
                )?)),
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
                I::RuntimePointerOffset {
                    base,
                    index,
                    element: _,
                    stride,
                    subtract,
                } => {
                    let index_value = self.value(*index)?;
                    let index_ty = builder.func.dfg.value_type(index_value);
                    let index = coerce_integer(
                        builder,
                        index_value,
                        index_ty,
                        ir::types::I64,
                        is_signed(&self.module.types, self.value_ty(*index)?, self.config)?,
                    );
                    let stride = self.value(*stride)?;
                    let displacement = builder.ins().imul(index, stride);
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
                I::RuntimePointerDifference {
                    left,
                    right,
                    stride,
                    ..
                } => {
                    let bytes = builder.ins().isub(self.value(*left)?, self.value(*right)?);
                    let stride = self.value(*stride)?;
                    let difference = builder.ins().sdiv(bytes, stride);
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
                } => {
                    if is_x87_f80(&self.module.types, *object, self.config) {
                        if access.atomic.is_some() {
                            return Err(CodegenError {
                                code: ATOMIC_ERROR,
                                message: "atomic x87 extended-precision loads are not enabled"
                                    .to_owned(),
                                span: None,
                            });
                        }
                        let result = create_stack_backing(builder, 16, 16)?;
                        zero_memory(builder, result, 16)?;
                        if access.volatile {
                            self.f80_support_call(
                                builder,
                                F80_OP_VOLATILE_LOAD,
                                result,
                                self.value(*address)?,
                                None,
                            )?;
                        } else {
                            copy_memory(
                                builder,
                                result,
                                self.value(*address)?,
                                16,
                                gir::MemoryAccess::default(),
                                *access,
                            )?;
                        }
                        Ok(Some(result))
                    } else {
                        Ok(Some(lower_load(
                            builder,
                            self.value(*address)?,
                            scalar_type(&self.module.types, *object, self.config)?,
                            *access,
                        )?))
                    }
                }
                I::Store {
                    address,
                    value,
                    object,
                    access,
                } => {
                    if is_x87_f80(&self.module.types, *object, self.config) {
                        if access.atomic.is_some() {
                            return Err(CodegenError {
                                code: ATOMIC_ERROR,
                                message: "atomic x87 extended-precision stores are not enabled"
                                    .to_owned(),
                                span: None,
                            });
                        }
                        if access.volatile {
                            self.f80_support_call(
                                builder,
                                F80_OP_VOLATILE_STORE,
                                self.value(*address)?,
                                self.value(*value)?,
                                None,
                            )?;
                        } else {
                            copy_memory(
                                builder,
                                self.value(*address)?,
                                self.value(*value)?,
                                16,
                                *access,
                                gir::MemoryAccess::default(),
                            )?;
                        }
                        return Ok(None);
                    }
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
                    let source = self.references.string(builder, string.0)?;
                    let source = backend::materialize_symbol(builder, ir::types::I64, source);
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
                } => {
                    if is_x87_f80(&self.module.types, *from, self.config)
                        || is_x87_f80(&self.module.types, *to, self.config)
                    {
                        self.lower_f80_conversion(builder, self.value(*operand)?, *kind, *from, *to)
                    } else {
                        lower_conversion(
                            builder,
                            &self.module.types,
                            self.value(*operand)?,
                            *kind,
                            *from,
                            *to,
                            self.config,
                            self.references,
                        )
                    }
                }
                I::Unary { operator, operand } => {
                    let operand_ty = self.value_ty(*operand)?;
                    let result_ty =
                        result_ty.ok_or_else(|| error("unary instruction has no result"))?;
                    if is_x87_f80(&self.module.types, operand_ty, self.config) {
                        Ok(Some(self.lower_f80_unary(
                            builder,
                            *operator,
                            self.value(*operand)?,
                            result_ty,
                        )?))
                    } else {
                        Ok(Some(lower_unary(
                            builder,
                            &self.module.types,
                            *operator,
                            self.value(*operand)?,
                            operand_ty,
                            result_ty,
                            self.config,
                        )?))
                    }
                }
                I::Binary {
                    operator,
                    left,
                    right,
                } => {
                    let operand_ty = self.value_ty(*left)?;
                    let result_ty =
                        result_ty.ok_or_else(|| error("binary instruction has no result"))?;
                    if is_x87_f80(&self.module.types, operand_ty, self.config) {
                        return Ok(Some(self.lower_f80_binary(
                            builder,
                            *operator,
                            self.value(*left)?,
                            self.value(*right)?,
                            result_ty,
                        )?));
                    }
                    let floating = is_float(&self.module.types, operand_ty);
                    let signed = if floating {
                        false
                    } else {
                        is_signed(&self.module.types, operand_ty, self.config)?
                    };
                    let result = scalar_type(&self.module.types, result_ty, self.config)?;
                    Ok(Some(lower_binary(
                        builder,
                        *operator,
                        self.value(*left)?,
                        self.value(*right)?,
                        floating,
                        signed,
                        result,
                        self.references,
                    )?))
                }
                I::IntegerIntrinsic { operation, operand } => {
                    let operand = self.value(*operand)?;
                    let value = match operation {
                        gir::IntegerIntrinsicOperation::ByteSwap64 => builder.ins().bswap(operand),
                        gir::IntegerIntrinsicOperation::CountLeadingZerosInt
                        | gir::IntegerIntrinsicOperation::CountLeadingZerosLong
                        | gir::IntegerIntrinsicOperation::CountLeadingZerosLongLong => {
                            builder.ins().clz(operand)
                        }
                        gir::IntegerIntrinsicOperation::CountTrailingZerosLongLong
                        | gir::IntegerIntrinsicOperation::CountTrailingZerosInt => {
                            builder.ins().ctz(operand)
                        }
                        gir::IntegerIntrinsicOperation::PopulationCountInt
                        | gir::IntegerIntrinsicOperation::PopulationCountLongLong => {
                            builder.ins().popcnt(operand)
                        }
                    };
                    let source = builder.func.dfg.value_type(value);
                    let destination = scalar_type(
                        &self.module.types,
                        result_ty.ok_or_else(|| error("integer intrinsic has no result"))?,
                        self.config,
                    )?;
                    Ok(Some(coerce_integer(
                        builder,
                        value,
                        source,
                        destination,
                        false,
                    )))
                }
                I::MemoryCopy {
                    destination,
                    source,
                    length,
                    overlap,
                } => {
                    let destination = self.value(*destination)?;
                    let source = self.value(*source)?;
                    let length = self.value(*length)?;
                    if *overlap {
                        builder.call_memmove(self.frontend_config, destination, source, length);
                    } else {
                        builder.call_memcpy(self.frontend_config, destination, source, length);
                    }
                    Ok(None)
                }
                I::MemorySet {
                    destination,
                    value,
                    length,
                } => {
                    let value = self.value(*value)?;
                    let value_ty = builder.func.dfg.value_type(value);
                    let value = coerce_integer(builder, value, value_ty, ir::types::I8, true);
                    builder.call_memset(
                        self.frontend_config,
                        self.value(*destination)?,
                        value,
                        self.value(*length)?,
                    );
                    Ok(None)
                }
                I::AtomicReadModifyWrite {
                    operation,
                    address,
                    operand,
                    object,
                    return_new,
                    order: gir::MemoryOrder::SequentiallyConsistent,
                } => {
                    let ty = atomic_scalar_type(&self.module.types, *object, self.config)?;
                    let operation = match operation {
                        gir::AtomicReadModifyWriteOperation::Add => ir::AtomicRmwOp::Add,
                        gir::AtomicReadModifyWriteOperation::Subtract => ir::AtomicRmwOp::Sub,
                        gir::AtomicReadModifyWriteOperation::BitwiseAnd => ir::AtomicRmwOp::And,
                        gir::AtomicReadModifyWriteOperation::BitwiseOr => ir::AtomicRmwOp::Or,
                        gir::AtomicReadModifyWriteOperation::BitwiseXor => ir::AtomicRmwOp::Xor,
                        gir::AtomicReadModifyWriteOperation::Exchange => ir::AtomicRmwOp::Xchg,
                    };
                    let operand = self.value(*operand)?;
                    let old = builder.ins().atomic_rmw(
                        ty,
                        backend::empty_memory_flags(),
                        operation,
                        self.value(*address)?,
                        operand,
                    );
                    let result = if *return_new {
                        match operation {
                            ir::AtomicRmwOp::Add => builder.ins().iadd(old, operand),
                            ir::AtomicRmwOp::Sub => builder.ins().isub(old, operand),
                            ir::AtomicRmwOp::And => builder.ins().band(old, operand),
                            ir::AtomicRmwOp::Or => builder.ins().bor(old, operand),
                            ir::AtomicRmwOp::Xor => builder.ins().bxor(old, operand),
                            _ => {
                                return Err(error(
                                    "atomic exchange cannot return a derived replacement value",
                                ));
                            }
                        }
                    } else {
                        old
                    };
                    Ok(Some(result))
                }
                I::AtomicCompareExchange {
                    address,
                    expected,
                    replacement,
                    object,
                    order: gir::MemoryOrder::SequentiallyConsistent,
                } => {
                    let _ = atomic_scalar_type(&self.module.types, *object, self.config)?;
                    Ok(Some(builder.ins().atomic_cas(
                        backend::empty_memory_flags(),
                        self.value(*address)?,
                        self.value(*expected)?,
                        self.value(*replacement)?,
                    )))
                }
                I::Prefetch {
                    address,
                    write: _,
                    locality: _,
                } => {
                    let _ = self.value(*address)?;
                    Ok(None)
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
                I::CompilerBarrier { memory } => {
                    if *memory {
                        // Cranelift does not expose a compiler-only memory
                        // barrier. Its fence is stronger and preserves the
                        // required ordering without weakening behavior.
                        builder.ins().fence();
                    }
                    Ok(None)
                }
                I::OpaqueScalar { operand } => {
                    // The retained IR operation prevents CCC-side folding.
                    // A fence keeps surrounding operations ordered; native
                    // helper materialization may later provide a stronger
                    // backend optimization barrier without changing the IR.
                    builder.ins().fence();
                    Ok(Some(self.value(*operand)?))
                }
                I::CodeLayoutHint(_) => Ok(None),
                I::X86Cpuid {
                    leaf,
                    subleaf,
                    eax,
                    ebx,
                    ecx,
                    edx,
                } => {
                    let helper = self.references.inline_cpuid_support(builder)?;
                    let subleaf = if let Some(subleaf) = subleaf {
                        self.value(*subleaf)?
                    } else {
                        builder.ins().iconst(ir::types::I32, 0)
                    };
                    let null = builder.ins().iconst(ir::types::I64, 0);
                    let output = |value: &Option<gir::ValueId>| -> Result<ir::Value, CodegenError> {
                        value.map_or(Ok(null), |value| self.value(value))
                    };
                    builder.ins().call(
                        helper,
                        &[
                            self.value(*leaf)?,
                            subleaf,
                            output(eax)?,
                            output(ebx)?,
                            output(ecx)?,
                            output(edx)?,
                        ],
                    );
                    Ok(None)
                }
                I::X86Rdtsc { low, high } => {
                    let helper = self.references.inline_rdtsc_support(builder)?;
                    builder
                        .ins()
                        .call(helper, &[self.value(*low)?, self.value(*high)?]);
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
                        va_list_size(self.config),
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
        let reference = self.references.global(builder, raw)?;
        if let Some(accessor) = reference.tls_accessor {
            let call = builder.ins().call(accessor, &[]);
            return builder
                .inst_results(call)
                .first()
                .copied()
                .ok_or_else(|| error("TLS address accessor returned no pointer"));
        }
        let value = reference
            .value
            .ok_or_else(|| error("non-TLS data reference has no global value"))?;
        Ok(backend::materialize_symbol(builder, ir::types::I64, value))
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
                let reference = self.references.function_address(builder, id.0)?;
                builder.ins().func_addr(ir::types::I64, reference)
            }
            gir::RelocationTarget::String(id) => {
                let reference = self.references.string(builder, id.0)?;
                backend::materialize_symbol(builder, ir::types::I64, reference)
            }
        };
        let addend = i64::try_from(addend)
            .map_err(|_| error("address constant addend does not fit in 64 bits"))?;
        Ok(if addend == 0 {
            address
        } else {
            builder.ins().iadd_imm_s(address, addend)
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
                        builder.ins().imul_imm_s(
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
            va_list_size(self.config),
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
        match self.config.target.abi {
            ccc_target::AbiIdentity::SysvAmd64Lp64 => {
                self.va_arg_sysv_amd64(builder, list, result, plan)?
            }
            ccc_target::AbiIdentity::Aapcs64Lp64 => {
                self.va_arg_aapcs64(builder, list, result, plan)?
            }
            ccc_target::AbiIdentity::RiscvLp64d | ccc_target::AbiIdentity::DarwinArm64 => {
                self.va_arg_cursor(builder, list, result, plan)?
            }
        }
        va_arg_result(builder, &self.module.types, requested, result, self.config)
    }

    fn va_arg_sysv_amd64(
        &self,
        builder: &mut FunctionBuilder<'_>,
        list: ir::Value,
        result: ir::Value,
        plan: &ccc_abi::VaArgPlan,
    ) -> Result<(), CodegenError> {
        if plan.classified.passing == ccc_abi::PassingMode::Memory
            || plan.classified.classes.iter().any(|class| {
                matches!(
                    class,
                    ccc_abi::AbiClass::X87
                        | ccc_abi::AbiClass::X87Up
                        | ccc_abi::AbiClass::ComplexX87
                )
            })
        {
            let overflow_address = address_offset(builder, list, 8)?;
            return self.va_arg_cursor(builder, overflow_address, result, plan);
        }
        let gp_offset_address = list;
        let fp_offset_address = address_offset(builder, list, 4)?;
        let gp_offset = builder.ins().load(
            ir::types::I32,
            backend::empty_memory_flags(),
            gp_offset_address,
            0,
        );
        let fp_offset = builder.ins().load(
            ir::types::I32,
            backend::empty_memory_flags(),
            fp_offset_address,
            0,
        );
        let gp_limit = 48u32
            .checked_sub(u32::from(plan.gp_slots) * 8)
            .ok_or_else(|| error("va_arg GP slot requirement exceeds the save area"))?;
        let fp_limit = 176u32
            .checked_sub(u32::from(plan.sse_slots) * 16)
            .ok_or_else(|| error("va_arg SSE slot requirement exceeds the save area"))?;
        let gp_available = if plan.gp_slots == 0 {
            builder.ins().iconst(ir::types::I8, 1)
        } else {
            builder.ins().icmp_imm_s(
                IntCC::UnsignedLessThanOrEqual,
                gp_offset,
                i64::from(gp_limit),
            )
        };
        let fp_available = if plan.sse_slots == 0 {
            builder.ins().iconst(ir::types::I8, 1)
        } else {
            builder.ins().icmp_imm_s(
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
        let save_area = builder.ins().load(
            ir::types::I64,
            backend::empty_memory_flags(),
            save_area_address,
            0,
        );
        let mut next_gp = gp_offset;
        let mut next_fp = fp_offset;
        for piece in &plan.classified.pieces {
            let source = match piece.class {
                ccc_abi::AbiClass::Integer => {
                    let offset = builder.ins().uextend(ir::types::I64, next_gp);
                    next_gp = builder.ins().iadd_imm_s(next_gp, 8);
                    builder.ins().iadd(save_area, offset)
                }
                ccc_abi::AbiClass::Sse | ccc_abi::AbiClass::SseUp => {
                    let offset = builder.ins().uextend(ir::types::I64, next_fp);
                    next_fp = builder.ins().iadd_imm_s(next_fp, 16);
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
                .store(backend::empty_memory_flags(), next_gp, gp_offset_address, 0);
        }
        if plan.sse_slots != 0 {
            builder
                .ins()
                .store(backend::empty_memory_flags(), next_fp, fp_offset_address, 0);
        }
        builder.ins().jump(merge_block, &[]);

        builder.switch_to_block(overflow_block);
        let overflow_address = address_offset(builder, list, 8)?;
        self.va_arg_cursor(builder, overflow_address, result, plan)?;
        builder.ins().jump(merge_block, &[]);

        builder.switch_to_block(merge_block);
        Ok(())
    }

    fn va_arg_aapcs64(
        &self,
        builder: &mut FunctionBuilder<'_>,
        list: ir::Value,
        result: ir::Value,
        plan: &ccc_abi::VaArgPlan,
    ) -> Result<(), CodegenError> {
        let gr_offset_address = address_offset(builder, list, 24)?;
        let vr_offset_address = address_offset(builder, list, 28)?;
        let mut gr_offset = builder.ins().load(
            ir::types::I32,
            backend::empty_memory_flags(),
            gr_offset_address,
            0,
        );
        let vr_offset = builder.ins().load(
            ir::types::I32,
            backend::empty_memory_flags(),
            vr_offset_address,
            0,
        );
        if !plan.indirect && plan.gp_slots != 0 && plan.sse_slots == 0 && plan.overflow_align >= 16
        {
            let advanced = builder.ins().iadd_imm_s(gr_offset, 15);
            gr_offset = builder.ins().band_imm_u(advanced, -16);
        }
        let gr_available = if plan.gp_slots == 0 {
            builder.ins().iconst(ir::types::I8, 1)
        } else {
            builder.ins().icmp_imm_s(
                IntCC::SignedLessThanOrEqual,
                gr_offset,
                -i64::from(plan.gp_slots) * 8,
            )
        };
        let vr_available = if plan.sse_slots == 0 {
            builder.ins().iconst(ir::types::I8, 1)
        } else {
            builder.ins().icmp_imm_s(
                IntCC::SignedLessThanOrEqual,
                vr_offset,
                -i64::from(plan.sse_slots) * 16,
            )
        };
        let registers_available = builder.ins().band(gr_available, vr_available);
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
        let gr_top_address = address_offset(builder, list, 8)?;
        let vr_top_address = address_offset(builder, list, 16)?;
        let gr_top = builder.ins().load(
            ir::types::I64,
            backend::empty_memory_flags(),
            gr_top_address,
            0,
        );
        let vr_top = builder.ins().load(
            ir::types::I64,
            backend::empty_memory_flags(),
            vr_top_address,
            0,
        );
        let mut next_gr = gr_offset;
        let mut next_vr = vr_offset;
        if plan.indirect {
            let offset = builder.ins().sextend(ir::types::I64, next_gr);
            let slot = builder.ins().iadd(gr_top, offset);
            let source = builder
                .ins()
                .load(ir::types::I64, backend::empty_memory_flags(), slot, 0);
            copy_memory(
                builder,
                result,
                source,
                plan.result_size,
                gir::MemoryAccess::default(),
                gir::MemoryAccess::default(),
            )?;
            next_gr = builder.ins().iadd_imm_s(next_gr, 8);
        } else {
            for piece in &plan.classified.pieces {
                let source = match piece.class {
                    ccc_abi::AbiClass::Integer => {
                        let offset = builder.ins().sextend(ir::types::I64, next_gr);
                        next_gr = builder.ins().iadd_imm_s(next_gr, 8);
                        builder.ins().iadd(gr_top, offset)
                    }
                    ccc_abi::AbiClass::Sse => {
                        let offset = builder.ins().sextend(ir::types::I64, next_vr);
                        next_vr = builder.ins().iadd_imm_s(next_vr, 16);
                        builder.ins().iadd(vr_top, offset)
                    }
                    class => {
                        return Err(error(format!(
                            "AAPCS64 va_arg register path contains unsupported class {class:?}"
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
        }
        if plan.gp_slots != 0 {
            builder
                .ins()
                .store(backend::empty_memory_flags(), next_gr, gr_offset_address, 0);
        }
        if plan.sse_slots != 0 {
            builder
                .ins()
                .store(backend::empty_memory_flags(), next_vr, vr_offset_address, 0);
        }
        builder.ins().jump(merge_block, &[]);

        builder.switch_to_block(overflow_block);
        // AAPCS C.13 and C.14 exhaust the applicable register bank when an
        // argument cannot fit wholly. Later va_arg operations must therefore
        // continue in the overflow area rather than reusing a trailing slot.
        if plan.gp_slots != 0 {
            let exhausted = builder.ins().iconst(ir::types::I32, 0);
            builder.ins().store(
                backend::empty_memory_flags(),
                exhausted,
                gr_offset_address,
                0,
            );
        }
        if plan.sse_slots != 0 {
            let exhausted = builder.ins().iconst(ir::types::I32, 0);
            builder.ins().store(
                backend::empty_memory_flags(),
                exhausted,
                vr_offset_address,
                0,
            );
        }
        self.va_arg_cursor(builder, list, result, plan)?;
        builder.ins().jump(merge_block, &[]);

        builder.switch_to_block(merge_block);
        Ok(())
    }

    fn va_arg_cursor(
        &self,
        builder: &mut FunctionBuilder<'_>,
        cursor_address: ir::Value,
        result: ir::Value,
        plan: &ccc_abi::VaArgPlan,
    ) -> Result<(), CodegenError> {
        let cursor = builder.ins().load(
            ir::types::I64,
            backend::empty_memory_flags(),
            cursor_address,
            0,
        );
        let aligned = if plan.overflow_align <= 1 {
            cursor
        } else {
            let added = builder.ins().iadd_imm_s(
                cursor,
                i64::try_from(plan.overflow_align - 1)
                    .map_err(|_| error("va_arg overflow alignment is too large"))?,
            );
            builder.ins().band_imm_u(
                added,
                -i64::try_from(plan.overflow_align)
                    .map_err(|_| error("va_arg overflow alignment is too large"))?,
            )
        };
        let source = if plan.indirect {
            builder
                .ins()
                .load(ir::types::I64, backend::empty_memory_flags(), aligned, 0)
        } else {
            aligned
        };
        copy_memory(
            builder,
            result,
            source,
            plan.result_size,
            gir::MemoryAccess::default(),
            gir::MemoryAccess::default(),
        )?;
        let next = builder.ins().iadd_imm_s(
            aligned,
            i64::try_from(plan.overflow_size)
                .map_err(|_| error("va_arg overflow size is too large"))?,
        );
        builder
            .ins()
            .store(backend::empty_memory_flags(), next, cursor_address, 0);
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
        let unit = builder
            .ins()
            .load(storage_ty, backend::empty_memory_flags(), address, 0);
        let shifted = if descriptor.bit_offset == 0 {
            unit
        } else {
            builder
                .ins()
                .ushr_imm_u(unit, i64::from(descriptor.bit_offset))
        };
        let masked = integer_and_mask(
            builder,
            shifted,
            storage_ty,
            low_mask_u128(descriptor.width),
        );
        let normalized = if descriptor.signed && descriptor.width != 0 {
            let shift = storage_ty.bits() - descriptor.width;
            if shift == 0 {
                masked
            } else {
                let shifted = builder.ins().ishl_imm_u(masked, i64::from(shift));
                builder.ins().sshr_imm_u(shifted, i64::from(shift))
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
        let old = builder
            .ins()
            .load(storage_ty, backend::empty_memory_flags(), address, 0);
        let value_ty = builder.func.dfg.value_type(value);
        let value = coerce_integer(builder, value, value_ty, storage_ty, descriptor.signed);
        let value_mask = low_mask_u128(descriptor.width);
        let field_mask = value_mask.checked_shl(descriptor.bit_offset).unwrap_or(0);
        let retained = integer_and_mask(builder, old, storage_ty, !field_mask);
        let value = integer_and_mask(builder, value, storage_ty, value_mask);
        let value = if descriptor.bit_offset == 0 {
            value
        } else {
            builder
                .ins()
                .ishl_imm_u(value, i64::from(descriptor.bit_offset))
        };
        let combined = builder.ins().bor(retained, value);
        builder
            .ins()
            .store(backend::empty_memory_flags(), combined, address, 0);
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
        match boundary {
            ccc_abi::BoundaryPlan::Native(plan) => {
                let (arguments, result_storage) =
                    self.marshal_native_call_arguments(builder, plan, arguments)?;
                let reference = self.references.direct_function(builder, function)?;
                let call = builder.ins().call(reference, &arguments);
                self.finish_native_call(builder, call, plan, result_storage)
            }
            ccc_abi::BoundaryPlan::Bridge(plan) => {
                let reference = self.references.function_address(builder, function)?;
                let target = builder.ins().func_addr(ir::types::I64, reference);
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
        let helper = self.references.call_helper(builder)?;
        let layout = bridge_frame_layout(plan.abi_identity);
        let frame_size = layout
            .call_fixed_size
            .checked_add(u64::from(plan.stack_size))
            .ok_or_else(|| error("variadic call frame size overflow"))?;
        let frame = create_stack_backing(builder, frame_size, 16)?;
        zero_memory(builder, frame, frame_size)?;
        if plan.abi_identity == ccc_target::AbiIdentity::RiscvLp64d {
            // LP64D requires narrower values in an FLEN=64 register to be
            // NaN-boxed. The helper restores every slot with `fld`, so seed
            // the high half of each slot before width-specific F32 stores.
            for index in 0..8_u64 {
                store_integer(
                    builder,
                    frame,
                    layout.call_float_arguments + index * 16 + 4,
                    ir::types::I32,
                    -1,
                )?;
            }
        }
        store_integer(builder, frame, 0, ir::types::I32, 0x4642_4343)?;
        store_integer(builder, frame, 4, ir::types::I16, 2)?;
        store_integer(
            builder,
            frame,
            6,
            ir::types::I16,
            if plan.abi_identity == ccc_target::AbiIdentity::SysvAmd64Lp64 {
                32
            } else {
                48
            },
        )?;
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
        let xmm_results = plan
            .result_pieces
            .iter()
            .filter(|piece| {
                matches!(
                    piece.piece.class,
                    ccc_abi::AbiClass::Sse | ccc_abi::AbiClass::SseUp
                )
            })
            .count();
        store_integer(builder, frame, 27, ir::types::I8, gp_results as i64)?;
        store_integer(builder, frame, 28, ir::types::I8, xmm_results as i64)?;
        let x87_result = plan
            .result_pieces
            .iter()
            .any(|piece| piece.piece.class == ccc_abi::AbiClass::X87);
        store_integer(builder, frame, 29, ir::types::I8, i64::from(x87_result))?;

        let result_storage = if plan.hidden_return {
            let result = create_stack_backing(builder, plan.result.size, plan.result.align)?;
            zero_memory(builder, result, plan.result.size)?;
            store_value(builder, frame, layout.indirect_result, result)?;
            Some(result)
        } else {
            None
        };
        let mut indirect_stages = HashMap::<u32, ir::Value>::new();
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
            let destination = bridge_argument_piece_address(builder, frame, plan, piece.location)?;
            if piece.indirect {
                let stage = if let Some(stage) = indirect_stages.get(&source_index) {
                    *stage
                } else {
                    let stage = create_stack_backing(builder, classified.size, classified.align)?;
                    copy_memory(
                        builder,
                        stage,
                        self.value(argument_id)?,
                        classified.size,
                        gir::MemoryAccess::default(),
                        gir::MemoryAccess::default(),
                    )?;
                    indirect_stages.insert(source_index, stage);
                    stage
                };
                store_value(builder, destination, 0, stage)?;
                continue;
            }
            if classified.passing == ccc_abi::PassingMode::Scalar
                && is_x87_f80(
                    &self.module.types,
                    QualifiedType::unqualified(classified.ty),
                    self.config,
                )
            {
                let source = address_offset(builder, self.value(argument_id)?, piece.piece.offset)?;
                copy_memory(
                    builder,
                    destination,
                    source,
                    u64::from(piece.piece.valid_bytes),
                    gir::MemoryAccess::default(),
                    gir::MemoryAccess::default(),
                )?;
            } else if classified.passing == ccc_abi::PassingMode::Scalar {
                let mut value = self.value(argument_id)?;
                let value_type = builder.func.dfg.value_type(value);
                if value_type == ir::types::I128 {
                    let (low, high) = builder.ins().isplit(value);
                    let piece_value = match piece.piece.index {
                        0 => low,
                        1 => high,
                        _ => {
                            return Err(error(
                                "wide scalar bridge has an invalid physical piece index",
                            ));
                        }
                    };
                    store_value(builder, destination, 0, piece_value)?;
                    continue;
                }
                if piece.piece.class == ccc_abi::AbiClass::Integer
                    && value_type.is_int()
                    && value_type.bits() < 32
                {
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
                let source = bridge_result_piece_address(builder, frame, plan, piece.location)?;
                if is_x87_f80(
                    &self.module.types,
                    QualifiedType::unqualified(plan.result.ty),
                    self.config,
                ) {
                    let result = create_stack_backing(builder, 16, 16)?;
                    zero_memory(builder, result, 16)?;
                    copy_memory(
                        builder,
                        result,
                        source,
                        10,
                        gir::MemoryAccess::default(),
                        gir::MemoryAccess::default(),
                    )?;
                    Ok(Some(result))
                } else {
                    Ok(Some(builder.ins().load(
                        scalar_type(
                            &self.module.types,
                            QualifiedType::unqualified(plan.result.ty),
                            self.config,
                        )?,
                        backend::empty_memory_flags(),
                        source,
                        0,
                    )))
                }
            }
            ccc_abi::PassingMode::Registers => {
                let result = create_stack_backing(builder, plan.result.size, plan.result.align)?;
                zero_memory(builder, result, plan.result.size)?;
                for piece in &plan.result_pieces {
                    let source = bridge_result_piece_address(builder, frame, plan, piece.location)?;
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
            if carrier.purpose == ccc_abi::NativePurpose::Padding {
                lowered.push(zero_carrier_value(builder, carrier.carrier)?);
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
                let value = self.value(source_value)?;
                lowered.push(coerce_carrier_value(
                    builder,
                    value,
                    native_carrier_type(carrier.carrier),
                    is_signed(
                        &self.module.types,
                        self.value_ty(source_value)?,
                        self.config,
                    )
                    .unwrap_or(false),
                )?);
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
                ccc_abi::NativePurpose::IndirectArgument => lowered.push(stage),
                ccc_abi::NativePurpose::Normal => {
                    let address = address_offset(builder, stage, carrier.source_offset)?;
                    lowered.push(builder.ins().load(
                        native_carrier_type(carrier.carrier),
                        backend::empty_memory_flags(),
                        address,
                        0,
                    ));
                }
                ccc_abi::NativePurpose::StructReturn | ccc_abi::NativePurpose::Padding => {
                    unreachable!()
                }
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
            ccc_abi::NativeResultPlan::Scalar { ty, carrier_index } => {
                let value = call_result(builder, call, true)?;
                let Some(value) = value else {
                    return Err(error("scalar call has no result value"));
                };
                let carrier = plan
                    .clif_results
                    .get(*carrier_index as usize)
                    .ok_or_else(|| error("scalar result carrier index is invalid"))?;
                let expected = scalar_type(
                    &self.module.types,
                    QualifiedType::unqualified(*ty),
                    self.config,
                )?;
                let value = coerce_carrier_value(
                    builder,
                    value,
                    expected,
                    matches!(carrier.extension, ccc_abi::IntegerExtension::Signed),
                )?;
                Ok(Some(value))
            }
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
                    builder
                        .ins()
                        .store(backend::empty_memory_flags(), value, destination, 0);
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

    fn runtime_sized_allocate(
        &self,
        builder: &mut FunctionBuilder<'_>,
        storage: gir::StorageId,
        size: gir::ValueId,
        element: QualifiedType,
        requested_alignment: Option<u64>,
    ) -> Result<ir::Value, CodegenError> {
        let slot = self
            .runtime_storage
            .get(&storage.0)
            .copied()
            .ok_or_else(|| {
                error(format!(
                    "runtime allocation references unavailable storage {}",
                    storage.0
                ))
            })?;
        let realloc = self.references.runtime_realloc(builder)?;
        let layout = object_layout(&self.module.types, element, self.config)?;
        const HOSTED_VLA_MINIMUM_ALIGNMENT: u64 = 16;
        let alignment = requested_alignment
            .map_or(layout.align.max(HOSTED_VLA_MINIMUM_ALIGNMENT), |value| {
                value.max(layout.align).max(HOSTED_VLA_MINIMUM_ALIGNMENT)
            });
        if alignment == 0 || !alignment.is_power_of_two() {
            return Err(error("variable-length array has an invalid alignment"));
        }

        let trap = TrapCode::unwrap_user(2);
        let size = self.value(size)?;
        let padding = builder.ins().iconst(
            ir::types::I64,
            i64::try_from(alignment - 1)
                .map_err(|_| error("variable-length array alignment exceeds size_t"))?,
        );
        let required = builder.ins().iadd(size, padding);
        let overflow = builder.ins().icmp(IntCC::UnsignedLessThan, required, size);
        builder.ins().trapnz(overflow, trap);

        let state = builder.ins().stack_addr(ir::types::I64, slot, 0);
        let base = builder
            .ins()
            .load(ir::types::I64, backend::empty_memory_flags(), state, 0);
        let capacity = builder
            .ins()
            .load(ir::types::I64, backend::empty_memory_flags(), state, 8);
        let grow = builder.create_block();
        let ready = builder.create_block();
        builder.append_block_param(ready, ir::types::I64);
        let needs_grow = builder
            .ins()
            .icmp(IntCC::UnsignedGreaterThan, required, capacity);
        builder
            .ins()
            .brif(needs_grow, grow, &[], ready, &[base.into()]);

        builder.seal_block(grow);
        builder.switch_to_block(grow);
        let call = builder.ins().call(realloc, &[base, required]);
        let allocated = *builder
            .inst_results(call)
            .first()
            .ok_or_else(|| error("realloc did not return a pointer"))?;
        builder.ins().trapz(allocated, trap);
        builder
            .ins()
            .store(backend::empty_memory_flags(), allocated, state, 0);
        builder
            .ins()
            .store(backend::empty_memory_flags(), required, state, 8);
        builder.ins().jump(ready, &[allocated.into()]);

        builder.seal_block(ready);
        builder.switch_to_block(ready);
        let base = builder.block_params(ready)[0];
        let biased = builder.ins().iadd(base, padding);
        let mask = builder
            .ins()
            .iconst(ir::types::I64, alignment.wrapping_neg() as i64);
        Ok(builder.ins().band(biased, mask))
    }

    fn runtime_size(
        &self,
        builder: &mut FunctionBuilder<'_>,
        extents: &[gir::ValueId],
        element: QualifiedType,
        constant_factor: u64,
    ) -> Result<ir::Value, CodegenError> {
        let layout = object_layout(&self.module.types, element, self.config)?;
        let fixed_size = layout
            .size
            .checked_mul(constant_factor)
            .ok_or_else(|| error("runtime size constant overflow"))?;
        if fixed_size == 0 {
            return Err(error("runtime size has a zero-sized element"));
        }
        let trap = TrapCode::unwrap_user(2);
        // `iconst` takes a signed immediate but preserves its two's-complement
        // bit pattern. Every u64 value is therefore representable as size_t.
        let mut size = builder.ins().iconst(ir::types::I64, fixed_size as i64);
        for extent in extents {
            let source = self.value(*extent)?;
            let source_ty = builder.func.dfg.value_type(source);
            let signed = is_signed(&self.module.types, self.value_ty(*extent)?, self.config)?;
            let invalid = if signed {
                builder
                    .ins()
                    .icmp_imm_s(IntCC::SignedLessThanOrEqual, source, 0)
            } else {
                builder.ins().icmp_imm_s(IntCC::Equal, source, 0)
            };
            builder.ins().trapnz(invalid, trap);
            let value = if source_ty.bits() > ir::types::I64.bits() {
                let narrowed = builder.ins().ireduce(ir::types::I64, source);
                let restored = builder.ins().uextend(source_ty, narrowed);
                let truncated = builder.ins().icmp(IntCC::NotEqual, restored, source);
                builder.ins().trapnz(truncated, trap);
                narrowed
            } else {
                coerce_integer(builder, source, source_ty, ir::types::I64, signed)
            };
            let product = builder.ins().imul(size, value);
            let recovered = builder.ins().udiv(product, value);
            let overflow = builder.ins().icmp(IntCC::NotEqual, recovered, size);
            builder.ins().trapnz(overflow, trap);
            size = product;
        }
        Ok(size)
    }

    fn release_runtime_storage(
        &self,
        builder: &mut FunctionBuilder<'_>,
    ) -> Result<(), CodegenError> {
        if self.runtime_storage.is_empty() {
            return Ok(());
        }
        let free = self.references.runtime_free(builder)?;
        let mut slots = self.runtime_storage.iter().collect::<Vec<_>>();
        slots.sort_unstable_by_key(|(storage, _)| **storage);
        for (_, slot) in slots {
            let state = builder.ins().stack_addr(ir::types::I64, *slot, 0);
            let base = builder
                .ins()
                .load(ir::types::I64, backend::empty_memory_flags(), state, 0);
            builder.ins().call(free, &[base]);
        }
        Ok(())
    }

    fn lower_native_return(
        &self,
        builder: &mut FunctionBuilder<'_>,
        value: Option<gir::ValueId>,
        plan: &ccc_abi::NativeBoundaryPlan,
    ) -> Result<(), CodegenError> {
        match (&plan.result, value) {
            (ccc_abi::NativeResultPlan::Void, None) => {
                self.release_runtime_storage(builder)?;
                builder.ins().return_(&[]);
            }
            (ccc_abi::NativeResultPlan::Scalar { carrier_index, .. }, Some(value)) => {
                let value = self.value(value)?;
                let carrier = plan
                    .clif_results
                    .get(*carrier_index as usize)
                    .ok_or_else(|| error("scalar result carrier index is invalid"))?;
                let value = coerce_carrier_value(
                    builder,
                    value,
                    native_carrier_type(carrier.carrier),
                    matches!(carrier.extension, ccc_abi::IntegerExtension::Signed),
                )?;
                self.release_runtime_storage(builder)?;
                builder.ins().return_(&[value]);
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
                        backend::empty_memory_flags(),
                        source,
                        0,
                    ));
                }
                self.release_runtime_storage(builder)?;
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
                self.release_runtime_storage(builder)?;
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
                let destination =
                    variadic_result_piece_address(builder, frame, plan, piece.location)?;
                if is_x87_f80(
                    &self.module.types,
                    QualifiedType::unqualified(plan.result.ty),
                    self.config,
                ) {
                    copy_memory(
                        builder,
                        destination,
                        self.value(value)?,
                        10,
                        gir::MemoryAccess::default(),
                        gir::MemoryAccess::default(),
                    )?;
                } else {
                    builder.ins().store(
                        backend::empty_memory_flags(),
                        self.value(value)?,
                        destination,
                        0,
                    );
                }
            }
            (ccc_abi::PassingMode::Registers, Some(value)) => {
                let source = self.value(value)?;
                for piece in &plan.result_pieces {
                    let piece_source = address_offset(builder, source, piece.piece.offset)?;
                    let destination =
                        variadic_result_piece_address(builder, frame, plan, piece.location)?;
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
        self.release_runtime_storage(builder)?;
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
                    let constant = if selector_ty == ir::types::I128 {
                        i128_constant(builder, case.value)
                    } else {
                        builder.ins().iconst(selector_ty, case.value as u64 as i64)
                    };
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
            gir::FullTerminator::IndirectBranch { selector, targets } => {
                let selector = self.value(*selector)?;
                let selector_ty = builder.func.dfg.value_type(selector);
                if !selector_ty.is_int() {
                    return Err(error(
                        "computed goto selector is not represented as an integer",
                    ));
                }
                let index = builder.ins().iadd_imm_s(selector, -1);
                let trap = builder.create_block();
                if selector_ty.bits() > 32 {
                    let dispatch = builder.create_block();
                    let out_of_range = builder.ins().icmp_imm_s(
                        IntCC::UnsignedGreaterThan,
                        index,
                        i64::from(u32::MAX),
                    );
                    builder.ins().brif(out_of_range, trap, &[], dispatch, &[]);
                    builder.seal_block(dispatch);
                    builder.switch_to_block(dispatch);
                }
                let index = match selector_ty.bits() {
                    bits if bits > 32 => builder.ins().ireduce(ir::types::I32, index),
                    bits if bits < 32 => builder.ins().uextend(ir::types::I32, index),
                    _ => index,
                };
                let default_call = builder.func.dfg.block_call(trap, &[]);
                let mut target_calls = Vec::with_capacity(targets.len());
                for target in targets {
                    let block = self.block(target.target.0)?;
                    target_calls.push(builder.func.dfg.block_call(block, &[]));
                }
                let table =
                    builder.create_jump_table(ir::JumpTableData::new(default_call, &target_calls));
                builder.ins().br_table(index, table);
                builder.seal_block(trap);
                builder.switch_to_block(trap);
                builder.ins().trap(TrapCode::unwrap_user(1));
                Ok(())
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
        ccc_abi::AbiCarrier::I128 => ir::types::I128,
        ccc_abi::AbiCarrier::F16 => ir::types::F16,
        ccc_abi::AbiCarrier::F32 => ir::types::F32,
        ccc_abi::AbiCarrier::F64 => ir::types::F64,
        ccc_abi::AbiCarrier::V32 => ir::types::I8X4,
        ccc_abi::AbiCarrier::V64 => ir::types::I8X8,
    }
}

fn zero_carrier_value(
    builder: &mut FunctionBuilder<'_>,
    carrier: ccc_abi::AbiCarrier,
) -> Result<ir::Value, CodegenError> {
    Ok(match carrier {
        ccc_abi::AbiCarrier::I8
        | ccc_abi::AbiCarrier::I16
        | ccc_abi::AbiCarrier::I32
        | ccc_abi::AbiCarrier::I64
        | ccc_abi::AbiCarrier::I128 => builder.ins().iconst(native_carrier_type(carrier), 0),
        ccc_abi::AbiCarrier::F16 => builder.ins().f16const(Ieee16::with_bits(0)),
        ccc_abi::AbiCarrier::F32 => builder.ins().f32const(0.0),
        ccc_abi::AbiCarrier::F64 => builder.ins().f64const(0.0),
        ccc_abi::AbiCarrier::V32 | ccc_abi::AbiCarrier::V64 => {
            return Err(error("vector ABI carriers cannot be synthetic padding"));
        }
    })
}

fn va_list_size(config: &EffectiveCompilationConfig) -> u64 {
    match config.target.abi {
        ccc_target::AbiIdentity::SysvAmd64Lp64 => 24,
        ccc_target::AbiIdentity::Aapcs64Lp64 => 32,
        ccc_target::AbiIdentity::RiscvLp64d | ccc_target::AbiIdentity::DarwinArm64 => 8,
    }
}

fn coerce_carrier_value(
    builder: &mut FunctionBuilder<'_>,
    value: ir::Value,
    target: ir::Type,
    signed: bool,
) -> Result<ir::Value, CodegenError> {
    let source = builder.func.dfg.value_type(value);
    if source == target {
        return Ok(value);
    }
    if source.bits() == target.bits()
        && ((source.is_int() && target.is_float()) || (source.is_float() && target.is_int()))
    {
        return Ok(builder
            .ins()
            .bitcast(target, backend::empty_memory_flags(), value));
    }
    if source.is_int() && target.is_int() {
        return Ok(coerce_integer(builder, value, source, target, signed));
    }
    Err(error(format!(
        "cannot coerce native ABI carrier from {source} to {target}"
    )))
}

#[derive(Clone, Copy)]
struct BridgeFrameLayout {
    call_fixed_size: u64,
    call_integer_arguments: u64,
    call_float_arguments: u64,
    entry_integer_arguments: u64,
    entry_float_arguments: u64,
    integer_results: u64,
    float_results: u64,
    x87_result: u64,
    indirect_result: u64,
    entry_indirect_result: u64,
}

fn bridge_frame_layout(abi: ccc_target::AbiIdentity) -> BridgeFrameLayout {
    match abi {
        ccc_target::AbiIdentity::SysvAmd64Lp64 => BridgeFrameLayout {
            call_fixed_size: 272,
            call_integer_arguments: 32,
            call_float_arguments: 80,
            entry_integer_arguments: 32,
            entry_float_arguments: 80,
            integer_results: 208,
            float_results: 224,
            x87_result: 256,
            indirect_result: 32,
            entry_indirect_result: 32,
        },
        ccc_target::AbiIdentity::Aapcs64Lp64 | ccc_target::AbiIdentity::DarwinArm64 => {
            BridgeFrameLayout {
                call_fixed_size: 320,
                call_integer_arguments: 48,
                call_float_arguments: 112,
                entry_integer_arguments: 48,
                entry_float_arguments: 112,
                integer_results: 240,
                float_results: 256,
                x87_result: 0,
                indirect_result: 32,
                entry_indirect_result: 40,
            }
        }
        ccc_target::AbiIdentity::RiscvLp64d => BridgeFrameLayout {
            call_fixed_size: 320,
            call_integer_arguments: 48,
            call_float_arguments: 112,
            // The entry's integer save area ends exactly at the incoming
            // stack argument area, making the public pointer va_list cursor
            // contiguous across saved a-registers and caller stack slots.
            entry_integer_arguments: 448,
            entry_float_arguments: 112,
            integer_results: 240,
            float_results: 256,
            x87_result: 0,
            indirect_result: 48,
            entry_indirect_result: 448,
        },
    }
}

fn variadic_parameter_piece_address(
    builder: &mut FunctionBuilder<'_>,
    frame: ir::Value,
    plan: &ccc_abi::BridgeBoundaryPlan,
    location: ccc_abi::BridgeLocation,
) -> Result<ir::Value, CodegenError> {
    let layout = bridge_frame_layout(plan.abi_identity);
    match location {
        ccc_abi::BridgeLocation::Register(register) => address_offset(
            builder,
            frame,
            match register.bank {
                ccc_abi::RegisterBank::Integer => {
                    layout.entry_integer_arguments + u64::from(register.index) * 8
                }
                ccc_abi::RegisterBank::Float => {
                    layout.entry_float_arguments + u64::from(register.index) * 16
                }
                ccc_abi::RegisterBank::X87 => {
                    return Err(error("x87 is not an incoming register bank"));
                }
            },
        ),
        ccc_abi::BridgeLocation::Stack { offset } => {
            let overflow_slot = address_offset(builder, frame, 16)?;
            let overflow = builder.ins().load(
                ir::types::I64,
                backend::empty_memory_flags(),
                overflow_slot,
                0,
            );
            let fixed_stack_base = if plan.overflow_arg_offset == 0 {
                overflow
            } else {
                builder
                    .ins()
                    .iadd_imm_s(overflow, -i64::from(plan.overflow_arg_offset))
            };
            address_offset(builder, fixed_stack_base, u64::from(offset))
        }
    }
}

fn variadic_result_piece_address(
    builder: &mut FunctionBuilder<'_>,
    frame: ir::Value,
    plan: &ccc_abi::BridgeBoundaryPlan,
    location: ccc_abi::BridgeLocation,
) -> Result<ir::Value, CodegenError> {
    let register = match location {
        ccc_abi::BridgeLocation::Register(register)
            if match register.bank {
                ccc_abi::RegisterBank::Integer => register.index < 2,
                ccc_abi::RegisterBank::Float => register.index < 4,
                ccc_abi::RegisterBank::X87 => register.index == 0,
            } =>
        {
            register
        }
        _ => return Err(error("unsupported variadic result bridge location")),
    };
    // Result banks occupy identical offsets in call and entry frames.
    let layout = bridge_frame_layout(plan.abi_identity);
    let offset = match register.bank {
        ccc_abi::RegisterBank::Integer => layout.integer_results + u64::from(register.index) * 8,
        ccc_abi::RegisterBank::Float => layout.float_results + u64::from(register.index) * 16,
        ccc_abi::RegisterBank::X87 => layout.x87_result,
    };
    address_offset(builder, frame, offset)
}

fn bridge_argument_piece_address(
    builder: &mut FunctionBuilder<'_>,
    frame: ir::Value,
    plan: &ccc_abi::BridgeBoundaryPlan,
    location: ccc_abi::BridgeLocation,
) -> Result<ir::Value, CodegenError> {
    let layout = bridge_frame_layout(plan.abi_identity);
    let offset = match location {
        ccc_abi::BridgeLocation::Register(register) => match register.bank {
            ccc_abi::RegisterBank::Integer => {
                layout.call_integer_arguments + u64::from(register.index) * 8
            }
            ccc_abi::RegisterBank::Float => {
                layout.call_float_arguments + u64::from(register.index) * 16
            }
            ccc_abi::RegisterBank::X87 => {
                return Err(error("x87 is not an outgoing argument register bank"));
            }
        },
        ccc_abi::BridgeLocation::Stack { offset } => layout.call_fixed_size + u64::from(offset),
    };
    address_offset(builder, frame, offset)
}

fn bridge_result_piece_address(
    builder: &mut FunctionBuilder<'_>,
    frame: ir::Value,
    plan: &ccc_abi::BridgeBoundaryPlan,
    location: ccc_abi::BridgeLocation,
) -> Result<ir::Value, CodegenError> {
    variadic_result_piece_address(builder, frame, plan, location)
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
    builder
        .ins()
        .store(backend::empty_memory_flags(), value, destination, 0);
    Ok(())
}

fn value_representation_type(
    types: &TypeStore,
    ty: QualifiedType,
    config: &EffectiveCompilationConfig,
) -> Result<ir::Type, CodegenError> {
    if is_x87_f80(types, ty, config)
        || matches!(
            types.try_kind(ty.ty),
            Some(TypeKind::Array(_) | TypeKind::Record(_))
        )
    {
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
    if is_x87_f80(types, requested, config)
        || matches!(
            types.try_kind(requested.ty),
            Some(TypeKind::Array(_) | TypeKind::Record(_))
        )
    {
        Ok(address)
    } else {
        Ok(builder.ins().load(
            scalar_type(types, requested, config)?,
            backend::empty_memory_flags(),
            address,
            0,
        ))
    }
}

fn is_x87_f80(types: &TypeStore, ty: QualifiedType, config: &EffectiveCompilationConfig) -> bool {
    config.target.abi == ccc_target::AbiIdentity::SysvAmd64Lp64
        && config.target.data_layout.long_double_format == ccc_target::LongDoubleFormat::X87Extended
        && types.builtin_type(ty.ty) == Some(BuiltinType::LongDouble)
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
        Some(TypeKind::Builtin(BuiltinType::LongDouble))
            if config.target.data_layout.long_double_width == 64 =>
        {
            Ok(ir::types::F64)
        }
        Some(TypeKind::Builtin(BuiltinType::LongDouble)) => Err(CodegenError {
            code: "CCC3509",
            message: "binary128 `long double` values require a target transport capability"
                .to_owned(),
            span: None,
        }),
        Some(TypeKind::Builtin(BuiltinType::Float16)) => Ok(ir::types::I16),
        Some(TypeKind::Builtin(BuiltinType::Int128 | BuiltinType::UnsignedInt128)) => {
            if config.target.abi.supports_int128_values() {
                Ok(ir::types::I128)
            } else {
                Err(CodegenError {
                    code: "CCC3517",
                    message: "128-bit integer values require an enabled transport capability"
                        .to_owned(),
                    span: None,
                })
            }
        }
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
        Some(TypeKind::AlignmentAdjusted(adjusted)) => scalar_type(
            types,
            QualifiedType::new(adjusted.underlying, ty.qualifiers),
            config,
        ),
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
        16 => Ok(ir::types::I128),
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
            BuiltinType::Int128 => true,
            BuiltinType::UnsignedInt128 => false,
            BuiltinType::Void
            | BuiltinType::Float16
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
        Some(TypeKind::AlignmentAdjusted(adjusted)) => is_signed(
            types,
            QualifiedType::new(adjusted.underlying, ty.qualifiers),
            config,
        ),
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
        Some(
            BuiltinType::Float16
                | BuiltinType::Float
                | BuiltinType::Double
                | BuiltinType::LongDouble
        )
    )
}

fn lower_constant(
    builder: &mut FunctionBuilder<'_>,
    types: &TypeStore,
    ty: QualifiedType,
    constant: gir::ScalarConstant,
    config: &EffectiveCompilationConfig,
) -> Result<ir::Value, CodegenError> {
    if let gir::ScalarConstant::LongDouble(value) = constant {
        if !is_x87_f80(types, ty, config)
            || value.format != ccc_target::LongDoubleFormat::X87Extended
            || value.format != config.target.data_layout.long_double_format
        {
            return Err(CodegenError {
                code: "CCC3509",
                message: "long-double constant has no enabled target representation".to_owned(),
                span: None,
            });
        }
        let result = create_stack_backing(builder, 16, 16)?;
        zero_memory(builder, result, 16)?;
        let low = u64::from_le_bytes(
            value.bytes[..8]
                .try_into()
                .expect("long-double low word has eight bytes"),
        );
        let high = u16::from_le_bytes(
            value.bytes[8..10]
                .try_into()
                .expect("long-double exponent word has two bytes"),
        );
        store_integer(builder, result, 0, ir::types::I64, low as i64)?;
        store_integer(builder, result, 8, ir::types::I16, i64::from(high))?;
        return Ok(result);
    }
    let clif_ty = scalar_type(types, ty, config)?;
    if types.builtin_type(ty.ty) == Some(BuiltinType::Bool) {
        let normalized = scalar_constant_bits(types, ty, constant, config)? as i64;
        return Ok(builder.ins().iconst(ir::types::I8, normalized));
    }
    match constant {
        gir::ScalarConstant::Signed(value) if clif_ty == ir::types::I128 => {
            Ok(i128_constant(builder, value as u128))
        }
        gir::ScalarConstant::Unsigned(value) if clif_ty == ir::types::I128 => {
            Ok(i128_constant(builder, value))
        }
        gir::ScalarConstant::Signed(value) => Ok(builder.ins().iconst(clif_ty, value as i64)),
        gir::ScalarConstant::Unsigned(value) => Ok(builder.ins().iconst(clif_ty, value as i64)),
        gir::ScalarConstant::Floating(value) => match clif_ty {
            ir::types::I16 if types.builtin_type(ty.ty) == Some(BuiltinType::Float16) => {
                Ok(builder
                    .ins()
                    .iconst(ir::types::I16, i64::from(f64_to_f16_bits(value))))
            }
            ir::types::F32 => Ok(builder
                .ins()
                .f32const(Ieee32::with_bits((value as f32).to_bits()))),
            ir::types::F64 => Ok(builder.ins().f64const(Ieee64::with_bits(value.to_bits()))),
            _ => Err(error("floating constant has a non-floating result type")),
        },
        gir::ScalarConstant::LongDouble(_) => unreachable!(),
        gir::ScalarConstant::NullPointer => {
            if !matches!(types.try_kind(ty.ty), Some(TypeKind::Pointer(_))) {
                return Err(error("null pointer constant has a non-pointer result type"));
            }
            Ok(builder.ins().iconst(ir::types::I64, 0))
        }
    }
}

fn i128_constant(builder: &mut FunctionBuilder<'_>, value: u128) -> ir::Value {
    let low = builder.ins().iconst(ir::types::I64, value as u64 as i64);
    let high = builder
        .ins()
        .iconst(ir::types::I64, (value >> 64) as u64 as i64);
    builder.ins().iconcat(low, high)
}

fn integer_and_mask(
    builder: &mut FunctionBuilder<'_>,
    value: ir::Value,
    ty: ir::Type,
    mask: u128,
) -> ir::Value {
    if ty == ir::types::I128 {
        let mask = i128_constant(builder, mask);
        builder.ins().band(value, mask)
    } else {
        builder.ins().band_imm_u(value, mask as u64 as i64)
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

fn atomic_scalar_type(
    types: &TypeStore,
    object: QualifiedType,
    config: &EffectiveCompilationConfig,
) -> Result<ir::Type, CodegenError> {
    let ty = scalar_type(types, object, config)?;
    if ty.is_int() && matches!(ty.bits(), 8 | 16 | 32 | 64) {
        Ok(ty)
    } else {
        Err(CodegenError {
            code: ATOMIC_ERROR,
            message: format!(
                "atomic operation requires a native 1, 2, 4, or 8-byte integer representation, not `{}`",
                types.display_qualified(object)
            ),
            span: None,
        })
    }
}

fn lower_load(
    builder: &mut FunctionBuilder<'_>,
    address: ir::Value,
    ty: ir::Type,
    access: gir::MemoryAccess,
) -> Result<ir::Value, CodegenError> {
    if access.atomic.is_some() {
        validate_atomic_clif_type(ty)?;
    }
    if access.volatile || access.atomic.is_some() {
        builder.ins().fence();
    }
    let value = builder
        .ins()
        .load(ty, backend::empty_memory_flags(), address, 0);
    if access.volatile || access.atomic.is_some() {
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
    if access.atomic.is_some() {
        validate_atomic_clif_type(builder.func.dfg.value_type(value))?;
    }
    if access.volatile || access.atomic.is_some() {
        builder.ins().fence();
    }
    builder
        .ins()
        .store(backend::empty_memory_flags(), value, address, 0);
    if access.volatile || access.atomic.is_some() {
        builder.ins().fence();
    }
    Ok(())
}

fn validate_atomic_clif_type(ty: ir::Type) -> Result<(), CodegenError> {
    if ty.is_int() && matches!(ty.bits(), 8 | 16 | 32 | 64) {
        Ok(())
    } else {
        Err(CodegenError {
            code: ATOMIC_ERROR,
            message: "atomic load or store requires a native 1, 2, 4, or 8-byte integer or pointer representation".to_owned(),
            span: None,
        })
    }
}

fn address_offset(
    builder: &mut FunctionBuilder<'_>,
    address: ir::Value,
    offset: u64,
) -> Result<ir::Value, CodegenError> {
    if offset == 0 {
        return Ok(address);
    }
    Ok(builder.ins().iadd_imm_s(
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
        builder
            .ins()
            .store(backend::empty_memory_flags(), zero, address, 0);
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
    if !matches!(descriptor.storage_size, 1 | 2 | 4 | 8 | 16) {
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

#[allow(clippy::too_many_arguments)]
fn lower_conversion(
    builder: &mut FunctionBuilder<'_>,
    types: &TypeStore,
    operand: ir::Value,
    kind: gir::ScalarConversion,
    from: QualifiedType,
    to: QualifiedType,
    config: &EffectiveCompilationConfig,
    runtime_helpers: &FunctionReferences<'_>,
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
    let from_float16 = types.builtin_type(from.ty) == Some(BuiltinType::Float16);
    let to_float16 = types.builtin_type(to.ty) == Some(BuiltinType::Float16);
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
        gir::ScalarConversion::FloatingConversion if from_float16 && to_float16 => operand,
        gir::ScalarConversion::FloatingConversion if from_float16 => {
            let value = float16_to_f32(builder, operand);
            if destination == ir::types::F64 {
                builder.ins().fpromote(destination, value)
            } else if destination == ir::types::F32 {
                value
            } else {
                return Err(error("invalid `_Float16` floating destination"));
            }
        }
        gir::ScalarConversion::FloatingConversion if to_float16 => match source {
            ir::types::F32 => f32_to_float16(builder, operand),
            ir::types::F64 => f64_to_float16(builder, operand),
            _ => return Err(error("invalid `_Float16` floating source")),
        },
        gir::ScalarConversion::FloatingConversion => match (source, destination) {
            (ir::types::F32, ir::types::F32) | (ir::types::F64, ir::types::F64) => operand,
            (ir::types::F32, ir::types::F64) => builder.ins().fpromote(destination, operand),
            (ir::types::F64, ir::types::F32) => builder.ins().fdemote(destination, operand),
            _ => return Err(error("invalid floating conversion types")),
        },
        gir::ScalarConversion::IntegerToFloating => {
            let signed = is_signed(types, from, config)?;
            let floating_destination = if to_float16 {
                ir::types::F32
            } else {
                destination
            };
            let converted = if source == ir::types::I128 {
                let symbol = match (signed, floating_destination) {
                    (true, ir::types::F32) => "__floattisf",
                    (true, ir::types::F64) => "__floattidf",
                    (false, ir::types::F32) => "__floatuntisf",
                    (false, ir::types::F64) => "__floatuntidf",
                    _ => return Err(error("invalid wide integer-to-floating conversion")),
                };
                runtime_helper_call(builder, runtime_helpers, symbol, &[operand])?
            } else if signed {
                builder.ins().fcvt_from_sint(floating_destination, operand)
            } else {
                builder.ins().fcvt_from_uint(floating_destination, operand)
            };
            if to_float16 {
                f32_to_float16(builder, converted)
            } else {
                converted
            }
        }
        gir::ScalarConversion::FloatingToInteger => {
            let signed = is_signed(types, to, config)?;
            let (operand, source) = if from_float16 {
                let value = float16_to_f32(builder, operand);
                (value, ir::types::F32)
            } else {
                (operand, source)
            };
            if destination == ir::types::I128 {
                let symbol = match (signed, source) {
                    (true, ir::types::F32) => "__fixsfti",
                    (true, ir::types::F64) => "__fixdfti",
                    (false, ir::types::F32) => "__fixunssfti",
                    (false, ir::types::F64) => "__fixunsdfti",
                    _ => return Err(error("invalid floating-to-wide-integer conversion")),
                };
                runtime_helper_call(builder, runtime_helpers, symbol, &[operand])?
            } else if signed {
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
        if types.builtin_type(from.ty) == Some(BuiltinType::Float16) {
            let magnitude = builder.ins().band_imm_u(operand, 0x7fff);
            builder.ins().icmp_imm_s(IntCC::NotEqual, magnitude, 0)
        } else {
            let zero = match source {
                ir::types::F32 => builder.ins().f32const(Ieee32::with_bits(0)),
                ir::types::F64 => builder.ins().f64const(Ieee64::with_bits(0)),
                _ => return Err(error("floating boolean conversion has invalid source type")),
            };
            builder.ins().fcmp(FloatCC::NotEqual, operand, zero)
        }
    } else {
        builder.ins().icmp_imm_s(IntCC::NotEqual, operand, 0)
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

fn float16_to_f32(builder: &mut FunctionBuilder<'_>, value: ir::Value) -> ir::Value {
    let raw = builder.ins().uextend(ir::types::I32, value);
    let sign = builder.ins().band_imm_u(raw, 0x8000);
    let sign = builder.ins().ishl_imm_u(sign, 16);
    let exponent = builder.ins().ushr_imm_u(raw, 10);
    let exponent = builder.ins().band_imm_u(exponent, 0x1f);
    let fraction = builder.ins().band_imm_u(raw, 0x03ff);

    let normal_exponent = builder.ins().iadd_imm_s(exponent, 112);
    let normal_exponent = builder.ins().ishl_imm_u(normal_exponent, 23);
    let normal_fraction = builder.ins().ishl_imm_u(fraction, 13);
    let normal = builder.ins().bor(sign, normal_exponent);
    let normal = builder.ins().bor(normal, normal_fraction);

    let special_exponent = builder.ins().iconst(ir::types::I32, 0x7f80_0000);
    let special = builder.ins().bor(sign, special_exponent);
    let special = builder.ins().bor(special, normal_fraction);

    let subnormal = builder.ins().fcvt_from_uint(ir::types::F32, fraction);
    let scale = builder
        .ins()
        .f32const(Ieee32::with_bits((2.0f32).powi(-24).to_bits()));
    let subnormal = builder.ins().fmul(subnormal, scale);
    let subnormal = builder
        .ins()
        .bitcast(ir::types::I32, backend::empty_memory_flags(), subnormal);
    let subnormal = builder.ins().bor(subnormal, sign);

    let is_zero_or_subnormal = builder.ins().icmp_imm_s(IntCC::Equal, exponent, 0);
    let is_special = builder.ins().icmp_imm_s(IntCC::Equal, exponent, 0x1f);
    let finite = builder
        .ins()
        .select(is_zero_or_subnormal, subnormal, normal);
    let bits = builder.ins().select(is_special, special, finite);
    builder
        .ins()
        .bitcast(ir::types::F32, backend::empty_memory_flags(), bits)
}

fn f32_to_float16(builder: &mut FunctionBuilder<'_>, value: ir::Value) -> ir::Value {
    let raw = builder
        .ins()
        .bitcast(ir::types::I32, backend::empty_memory_flags(), value);
    let sign = builder.ins().ushr_imm_u(raw, 16);
    let sign = builder.ins().band_imm_u(sign, 0x8000);
    let magnitude = builder.ins().band_imm_u(raw, 0x7fff_ffff);
    let exponent = builder.ins().ushr_imm_u(magnitude, 23);
    let fraction = builder.ins().band_imm_u(magnitude, 0x007f_ffff);

    let half_exponent = builder.ins().iadd_imm_s(exponent, -112);
    let half_exponent = builder.ins().ishl_imm_u(half_exponent, 10);
    let normal_fraction = builder.ins().ushr_imm_u(fraction, 13);
    let normal_base = builder.ins().bor(half_exponent, normal_fraction);
    let normal_remainder = builder.ins().band_imm_u(fraction, 0x1fff);
    let normal_above =
        builder
            .ins()
            .icmp_imm_s(IntCC::UnsignedGreaterThan, normal_remainder, 0x1000);
    let normal_tie = builder
        .ins()
        .icmp_imm_s(IntCC::Equal, normal_remainder, 0x1000);
    let normal_odd = builder.ins().band_imm_u(normal_base, 1);
    let normal_odd = builder.ins().icmp_imm_s(IntCC::NotEqual, normal_odd, 0);
    let normal_tie_odd = builder.ins().band(normal_tie, normal_odd);
    let normal_increment = builder.ins().bor(normal_above, normal_tie_odd);
    let normal_increment = builder.ins().uextend(ir::types::I32, normal_increment);
    let normal = builder.ins().iadd(normal_base, normal_increment);

    let subnormal_bias = builder.ins().iconst(ir::types::I32, 126);
    let shift = builder.ins().isub(subnormal_bias, exponent);
    let in_subnormal_range =
        builder
            .ins()
            .icmp_imm_s(IntCC::UnsignedGreaterThanOrEqual, exponent, 102);
    let maximum_subnormal_exponent =
        builder
            .ins()
            .icmp_imm_s(IntCC::UnsignedLessThanOrEqual, exponent, 112);
    let safe_subnormal = builder
        .ins()
        .band(in_subnormal_range, maximum_subnormal_exponent);
    let fallback_shift = builder.ins().iconst(ir::types::I32, 24);
    let shift = builder.ins().select(safe_subnormal, shift, fallback_shift);
    let significand_bit = builder.ins().iconst(ir::types::I32, 0x0080_0000);
    let significand = builder.ins().bor(fraction, significand_bit);
    let subnormal_base = builder.ins().ushr(significand, shift);
    let one = builder.ins().iconst(ir::types::I32, 1);
    let divisor = builder.ins().ishl(one, shift);
    let mask = builder.ins().iadd_imm_s(divisor, -1);
    let subnormal_remainder = builder.ins().band(significand, mask);
    let half_shift = builder.ins().iadd_imm_s(shift, -1);
    let halfway = builder.ins().ishl(one, half_shift);
    let subnormal_above =
        builder
            .ins()
            .icmp(IntCC::UnsignedGreaterThan, subnormal_remainder, halfway);
    let subnormal_tie = builder
        .ins()
        .icmp(IntCC::Equal, subnormal_remainder, halfway);
    let subnormal_odd = builder.ins().band_imm_u(subnormal_base, 1);
    let subnormal_odd = builder.ins().icmp_imm_s(IntCC::NotEqual, subnormal_odd, 0);
    let subnormal_tie_odd = builder.ins().band(subnormal_tie, subnormal_odd);
    let subnormal_increment = builder.ins().bor(subnormal_above, subnormal_tie_odd);
    let subnormal_increment = builder.ins().uextend(ir::types::I32, subnormal_increment);
    let subnormal = builder.ins().iadd(subnormal_base, subnormal_increment);

    let payload = builder.ins().ushr_imm_u(fraction, 13);
    let has_payload = builder.ins().icmp_imm_s(IntCC::NotEqual, fraction, 0);
    let has_payload = builder.ins().uextend(ir::types::I32, has_payload);
    let payload = builder.ins().bor(payload, has_payload);
    let infinity = builder.ins().iconst(ir::types::I32, 0x7c00);
    let special = builder.ins().bor(infinity, payload);

    let is_special = builder.ins().icmp_imm_s(IntCC::Equal, exponent, 0xff);
    let overflows =
        builder
            .ins()
            .icmp_imm_s(IntCC::UnsignedGreaterThanOrEqual, magnitude, 0x477f_f000);
    let is_normal =
        builder
            .ins()
            .icmp_imm_s(IntCC::UnsignedGreaterThanOrEqual, magnitude, 0x3880_0000);
    let zero = builder.ins().iconst(ir::types::I32, 0);
    let underflow = builder.ins().select(in_subnormal_range, subnormal, zero);
    let finite = builder.ins().select(is_normal, normal, underflow);
    let finite = builder.ins().select(overflows, infinity, finite);
    let magnitude = builder.ins().select(is_special, special, finite);
    let result = builder.ins().bor(sign, magnitude);
    builder.ins().ireduce(ir::types::I16, result)
}

fn f64_to_float16(builder: &mut FunctionBuilder<'_>, value: ir::Value) -> ir::Value {
    let raw = builder
        .ins()
        .bitcast(ir::types::I64, backend::empty_memory_flags(), value);
    let sign = builder.ins().ushr_imm_u(raw, 48);
    let sign = builder.ins().band_imm_u(sign, 0x8000);
    let magnitude = builder.ins().band_imm_u(raw, 0x7fff_ffff_ffff_ffff);
    let exponent = builder.ins().ushr_imm_u(magnitude, 52);
    let fraction = builder.ins().band_imm_u(magnitude, 0x000f_ffff_ffff_ffff);

    let half_exponent = builder.ins().iadd_imm_s(exponent, -1008);
    let half_exponent = builder.ins().ishl_imm_u(half_exponent, 10);
    let normal_fraction = builder.ins().ushr_imm_u(fraction, 42);
    let normal_base = builder.ins().bor(half_exponent, normal_fraction);
    let normal_remainder = builder.ins().band_imm_u(fraction, (1i64 << 42) - 1);
    let normal_above =
        builder
            .ins()
            .icmp_imm_s(IntCC::UnsignedGreaterThan, normal_remainder, 1i64 << 41);
    let normal_tie = builder
        .ins()
        .icmp_imm_s(IntCC::Equal, normal_remainder, 1i64 << 41);
    let normal_odd = builder.ins().band_imm_u(normal_base, 1);
    let normal_odd = builder.ins().icmp_imm_s(IntCC::NotEqual, normal_odd, 0);
    let normal_tie_odd = builder.ins().band(normal_tie, normal_odd);
    let normal_increment = builder.ins().bor(normal_above, normal_tie_odd);
    let normal_increment = builder.ins().uextend(ir::types::I64, normal_increment);
    let normal = builder.ins().iadd(normal_base, normal_increment);

    let subnormal_bias = builder.ins().iconst(ir::types::I64, 1051);
    let shift = builder.ins().isub(subnormal_bias, exponent);
    let in_subnormal_range =
        builder
            .ins()
            .icmp_imm_s(IntCC::UnsignedGreaterThanOrEqual, exponent, 998);
    let maximum_subnormal_exponent =
        builder
            .ins()
            .icmp_imm_s(IntCC::UnsignedLessThanOrEqual, exponent, 1008);
    let safe_subnormal = builder
        .ins()
        .band(in_subnormal_range, maximum_subnormal_exponent);
    let fallback_shift = builder.ins().iconst(ir::types::I64, 53);
    let shift = builder.ins().select(safe_subnormal, shift, fallback_shift);
    let significand_bit = builder.ins().iconst(ir::types::I64, 0x0010_0000_0000_0000);
    let significand = builder.ins().bor(fraction, significand_bit);
    let subnormal_base = builder.ins().ushr(significand, shift);
    let one = builder.ins().iconst(ir::types::I64, 1);
    let divisor = builder.ins().ishl(one, shift);
    let mask = builder.ins().iadd_imm_s(divisor, -1);
    let subnormal_remainder = builder.ins().band(significand, mask);
    let half_shift = builder.ins().iadd_imm_s(shift, -1);
    let halfway = builder.ins().ishl(one, half_shift);
    let subnormal_above =
        builder
            .ins()
            .icmp(IntCC::UnsignedGreaterThan, subnormal_remainder, halfway);
    let subnormal_tie = builder
        .ins()
        .icmp(IntCC::Equal, subnormal_remainder, halfway);
    let subnormal_odd = builder.ins().band_imm_u(subnormal_base, 1);
    let subnormal_odd = builder.ins().icmp_imm_s(IntCC::NotEqual, subnormal_odd, 0);
    let subnormal_tie_odd = builder.ins().band(subnormal_tie, subnormal_odd);
    let subnormal_increment = builder.ins().bor(subnormal_above, subnormal_tie_odd);
    let subnormal_increment = builder.ins().uextend(ir::types::I64, subnormal_increment);
    let subnormal = builder.ins().iadd(subnormal_base, subnormal_increment);

    let payload = builder.ins().ushr_imm_u(fraction, 42);
    let has_payload = builder.ins().icmp_imm_s(IntCC::NotEqual, fraction, 0);
    let has_payload = builder.ins().uextend(ir::types::I64, has_payload);
    let payload = builder.ins().bor(payload, has_payload);
    let infinity = builder.ins().iconst(ir::types::I64, 0x7c00);
    let special = builder.ins().bor(infinity, payload);
    let is_special = builder.ins().icmp_imm_s(IntCC::Equal, exponent, 0x7ff);
    let overflows = builder.ins().icmp_imm_s(
        IntCC::UnsignedGreaterThanOrEqual,
        magnitude,
        0x40ef_fe00_0000_0000,
    );
    let is_normal = builder.ins().icmp_imm_s(
        IntCC::UnsignedGreaterThanOrEqual,
        magnitude,
        0x3f10_0000_0000_0000,
    );
    let zero = builder.ins().iconst(ir::types::I64, 0);
    let underflow = builder.ins().select(in_subnormal_range, subnormal, zero);
    let finite = builder.ins().select(is_normal, normal, underflow);
    let finite = builder.ins().select(overflows, infinity, finite);
    let magnitude = builder.ins().select(is_special, special, finite);
    let result = builder.ins().bor(sign, magnitude);
    builder.ins().ireduce(ir::types::I16, result)
}

fn f80_to_float16(builder: &mut FunctionBuilder<'_>, address: ir::Value) -> ir::Value {
    let significand = builder
        .ins()
        .load(ir::types::I64, backend::empty_memory_flags(), address, 0);
    let high = builder
        .ins()
        .load(ir::types::I16, backend::empty_memory_flags(), address, 8);
    let high = builder.ins().uextend(ir::types::I64, high);
    let sign = builder.ins().band_imm_u(high, 0x8000);
    let exponent = builder.ins().band_imm_u(high, 0x7fff);

    let half_exponent = builder.ins().iadd_imm_s(exponent, -16_368);
    let half_exponent = builder.ins().ishl_imm_u(half_exponent, 10);
    let normal_fraction = builder.ins().ushr_imm_u(significand, 53);
    let normal_fraction = builder.ins().band_imm_u(normal_fraction, 0x03ff);
    let normal_base = builder.ins().bor(half_exponent, normal_fraction);
    let normal_remainder = builder.ins().band_imm_u(significand, 0x001f_ffff_ffff_ffff);
    let normal_above = builder.ins().icmp_imm_s(
        IntCC::UnsignedGreaterThan,
        normal_remainder,
        0x0010_0000_0000_0000,
    );
    let normal_tie =
        builder
            .ins()
            .icmp_imm_s(IntCC::Equal, normal_remainder, 0x0010_0000_0000_0000);
    let normal_odd = builder.ins().band_imm_u(normal_base, 1);
    let normal_odd = builder.ins().icmp_imm_s(IntCC::NotEqual, normal_odd, 0);
    let normal_tie_odd = builder.ins().band(normal_tie, normal_odd);
    let normal_increment = builder.ins().bor(normal_above, normal_tie_odd);
    let normal_increment = builder.ins().uextend(ir::types::I64, normal_increment);
    let normal = builder.ins().iadd(normal_base, normal_increment);

    let subnormal_bias = builder.ins().iconst(ir::types::I64, 16_422);
    let shift = builder.ins().isub(subnormal_bias, exponent);
    let has_regular_subnormal_shift =
        builder
            .ins()
            .icmp_imm_s(IntCC::UnsignedGreaterThanOrEqual, exponent, 16_359);
    let maximum_subnormal_exponent =
        builder
            .ins()
            .icmp_imm_s(IntCC::UnsignedLessThanOrEqual, exponent, 16_368);
    let safe_subnormal = builder
        .ins()
        .band(has_regular_subnormal_shift, maximum_subnormal_exponent);
    let fallback_shift = builder.ins().iconst(ir::types::I64, 63);
    let shift = builder.ins().select(safe_subnormal, shift, fallback_shift);
    let subnormal_base = builder.ins().ushr(significand, shift);
    let one = builder.ins().iconst(ir::types::I64, 1);
    let divisor = builder.ins().ishl(one, shift);
    let mask = builder.ins().iadd_imm_s(divisor, -1);
    let subnormal_remainder = builder.ins().band(significand, mask);
    let half_shift = builder.ins().iadd_imm_s(shift, -1);
    let halfway = builder.ins().ishl(one, half_shift);
    let subnormal_above =
        builder
            .ins()
            .icmp(IntCC::UnsignedGreaterThan, subnormal_remainder, halfway);
    let subnormal_tie = builder
        .ins()
        .icmp(IntCC::Equal, subnormal_remainder, halfway);
    let subnormal_odd = builder.ins().band_imm_u(subnormal_base, 1);
    let subnormal_odd = builder.ins().icmp_imm_s(IntCC::NotEqual, subnormal_odd, 0);
    let subnormal_tie_odd = builder.ins().band(subnormal_tie, subnormal_odd);
    let subnormal_increment = builder.ins().bor(subnormal_above, subnormal_tie_odd);
    let subnormal_increment = builder.ins().uextend(ir::types::I64, subnormal_increment);
    let regular_subnormal = builder.ins().iadd(subnormal_base, subnormal_increment);

    let minimum_halfway = builder
        .ins()
        .iconst(ir::types::I64, 0x8000_0000_0000_0000_u64 as i64);
    let above_minimum_halfway =
        builder
            .ins()
            .icmp(IntCC::UnsignedGreaterThan, significand, minimum_halfway);
    let minimum_subnormal = builder.ins().uextend(ir::types::I64, above_minimum_halfway);
    let is_minimum_exponent = builder.ins().icmp_imm_s(IntCC::Equal, exponent, 16_358);
    let subnormal = builder
        .ins()
        .select(is_minimum_exponent, minimum_subnormal, regular_subnormal);

    let fraction = builder.ins().band_imm_u(significand, 0x7fff_ffff_ffff_ffff);
    let payload = builder.ins().ushr_imm_u(fraction, 53);
    let has_payload = builder.ins().icmp_imm_s(IntCC::NotEqual, fraction, 0);
    let has_payload = builder.ins().uextend(ir::types::I64, has_payload);
    let payload = builder.ins().bor(payload, has_payload);
    let infinity = builder.ins().iconst(ir::types::I64, 0x7c00);
    let special = builder.ins().bor(infinity, payload);

    let is_special = builder.ins().icmp_imm_s(IntCC::Equal, exponent, 0x7fff);
    let overflows = builder
        .ins()
        .icmp_imm_s(IntCC::UnsignedGreaterThan, exponent, 16_398);
    let is_normal = builder
        .ins()
        .icmp_imm_s(IntCC::UnsignedGreaterThanOrEqual, exponent, 16_369);
    let in_subnormal_range =
        builder
            .ins()
            .icmp_imm_s(IntCC::UnsignedGreaterThanOrEqual, exponent, 16_358);
    let zero = builder.ins().iconst(ir::types::I64, 0);
    let underflow = builder.ins().select(in_subnormal_range, subnormal, zero);
    let finite = builder.ins().select(is_normal, normal, underflow);
    let finite = builder.ins().select(overflows, infinity, finite);
    let magnitude = builder.ins().select(is_special, special, finite);
    let result = builder.ins().bor(sign, magnitude);
    builder.ins().ireduce(ir::types::I16, result)
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
        gir::UnaryOperation::Negate
            if types.builtin_type(operand_ty.ty) == Some(BuiltinType::Float16) =>
        {
            Ok(builder.ins().bxor_imm_u(operand, 0x8000))
        }
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
                if types.builtin_type(operand_ty.ty) == Some(BuiltinType::Float16) {
                    let magnitude = builder.ins().band_imm_u(operand, 0x7fff);
                    builder.ins().icmp_imm_s(IntCC::Equal, magnitude, 0)
                } else {
                    let source = builder.func.dfg.value_type(operand);
                    let zero = match source {
                        ir::types::F32 => builder.ins().f32const(Ieee32::with_bits(0)),
                        ir::types::F64 => builder.ins().f64const(Ieee64::with_bits(0)),
                        _ => return Err(error("floating logical-not has invalid source type")),
                    };
                    builder.ins().fcmp(FloatCC::Equal, operand, zero)
                }
            } else {
                builder.ins().icmp_imm_s(IntCC::Equal, operand, 0)
            };
            let source = builder.func.dfg.value_type(boolean);
            Ok(coerce_integer(builder, boolean, source, result, false))
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn lower_binary(
    builder: &mut FunctionBuilder<'_>,
    operator: gir::BinaryOperation,
    mut left: ir::Value,
    mut right: ir::Value,
    floating: bool,
    signed: bool,
    result: ir::Type,
    runtime_helpers: &FunctionReferences<'_>,
) -> Result<ir::Value, CodegenError> {
    let float16 = floating && builder.func.dfg.value_type(left) == ir::types::I16;
    if float16 {
        left = float16_to_f32(builder, left);
        right = float16_to_f32(builder, right);
    }
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
        let value = match operator {
            gir::BinaryOperation::Multiply => builder.ins().fmul(left, right),
            gir::BinaryOperation::Divide => builder.ins().fdiv(left, right),
            gir::BinaryOperation::Add => builder.ins().fadd(left, right),
            gir::BinaryOperation::Subtract => builder.ins().fsub(left, right),
            _ => Err(error(format!(
                "operator {operator:?} is invalid for floating values"
            )))?,
        };
        return Ok(if float16 {
            f32_to_float16(builder, value)
        } else {
            value
        });
    }
    if result == ir::types::I128
        && matches!(
            operator,
            gir::BinaryOperation::Divide | gir::BinaryOperation::Remainder
        )
    {
        let symbol = match (operator, signed) {
            (gir::BinaryOperation::Divide, true) => "__divti3",
            (gir::BinaryOperation::Divide, false) => "__udivti3",
            (gir::BinaryOperation::Remainder, true) => "__modti3",
            (gir::BinaryOperation::Remainder, false) => "__umodti3",
            _ => unreachable!(),
        };
        return runtime_helper_call(builder, runtime_helpers, symbol, &[left, right]);
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

fn runtime_helper_call(
    builder: &mut FunctionBuilder<'_>,
    runtime_helpers: &FunctionReferences<'_>,
    symbol: &'static str,
    arguments: &[ir::Value],
) -> Result<ir::Value, CodegenError> {
    let reference = runtime_helpers.runtime_helper(builder, symbol)?;
    let call = builder.ins().call(reference, arguments);
    match builder.inst_results(call) {
        [result] => Ok(*result),
        results => Err(error(format!(
            "runtime helper `{symbol}` produced {} results",
            results.len()
        ))),
    }
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
