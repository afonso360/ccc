use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};

use ccc_pp::StringLiteralPrefix;
use ccc_sema::generic::{
    AccessSemantics, AtomicReadModifyWriteOperation as TypedAtomicReadModifyWriteOperation,
    BitfieldPlace, CompoundAssignmentPlan, ConstantValue, ConversionKind, FullFunctionId,
    FullLocalId, FullTypedBlockItem, FullTypedExpression, FullTypedExpressionKind,
    FullTypedForInitializer, FullTypedFunction, FullTypedInitializer, FullTypedInitializerKind,
    FullTypedLocalDeclaration, FullTypedStatement, FullTypedStatementKind,
    FullTypedTranslationUnit, GlobalId, InitializerPathElement,
    IntegerIntrinsicOperation as TypedIntegerIntrinsicOperation, LabelId, Linkage,
    MemoryOrder as TypedMemoryOrder, Place, PlaceBase, StorageDuration, StringId, SymbolReference,
};
use ccc_session::Span;
use ccc_syntax::frontend::{
    AssignmentOperator as AstAssignment, BinaryOperator as AstBinary, UnaryOperator as AstUnary,
};
use ccc_types::{
    ArrayLength, BuiltinType, FunctionParameters, QualifiedType, TypeId, TypeKind, TypeQualifiers,
    TypeStore,
};

use super::{
    AggregateOverlap, AggregateProjection, AtomicReadModifyWriteOperation, BinaryOperation,
    BitfieldDescriptor, BlockId, CallEffects, DataId, DataOrigin, FullBlock, FullEdge,
    FullFunction, FullGlobal, FullInstruction, FullInstructionKind, FullModule, FullParameter,
    FullStorage, FullString, FullTerminator, InitializerEdge, InitializerGraph, InitializerNode,
    InitializerNodeId, InitializerNodeKind, InitializerPath, InstructionId,
    IntegerIntrinsicOperation, IrError, MemoryAccess, MemoryOrder, MemoryResidencyReason,
    RelocationKind, RelocationTarget, ScalarConstant, ScalarConversion, StorageId, StorageLocation,
    StringEncoding, SwitchEdge, UnaryOperation, ValueId,
};

const LOWERING_ERROR: &str = "CCC3101";

pub fn lower_frontend(unit: &FullTypedTranslationUnit) -> Result<FullModule, IrError> {
    let mut types = unit.types.clone();

    let file_data = unit
        .globals
        .iter()
        .enumerate()
        .map(|(index, global)| (global.id, DataId(index as u32)))
        .collect::<BTreeMap<_, _>>();
    let static_declarations = collect_static_declarations(unit);
    let static_data = static_declarations
        .iter()
        .enumerate()
        .map(|(index, (function, declaration))| {
            (
                (*function, declaration.local),
                (
                    DataId((unit.globals.len() + index) as u32),
                    declaration.duration,
                ),
            )
        })
        .collect::<BTreeMap<_, _>>();

    let mut globals = Vec::with_capacity(unit.globals.len() + static_declarations.len());
    for global in &unit.globals {
        globals.push(FullGlobal {
            id: file_data[&global.id],
            source: DataOrigin::FileScope(global.id),
            name: global.name.clone(),
            ty: global.ty,
            storage: global.storage,
            linkage: global.linkage,
            duration: global.duration,
            initializer: global
                .initializer
                .as_ref()
                .map(|initializer| {
                    lower_initializer_graph(initializer, &file_data, &static_data, unit, None)
                })
                .transpose()?,
            tentative: global.tentative,
            emission: global.emission.clone(),
            span: global.span,
        });
    }
    for (function, declaration) in static_declarations {
        let Some(emission) = declaration.emission.clone() else {
            return Err(IrError::lower(
                LOWERING_ERROR,
                declaration.span,
                format!(
                    "static-duration local `{}` has no data-emission metadata",
                    declaration.name
                ),
            ));
        };
        globals.push(FullGlobal {
            id: static_data[&(function, declaration.local)].0,
            source: DataOrigin::BlockStatic {
                function,
                local: declaration.local,
            },
            name: declaration.name.clone(),
            ty: declaration.ty,
            storage: declaration.storage,
            linkage: Linkage::None,
            duration: declaration.duration,
            initializer: declaration
                .initializer
                .as_ref()
                .map(|initializer| {
                    lower_initializer_graph(
                        initializer,
                        &file_data,
                        &static_data,
                        unit,
                        Some(function),
                    )
                })
                .transpose()?,
            tentative: false,
            emission,
            span: declaration.span,
        });
    }

    let strings = unit
        .strings
        .iter()
        .map(|string| FullString {
            id: string.id,
            encoding: match string.prefix {
                StringLiteralPrefix::None => StringEncoding::Ordinary,
                StringLiteralPrefix::Utf8 => StringEncoding::Utf8,
                StringLiteralPrefix::Wide => StringEncoding::Wide,
                StringLiteralPrefix::Utf16 => StringEncoding::Utf16,
                StringLiteralPrefix::Utf32 => StringEncoding::Utf32,
            },
            code_units: string.code_units.clone(),
            ty: string.ty,
        })
        .collect();

    let mut functions = Vec::with_capacity(unit.functions.len());
    for function in &unit.functions {
        functions.push(FunctionBuilder::lower(
            function,
            unit,
            &file_data,
            &static_data,
            &mut types,
        )?);
    }

    let module = FullModule {
        types,
        globals,
        strings,
        functions,
    };
    super::verify_frontend(&module)?;
    Ok(module)
}

fn collect_static_declarations(
    unit: &FullTypedTranslationUnit,
) -> Vec<(FullFunctionId, &FullTypedLocalDeclaration)> {
    let mut declarations = Vec::new();
    for function in &unit.functions {
        if let Some(body) = &function.body {
            collect_static_declarations_in_statement(function.id, body, &mut declarations);
        }
    }
    declarations
}

fn collect_static_declarations_in_statement<'a>(
    function: FullFunctionId,
    statement: &'a FullTypedStatement,
    declarations: &mut Vec<(FullFunctionId, &'a FullTypedLocalDeclaration)>,
) {
    use FullTypedStatementKind as S;
    match &statement.kind {
        S::Label { statement, .. } | S::Case { statement, .. } => {
            collect_static_declarations_in_statement(function, statement, declarations);
        }
        S::Default(statement) => {
            collect_static_declarations_in_statement(function, statement, declarations);
        }
        S::Compound(items) => {
            for item in items {
                match item {
                    FullTypedBlockItem::Declaration(declaration)
                        if declaration.duration != StorageDuration::Automatic =>
                    {
                        declarations.push((function, declaration));
                    }
                    FullTypedBlockItem::Statement(statement) => {
                        collect_static_declarations_in_statement(function, statement, declarations);
                    }
                    _ => {}
                }
            }
        }
        S::If {
            then_statement,
            else_statement,
            ..
        } => {
            collect_static_declarations_in_statement(function, then_statement, declarations);
            if let Some(statement) = else_statement {
                collect_static_declarations_in_statement(function, statement, declarations);
            }
        }
        S::For {
            initializer,
            statement,
            ..
        } => {
            if let FullTypedForInitializer::Declarations(items) = initializer {
                for item in items {
                    if let FullTypedBlockItem::Declaration(declaration) = item
                        && declaration.duration != StorageDuration::Automatic
                    {
                        declarations.push((function, declaration));
                    }
                }
            }
            collect_static_declarations_in_statement(function, statement, declarations);
        }
        S::Switch { statement, .. } | S::While { statement, .. } | S::DoWhile { statement, .. } => {
            collect_static_declarations_in_statement(function, statement, declarations);
        }
        S::Expression(_)
        | S::Goto { .. }
        | S::ComputedGoto(_)
        | S::Continue
        | S::Break
        | S::Return(_) => {}
    }
}

struct InitializerBuilder<'a> {
    nodes: Vec<InitializerNode>,
    file_data: &'a BTreeMap<GlobalId, DataId>,
    static_data: &'a BTreeMap<(FullFunctionId, FullLocalId), (DataId, StorageDuration)>,
    unit: &'a FullTypedTranslationUnit,
    function: Option<FullFunctionId>,
}

fn lower_initializer_graph(
    initializer: &FullTypedInitializer,
    file_data: &BTreeMap<GlobalId, DataId>,
    static_data: &BTreeMap<(FullFunctionId, FullLocalId), (DataId, StorageDuration)>,
    unit: &FullTypedTranslationUnit,
    function: Option<FullFunctionId>,
) -> Result<InitializerGraph, IrError> {
    let mut builder = InitializerBuilder {
        nodes: Vec::new(),
        file_data,
        static_data,
        unit,
        function,
    };
    let root = builder.initializer(initializer)?;
    Ok(InitializerGraph {
        root,
        nodes: builder.nodes,
    })
}

impl InitializerBuilder<'_> {
    fn initializer(
        &mut self,
        initializer: &FullTypedInitializer,
    ) -> Result<InitializerNodeId, IrError> {
        let kind = match &initializer.kind {
            FullTypedInitializerKind::Zero => InitializerNodeKind::Zero,
            FullTypedInitializerKind::String(string) => InitializerNodeKind::StringData {
                string: *string,
                copy_code_units: string_copy_code_units(
                    &self.unit.types,
                    self.unit,
                    initializer.ty,
                    *string,
                    initializer.span,
                )?,
            },
            FullTypedInitializerKind::Scalar(expression) => {
                let Some(constant) = expression.constant else {
                    return Err(IrError::lower(
                        LOWERING_ERROR,
                        expression.span,
                        "static initializer did not retain a constant value",
                    ));
                };
                constant_initializer(
                    constant,
                    self.file_data,
                    self.static_data,
                    self.unit,
                    self.function,
                    expression.span,
                )?
            }
            FullTypedInitializerKind::Aggregate(entries) => {
                if let Some((element, count)) = repeated_array_element(entries) {
                    let element = self.initializer(element)?;
                    return self.push(
                        initializer.ty,
                        InitializerNodeKind::Repeat { element, count },
                    );
                }
                let mut lowered = Vec::with_capacity(entries.len());
                for entry in entries {
                    let node = self.initializer(&entry.initializer)?;
                    lowered.push(InitializerEdge {
                        path: entry.path.iter().map(initializer_path).collect(),
                        node,
                    });
                }
                InitializerNodeKind::Aggregate(lowered)
            }
        };
        self.push(initializer.ty, kind)
    }

    fn push(
        &mut self,
        ty: QualifiedType,
        kind: InitializerNodeKind,
    ) -> Result<InitializerNodeId, IrError> {
        let id = InitializerNodeId(self.nodes.len() as u32);
        self.nodes.push(InitializerNode { id, ty, kind });
        Ok(id)
    }
}

fn repeated_array_element(
    entries: &[ccc_sema::generic::FullTypedInitializerEntry],
) -> Option<(&FullTypedInitializer, u64)> {
    if entries.len() < 2 {
        return None;
    }
    let first = entries.first()?;
    if first.path.as_slice() != [InitializerPathElement::Index(0)]
        || !is_repeatable_initializer(&first.initializer)
    {
        return None;
    }
    for (index, entry) in entries.iter().enumerate().skip(1) {
        let index = u64::try_from(index).ok()?;
        if entry.path.as_slice() != [InitializerPathElement::Index(index)]
            || !same_initializer_fragment(&first.initializer, &entry.initializer)
        {
            return None;
        }
    }
    Some((&first.initializer, u64::try_from(entries.len()).ok()?))
}

fn is_repeatable_initializer(initializer: &FullTypedInitializer) -> bool {
    match &initializer.kind {
        FullTypedInitializerKind::Zero => true,
        FullTypedInitializerKind::Scalar(expression) => expression.constant.is_some(),
        FullTypedInitializerKind::Aggregate(_) | FullTypedInitializerKind::String(_) => false,
    }
}

fn same_initializer_fragment(left: &FullTypedInitializer, right: &FullTypedInitializer) -> bool {
    if left.ty != right.ty {
        return false;
    }
    match (&left.kind, &right.kind) {
        (FullTypedInitializerKind::Zero, FullTypedInitializerKind::Zero) => true,
        (FullTypedInitializerKind::Scalar(left), FullTypedInitializerKind::Scalar(right)) => {
            same_constant_value(left.constant, right.constant)
        }
        _ => false,
    }
}

fn same_constant_value(left: Option<ConstantValue>, right: Option<ConstantValue>) -> bool {
    match (left, right) {
        (Some(ConstantValue::Floating(left)), Some(ConstantValue::Floating(right))) => {
            left.to_bits() == right.to_bits()
        }
        _ => left == right,
    }
}

fn constant_initializer(
    constant: ConstantValue,
    file_data: &BTreeMap<GlobalId, DataId>,
    static_data: &BTreeMap<(FullFunctionId, FullLocalId), (DataId, StorageDuration)>,
    unit: &FullTypedTranslationUnit,
    initializer_function: Option<FullFunctionId>,
    span: Span,
) -> Result<InitializerNodeKind, IrError> {
    Ok(match constant {
        ConstantValue::Signed(value) => InitializerNodeKind::Scalar(ScalarConstant::Signed(value)),
        ConstantValue::Unsigned(value) => {
            InitializerNodeKind::Scalar(ScalarConstant::Unsigned(value))
        }
        ConstantValue::Floating(value) => {
            InitializerNodeKind::Scalar(ScalarConstant::Floating(value))
        }
        ConstantValue::NullPointer => InitializerNodeKind::Scalar(ScalarConstant::NullPointer),
        ConstantValue::Address(address) => {
            let (target, kind) = match address.base {
                ccc_sema::generic::RelocatableBase::Global(global) => (
                    RelocationTarget::Object(*file_data.get(&global).ok_or_else(|| {
                        IrError::lower(
                            LOWERING_ERROR,
                            span,
                            format!("initializer references unknown global {}", global.0),
                        )
                    })?),
                    if unit
                        .globals
                        .get(global.0 as usize)
                        .is_some_and(|object| object.duration == StorageDuration::Thread)
                    {
                        RelocationKind::ThreadLocalAddress
                    } else {
                        RelocationKind::ObjectAddress
                    },
                ),
                ccc_sema::generic::RelocatableBase::BlockStatic { function, local } => {
                    let (data, duration) = static_data
                        .get(&(function, local))
                        .copied()
                        .ok_or_else(|| {
                            IrError::lower(
                                LOWERING_ERROR,
                                span,
                                format!(
                                    "initializer references unknown block-static object {}:{}",
                                    function.0, local.0
                                ),
                            )
                        })?;
                    (
                        RelocationTarget::Object(data),
                        if duration == StorageDuration::Thread {
                            RelocationKind::ThreadLocalAddress
                        } else {
                            RelocationKind::ObjectAddress
                        },
                    )
                }
                ccc_sema::generic::RelocatableBase::Function(function) => (
                    RelocationTarget::Function(function),
                    RelocationKind::FunctionAddress,
                ),
                ccc_sema::generic::RelocatableBase::String(string) => (
                    RelocationTarget::String(string),
                    RelocationKind::StringAddress,
                ),
                ccc_sema::generic::RelocatableBase::Label { function, label } => {
                    if initializer_function != Some(function)
                        || address.addend != 0
                        || address.one_past
                        || label.0 == u32::MAX
                    {
                        return Err(IrError::lower(
                            LOWERING_ERROR,
                            span,
                            "invalid cross-function or adjusted label address in static initializer",
                        ));
                    }
                    return Ok(InitializerNodeKind::Scalar(ScalarConstant::Unsigned(
                        u128::from(label.0) + 1,
                    )));
                }
            };
            InitializerNodeKind::Relocation {
                target,
                addend: address.addend,
                one_past: address.one_past,
                kind,
            }
        }
    })
}

fn initializer_path(path: &InitializerPathElement) -> InitializerPath {
    match path {
        InitializerPathElement::Index(index) => InitializerPath::Index(*index),
        InitializerPathElement::Field {
            index,
            name,
            bitfield,
        } => InitializerPath::Field {
            index: *index,
            name: name.clone(),
            bitfield: bitfield.as_ref().map(bitfield_descriptor),
        },
    }
}

#[derive(Clone)]
struct LocalFact {
    local: FullLocalId,
    name: String,
    ty: QualifiedType,
    duration: StorageDuration,
    requested_alignment: Option<u64>,
    span: Span,
    reasons: BTreeSet<MemoryResidencyReason>,
}

fn local_facts(
    function: &FullTypedFunction,
    types: &TypeStore,
) -> BTreeMap<FullLocalId, LocalFact> {
    let mut facts = BTreeMap::new();
    for parameter in &function.parameters {
        facts.insert(
            parameter.local,
            make_local_fact(
                parameter.local,
                parameter.name.clone(),
                parameter.ty,
                StorageDuration::Automatic,
                None,
                parameter.span,
                types,
            ),
        );
    }
    if let Some(body) = &function.body {
        collect_automatic_local_facts(body, types, &mut facts);
        scan_statement_for_address_taken(body, types, &mut facts);
        if contains_computed_goto(body) {
            for fact in facts.values_mut() {
                fact.reasons
                    .insert(MemoryResidencyReason::IndirectControlFlow);
            }
        }
    }
    facts
}

fn contains_computed_goto(statement: &FullTypedStatement) -> bool {
    use FullTypedStatementKind as S;
    match &statement.kind {
        S::ComputedGoto(_) => true,
        S::Label { statement, .. }
        | S::Case { statement, .. }
        | S::Default(statement)
        | S::Switch { statement, .. }
        | S::While { statement, .. }
        | S::DoWhile { statement, .. }
        | S::For { statement, .. } => contains_computed_goto(statement),
        S::Compound(items) => items.iter().any(|item| {
            matches!(
                item,
                FullTypedBlockItem::Statement(statement) if contains_computed_goto(statement)
            )
        }),
        S::If {
            then_statement,
            else_statement,
            ..
        } => {
            contains_computed_goto(then_statement)
                || else_statement
                    .as_deref()
                    .is_some_and(contains_computed_goto)
        }
        S::Expression(_) | S::Goto { .. } | S::Continue | S::Break | S::Return(_) => false,
    }
}

fn make_local_fact(
    local: FullLocalId,
    name: String,
    ty: QualifiedType,
    duration: StorageDuration,
    requested_alignment: Option<u64>,
    span: Span,
    types: &TypeStore,
) -> LocalFact {
    let mut reasons = BTreeSet::new();
    if ty.qualifiers.contains(TypeQualifiers::VOLATILE) {
        reasons.insert(MemoryResidencyReason::Volatile);
    }
    if ty.qualifiers.contains(TypeQualifiers::ATOMIC) {
        reasons.insert(MemoryResidencyReason::Atomic);
    }
    if is_aggregate(types, ty.ty) {
        reasons.insert(MemoryResidencyReason::Aggregate);
    }
    if variably_modified(types, ty.ty, &mut HashSet::new()) {
        reasons.insert(MemoryResidencyReason::VariablyModified);
    }
    LocalFact {
        local,
        name,
        ty,
        duration,
        requested_alignment,
        span,
        reasons,
    }
}

fn collect_automatic_local_facts(
    statement: &FullTypedStatement,
    types: &TypeStore,
    facts: &mut BTreeMap<FullLocalId, LocalFact>,
) {
    use FullTypedStatementKind as S;
    match &statement.kind {
        S::Label { statement, .. } | S::Case { statement, .. } => {
            collect_automatic_local_facts(statement, types, facts);
        }
        S::Default(statement) => collect_automatic_local_facts(statement, types, facts),
        S::Compound(items) => {
            for item in items {
                match item {
                    FullTypedBlockItem::Declaration(declaration)
                        if declaration.duration == StorageDuration::Automatic =>
                    {
                        facts.insert(
                            declaration.local,
                            make_local_fact(
                                declaration.local,
                                declaration.name.clone(),
                                declaration.ty,
                                declaration.duration,
                                declaration.requested_alignment,
                                declaration.span,
                                types,
                            ),
                        );
                    }
                    FullTypedBlockItem::Statement(statement) => {
                        collect_automatic_local_facts(statement, types, facts);
                    }
                    _ => {}
                }
            }
        }
        S::If {
            then_statement,
            else_statement,
            ..
        } => {
            collect_automatic_local_facts(then_statement, types, facts);
            if let Some(statement) = else_statement {
                collect_automatic_local_facts(statement, types, facts);
            }
        }
        S::For {
            initializer,
            statement,
            ..
        } => {
            if let FullTypedForInitializer::Declarations(items) = initializer {
                for item in items {
                    if let FullTypedBlockItem::Declaration(declaration) = item
                        && declaration.duration == StorageDuration::Automatic
                    {
                        facts.insert(
                            declaration.local,
                            make_local_fact(
                                declaration.local,
                                declaration.name.clone(),
                                declaration.ty,
                                declaration.duration,
                                declaration.requested_alignment,
                                declaration.span,
                                types,
                            ),
                        );
                    }
                }
            }
            collect_automatic_local_facts(statement, types, facts);
        }
        S::Switch { statement, .. } | S::While { statement, .. } | S::DoWhile { statement, .. } => {
            collect_automatic_local_facts(statement, types, facts);
        }
        S::Expression(_)
        | S::Goto { .. }
        | S::ComputedGoto(_)
        | S::Continue
        | S::Break
        | S::Return(_) => {}
    }
}

fn variably_modified(types: &TypeStore, ty: TypeId, active: &mut HashSet<TypeId>) -> bool {
    if !active.insert(ty) {
        return false;
    }
    let result = match types.try_kind(ty) {
        Some(TypeKind::Array(array)) => {
            matches!(
                array.length,
                ArrayLength::Variable(_) | ArrayLength::UnspecifiedVariable(_)
            ) || variably_modified(types, array.element.ty, active)
        }
        Some(TypeKind::Pointer(pointer)) => variably_modified(types, pointer.pointee.ty, active),
        Some(TypeKind::Function(signature)) => {
            variably_modified(types, signature.result.ty, active)
                || match &signature.parameters {
                    FunctionParameters::Unspecified => false,
                    FunctionParameters::Prototype(parameters) => parameters
                        .iter()
                        .any(|parameter| variably_modified(types, parameter.ty, active)),
                }
        }
        _ => false,
    };
    active.remove(&ty);
    result
}

fn scan_statement_for_address_taken(
    statement: &FullTypedStatement,
    types: &TypeStore,
    facts: &mut BTreeMap<FullLocalId, LocalFact>,
) {
    use FullTypedStatementKind as S;
    match &statement.kind {
        S::Label { statement, .. } | S::Case { statement, .. } => {
            scan_statement_for_address_taken(statement, types, facts);
        }
        S::Default(statement) => scan_statement_for_address_taken(statement, types, facts),
        S::Compound(items) => {
            for item in items {
                match item {
                    FullTypedBlockItem::Declaration(declaration) => {
                        if let Some(initializer) = &declaration.initializer {
                            scan_initializer_for_address_taken(initializer, types, facts);
                        }
                    }
                    FullTypedBlockItem::Statement(statement) => {
                        scan_statement_for_address_taken(statement, types, facts);
                    }
                    _ => {}
                }
            }
        }
        S::Expression(expression) => {
            if let Some(expression) = expression {
                scan_expression_for_address_taken(expression, types, facts);
            }
        }
        S::If {
            condition,
            then_statement,
            else_statement,
        } => {
            scan_expression_for_address_taken(condition, types, facts);
            scan_statement_for_address_taken(then_statement, types, facts);
            if let Some(statement) = else_statement {
                scan_statement_for_address_taken(statement, types, facts);
            }
        }
        S::Switch {
            expression,
            statement,
        } => {
            scan_expression_for_address_taken(expression, types, facts);
            scan_statement_for_address_taken(statement, types, facts);
        }
        S::While {
            condition,
            statement,
        } => {
            scan_expression_for_address_taken(condition, types, facts);
            scan_statement_for_address_taken(statement, types, facts);
        }
        S::DoWhile {
            statement,
            condition,
        } => {
            scan_statement_for_address_taken(statement, types, facts);
            scan_expression_for_address_taken(condition, types, facts);
        }
        S::For {
            initializer,
            condition,
            step,
            statement,
        } => {
            match initializer {
                FullTypedForInitializer::Empty => {}
                FullTypedForInitializer::Expression(expression) => {
                    scan_expression_for_address_taken(expression, types, facts);
                }
                FullTypedForInitializer::Declarations(items) => {
                    for item in items {
                        if let FullTypedBlockItem::Declaration(declaration) = item
                            && let Some(initializer) = &declaration.initializer
                        {
                            scan_initializer_for_address_taken(initializer, types, facts);
                        }
                    }
                }
            }
            if let Some(condition) = condition {
                scan_expression_for_address_taken(condition, types, facts);
            }
            if let Some(step) = step {
                scan_expression_for_address_taken(step, types, facts);
            }
            scan_statement_for_address_taken(statement, types, facts);
        }
        S::Return(expression) => {
            if let Some(expression) = expression {
                scan_expression_for_address_taken(expression, types, facts);
            }
        }
        S::ComputedGoto(expression) => scan_expression_for_address_taken(expression, types, facts),
        S::Goto { .. } | S::Continue | S::Break => {}
    }
}

fn scan_initializer_for_address_taken(
    initializer: &FullTypedInitializer,
    types: &TypeStore,
    facts: &mut BTreeMap<FullLocalId, LocalFact>,
) {
    match &initializer.kind {
        FullTypedInitializerKind::Scalar(expression) => {
            scan_expression_for_address_taken(expression, types, facts);
        }
        FullTypedInitializerKind::Aggregate(entries) => {
            for entry in entries {
                scan_initializer_for_address_taken(&entry.initializer, types, facts);
            }
        }
        FullTypedInitializerKind::String(_) | FullTypedInitializerKind::Zero => {}
    }
}

fn scan_expression_for_address_taken(
    expression: &FullTypedExpression,
    types: &TypeStore,
    facts: &mut BTreeMap<FullLocalId, LocalFact>,
) {
    use FullTypedExpressionKind as E;
    match &expression.kind {
        E::AddressOf(operand) => {
            if let Some(place) = &operand.place {
                mark_place_address_taken(place, facts);
            }
            scan_expression_for_address_taken(operand, types, facts);
        }
        E::Conversion { expression, .. }
        | E::Unary {
            operand: expression,
            ..
        }
        | E::Dereference(expression) => {
            scan_expression_for_address_taken(expression, types, facts);
        }
        E::Binary { left, right, .. }
        | E::Subscript {
            base: left,
            index: right,
        } => {
            scan_expression_for_address_taken(left, types, facts);
            scan_expression_for_address_taken(right, types, facts);
        }
        E::Member { base, .. } => scan_expression_for_address_taken(base, types, facts),
        E::CompoundLiteral { local, initializer } => {
            let mut fact = make_local_fact(
                *local,
                format!("<compound-literal-{}>", local.0),
                expression.ty,
                StorageDuration::Automatic,
                None,
                expression.span,
                types,
            );
            fact.reasons.insert(MemoryResidencyReason::AddressTaken);
            facts.insert(*local, fact);
            scan_initializer_for_address_taken(initializer, types, facts);
        }
        E::Assignment { target, value, .. } => {
            scan_expression_for_address_taken(target, types, facts);
            scan_expression_for_address_taken(value, types, facts);
        }
        E::Increment { operand, .. } => scan_expression_for_address_taken(operand, types, facts),
        E::Call {
            callee, arguments, ..
        } => {
            scan_expression_for_address_taken(callee, types, facts);
            for argument in arguments {
                scan_expression_for_address_taken(argument, types, facts);
            }
        }
        E::Conditional {
            condition,
            then_expression,
            else_expression,
        } => {
            scan_expression_for_address_taken(condition, types, facts);
            scan_expression_for_address_taken(then_expression, types, facts);
            scan_expression_for_address_taken(else_expression, types, facts);
        }
        E::Comma(expressions) => {
            for expression in expressions {
                scan_expression_for_address_taken(expression, types, facts);
            }
        }
        E::BuiltinExpect { value, expected: _ } => {
            scan_expression_for_address_taken(value, types, facts);
        }
        E::IntegerIntrinsic { operand, .. } => {
            scan_expression_for_address_taken(operand, types, facts);
        }
        E::Prefetch { address, .. } => {
            scan_expression_for_address_taken(address, types, facts);
        }
        E::AtomicReadModifyWrite {
            pointer, operand, ..
        } => {
            scan_expression_for_address_taken(pointer, types, facts);
            scan_expression_for_address_taken(operand, types, facts);
        }
        E::AtomicCompareExchange {
            pointer,
            expected,
            replacement,
            ..
        } => {
            scan_expression_for_address_taken(pointer, types, facts);
            scan_expression_for_address_taken(expected, types, facts);
            scan_expression_for_address_taken(replacement, types, facts);
        }
        E::VaStart { list, .. } | E::VaArg { list, .. } | E::VaEnd { list } => {
            scan_expression_for_address_taken(list, types, facts);
            if let Some(place) = &list.place {
                mark_place_address_taken(place, facts);
            }
        }
        E::VaCopy {
            destination,
            source,
        } => {
            scan_expression_for_address_taken(destination, types, facts);
            scan_expression_for_address_taken(source, types, facts);
            if let Some(place) = &destination.place {
                mark_place_address_taken(place, facts);
            }
            if let Some(place) = &source.place {
                mark_place_address_taken(place, facts);
            }
        }
        E::Constant(_)
        | E::StringLiteral(_)
        | E::DeclRef(_)
        | E::Sizeof { .. }
        | E::Alignof { .. }
        | E::Offsetof { .. }
        | E::MemoryFence { .. } => {}
    }
}

fn mark_place_address_taken(place: &Place, facts: &mut BTreeMap<FullLocalId, LocalFact>) {
    if let PlaceBase::Local(local) | PlaceBase::CompoundLiteral(local) = place.base
        && let Some(fact) = facts.get_mut(&local)
    {
        fact.reasons.insert(MemoryResidencyReason::AddressTaken);
    }
}

#[derive(Clone)]
struct LoweredPlace {
    address: ValueId,
    object: QualifiedType,
    access: MemoryAccess,
    bitfield: Option<BitfieldDescriptor>,
}

enum PendingAggregateProjection<'a> {
    Field {
        index: usize,
        name: Option<&'a str>,
        bitfield: Option<BitfieldDescriptor>,
    },
    Index {
        index: &'a FullTypedExpression,
    },
}

fn collect_aggregate_projection<'a>(
    expression: &'a FullTypedExpression,
    projections: &mut Vec<PendingAggregateProjection<'a>>,
) -> Option<&'a FullTypedExpression> {
    match &expression.kind {
        FullTypedExpressionKind::Member {
            base,
            field_index,
            name,
            indirect: false,
            bitfield,
        } if base.place.is_none() => {
            let root = collect_aggregate_projection(base, projections).unwrap_or(base);
            projections.push(PendingAggregateProjection::Field {
                index: *field_index,
                name: name.as_deref(),
                bitfield: bitfield.as_deref().map(bitfield_descriptor),
            });
            Some(root)
        }
        FullTypedExpressionKind::Subscript { base, index } => {
            let FullTypedExpressionKind::Conversion {
                kind: ConversionKind::ArrayToPointer,
                expression: array,
            } = &base.kind
            else {
                return None;
            };
            let root = collect_aggregate_projection(array, projections)?;
            projections.push(PendingAggregateProjection::Index { index });
            Some(root)
        }
        _ => None,
    }
}

struct SwitchContext {
    cases: VecDeque<(i128, BlockId)>,
    default: Option<BlockId>,
}

struct FunctionBuilder<'a> {
    unit: &'a FullTypedTranslationUnit,
    file_data: &'a BTreeMap<GlobalId, DataId>,
    static_data: &'a BTreeMap<(FullFunctionId, FullLocalId), (DataId, StorageDuration)>,
    types: &'a mut TypeStore,
    function: FullFunction,
    current: Option<BlockId>,
    storage_by_local: BTreeMap<FullLocalId, StorageId>,
    label_blocks: BTreeMap<LabelId, BlockId>,
    break_targets: Vec<BlockId>,
    continue_targets: Vec<BlockId>,
    switches: Vec<SwitchContext>,
}

impl<'a> FunctionBuilder<'a> {
    fn lower(
        source: &FullTypedFunction,
        unit: &'a FullTypedTranslationUnit,
        file_data: &'a BTreeMap<GlobalId, DataId>,
        static_data: &'a BTreeMap<(FullFunctionId, FullLocalId), (DataId, StorageDuration)>,
        types: &'a mut TypeStore,
    ) -> Result<FullFunction, IrError> {
        if let Some(parameter) = source
            .parameters
            .iter()
            .find(|parameter| !parameter.variable_length_bounds.is_empty())
        {
            return Err(IrError::lower(
                LOWERING_ERROR,
                parameter.span,
                "runtime variable-length parameter bounds are not yet lowered",
            ));
        }
        let signature = types.function_signature(source.signature).ok_or_else(|| {
            IrError::lower(
                LOWERING_ERROR,
                source.span,
                format!("function `{}` has a non-function signature", source.name),
            )
        })?;
        let facts = if source.body.is_some() {
            local_facts(source, types)
        } else {
            BTreeMap::new()
        };
        let mut storage = Vec::new();
        let mut storage_by_local = BTreeMap::new();
        for fact in facts.values() {
            let id = StorageId(storage.len() as u32);
            storage_by_local.insert(fact.local, id);
            storage.push(FullStorage {
                id,
                local: fact.local,
                name: fact.name.clone(),
                ty: fact.ty,
                duration: fact.duration,
                location: match fact.duration {
                    StorageDuration::Automatic => StorageLocation::Automatic,
                    StorageDuration::Static => StorageLocation::Static,
                    StorageDuration::Thread => StorageLocation::ThreadLocal,
                },
                requested_alignment: fact.requested_alignment,
                required_by: fact.reasons.iter().copied().collect(),
                span: fact.span,
            });
        }
        let parameters = source
            .parameters
            .iter()
            .map(|parameter| FullParameter {
                local: parameter.local,
                name: parameter.name.clone(),
                ty: parameter.ty,
                incoming: None,
                storage: storage_by_local.get(&parameter.local).copied(),
                span: parameter.span,
            })
            .collect();
        let symbol_name = source
            .asm_label
            .as_ref()
            .map_or_else(|| source.name.clone(), |label| label.symbol.clone());
        let mut builder = Self {
            unit,
            file_data,
            static_data,
            types,
            function: FullFunction {
                id: source.id,
                name: source.name.clone(),
                signature: source.signature,
                storage_class: source.storage,
                linkage: source.linkage,
                binding: source.binding,
                visibility: source.visibility,
                properties: source.properties,
                symbol_name,
                result_type: signature.result,
                parameters,
                storage,
                blocks: Vec::new(),
                entry: None,
                value_types: Vec::new(),
                instruction_count: 0,
                span: source.span,
            },
            current: None,
            storage_by_local,
            label_blocks: BTreeMap::new(),
            break_targets: Vec::new(),
            continue_targets: Vec::new(),
            switches: Vec::new(),
        };
        let Some(body) = &source.body else {
            return Ok(builder.function);
        };

        let mut labels = Vec::new();
        collect_labels(body, &mut labels);
        for label in labels {
            let block = builder.new_block();
            builder.label_blocks.insert(label, block);
        }

        let entry = builder.new_block();
        builder.function.entry = Some(entry);
        builder.current = Some(entry);
        for index in 0..builder.function.parameters.len() {
            let ty = builder.function.parameters[index].ty;
            let span = builder.function.parameters[index].span;
            let incoming = builder.add_block_parameter(entry, ty);
            builder.function.parameters[index].incoming = Some(incoming);
            if let Some(storage) = builder.function.parameters[index].storage {
                let address = builder.address_of_storage(storage, span)?;
                if is_aggregate(builder.types, ty.ty) {
                    builder.emit_effect(
                        FullInstructionKind::AggregateCopy {
                            destination: address,
                            source: incoming,
                            destination_object: ty,
                            source_object: QualifiedType::unqualified(ty.ty),
                            destination_access: access_from_qualified(ty),
                            source_access: MemoryAccess::default(),
                            overlap: AggregateOverlap::MayOverlap,
                        },
                        span,
                    )?;
                } else {
                    builder.emit_effect(
                        FullInstructionKind::Store {
                            address,
                            value: incoming,
                            object: ty,
                            access: access_from_qualified(ty),
                        },
                        span,
                    )?;
                }
            }
        }

        builder.statement(body)?;
        if builder.current.is_some() {
            if is_void(builder.types, builder.function.result_type.ty) {
                builder.terminate(FullTerminator::Return(None))?;
            } else if !is_aggregate(builder.types, builder.function.result_type.ty) {
                // Reaching the end of main returns zero. Other scalar-return
                // functions have an indeterminate result in C when that value
                // is used; choosing zero keeps the IR deterministic without
                // rejecting programs that discard it.
                let value = builder.zero_value(builder.function.result_type, body.span)?;
                builder.terminate(FullTerminator::Return(Some(value)))?;
            } else {
                // Reaching the closing brace of a non-void function other than
                // `main` has undefined behavior when the result is used.
                builder.terminate(FullTerminator::Unreachable)?;
            }
        }
        for block in &mut builder.function.blocks {
            if block.terminator.is_none() {
                block.terminator = Some(FullTerminator::Unreachable);
            }
        }
        promote_scalar_locals(&mut builder.function, builder.types)?;
        Ok(builder.function)
    }

    fn new_block(&mut self) -> BlockId {
        let id = BlockId(self.function.blocks.len() as u32);
        self.function.blocks.push(FullBlock {
            id,
            parameters: Vec::new(),
            instructions: Vec::new(),
            terminator: None,
        });
        id
    }

    fn add_block_parameter(&mut self, block: BlockId, ty: QualifiedType) -> ValueId {
        let value = ValueId(self.function.value_types.len() as u32);
        self.function.value_types.push(ty.ty);
        self.function.blocks[block.0 as usize]
            .parameters
            .push(value);
        value
    }

    fn emit_result(
        &mut self,
        kind: FullInstructionKind,
        ty: QualifiedType,
        span: Span,
    ) -> Result<ValueId, IrError> {
        let value = ValueId(self.function.value_types.len() as u32);
        self.function.value_types.push(ty.ty);
        self.emit(Some(value), kind, span)?;
        Ok(value)
    }

    fn emit_effect(&mut self, kind: FullInstructionKind, span: Span) -> Result<(), IrError> {
        self.emit(None, kind, span)
    }

    fn emit(
        &mut self,
        result: Option<ValueId>,
        kind: FullInstructionKind,
        span: Span,
    ) -> Result<(), IrError> {
        let Some(block) = self.current else {
            return Err(IrError::lower(
                LOWERING_ERROR,
                span,
                "attempted to emit an instruction without a current CFG block",
            ));
        };
        let id = InstructionId(self.function.instruction_count);
        self.function.instruction_count = self
            .function
            .instruction_count
            .checked_add(1)
            .expect("instruction id space exhausted");
        self.function.blocks[block.0 as usize]
            .instructions
            .push(FullInstruction {
                id,
                result,
                kind,
                span,
            });
        Ok(())
    }

    fn terminate(&mut self, terminator: FullTerminator) -> Result<(), IrError> {
        let block = self.current.take().ok_or_else(|| {
            IrError::verify("attempted to terminate a function without a current block")
        })?;
        let slot = &mut self.function.blocks[block.0 as usize].terminator;
        if slot.is_some() {
            return Err(IrError::verify(format!(
                "block {} received more than one terminator",
                block.0
            )));
        }
        *slot = Some(terminator);
        Ok(())
    }

    fn branch(&mut self, target: BlockId) -> Result<(), IrError> {
        self.terminate(FullTerminator::Branch(FullEdge {
            target,
            arguments: Vec::new(),
        }))
    }

    fn activate(&mut self, block: BlockId) -> Result<(), IrError> {
        if self.current == Some(block) {
            return Ok(());
        }
        if self.current.is_some() {
            self.branch(block)?;
        }
        self.current = Some(block);
        Ok(())
    }

    fn instruction_root(&mut self) -> BlockId {
        if let Some(block) = self.current {
            block
        } else {
            let block = self.new_block();
            self.current = Some(block);
            block
        }
    }

    fn address_of_storage(&mut self, storage: StorageId, span: Span) -> Result<ValueId, IrError> {
        let object = self
            .function
            .storage
            .get(storage.0 as usize)
            .ok_or_else(|| IrError::lower(LOWERING_ERROR, span, "unknown local storage object"))?
            .ty;
        let pointer = self.types.pointer(object);
        self.emit_result(
            FullInstructionKind::AddressOfStorage { storage },
            QualifiedType::unqualified(pointer),
            span,
        )
    }
}

/// Promotes automatic scalar objects that do not require an address to SSA.
/// Every non-entry block receives one parameter per promoted object, which is
/// deliberately simple and correct for arbitrary structured control flow,
/// switches, and gotos. A later sparsification pass can remove redundant block
/// parameters without changing the memory-residency contract.
fn promote_scalar_locals(function: &mut FullFunction, types: &TypeStore) -> Result<(), IrError> {
    let Some(entry) = function.entry else {
        return Ok(());
    };
    let eligible = function
        .storage
        .iter()
        .filter(|storage| {
            storage.location == StorageLocation::Automatic && storage.required_by.is_empty()
        })
        .map(|storage| (storage.id, storage.ty.ty))
        .collect::<Vec<_>>();
    if eligible.is_empty() {
        return Ok(());
    }
    let eligible_ids = eligible
        .iter()
        .map(|(storage, _)| *storage)
        .collect::<BTreeSet<_>>();

    // An indeterminate scalar may take any representable value. Giving it a
    // deterministic zero seed avoids inventing an address while keeping uses
    // of an uninitialized object in the source program undefined.
    let mut initial_values = function
        .parameters
        .iter()
        .filter_map(|parameter| Some((parameter.storage?, parameter.incoming?)))
        .filter(|(storage, _)| eligible_ids.contains(storage))
        .collect::<BTreeMap<_, _>>();
    let mut zero_values = BTreeMap::new();
    let mut zero_instructions = Vec::with_capacity(eligible.len());
    for (storage, ty) in &eligible {
        if initial_values.contains_key(storage) {
            continue;
        }
        let value = ValueId(function.value_types.len() as u32);
        function.value_types.push(*ty);
        zero_values.insert(*storage, value);
        initial_values.insert(*storage, value);
        zero_instructions.push(FullInstruction {
            id: InstructionId(function.instruction_count),
            result: Some(value),
            kind: FullInstructionKind::Constant(zero_scalar(types, *ty)?),
            span: function.span,
        });
        function.instruction_count += 1;
    }
    function.blocks[entry.0 as usize]
        .instructions
        .splice(0..0, zero_instructions);

    let mut block_values = BTreeMap::<BlockId, BTreeMap<StorageId, ValueId>>::new();
    for block in &mut function.blocks {
        if block.id == entry {
            block_values.insert(block.id, initial_values.clone());
            continue;
        }
        let mut values = BTreeMap::new();
        for (storage, ty) in &eligible {
            let value = ValueId(function.value_types.len() as u32);
            function.value_types.push(*ty);
            block.parameters.push(value);
            values.insert(*storage, value);
        }
        block_values.insert(block.id, values);
    }

    let mut address_storage = BTreeMap::<ValueId, StorageId>::new();
    let mut aliases = BTreeMap::<ValueId, ValueId>::new();
    for block in &mut function.blocks {
        let mut current = block_values
            .remove(&block.id)
            .expect("every block received promoted values");
        let mut retained = Vec::with_capacity(block.instructions.len());
        for instruction in std::mem::take(&mut block.instructions) {
            match &instruction.kind {
                FullInstructionKind::AddressOfStorage { storage }
                    if eligible_ids.contains(storage) =>
                {
                    let result = instruction.result.ok_or_else(|| {
                        IrError::verify("promotable storage address has no SSA result")
                    })?;
                    address_storage.insert(result, *storage);
                }
                FullInstructionKind::Load {
                    address,
                    object,
                    access,
                } if address_storage.contains_key(address) => {
                    let storage = address_storage[address];
                    if !eligible_ids.contains(&storage) {
                        retained.push(instruction);
                        continue;
                    }
                    if object.ty != eligible_type(&eligible, storage)? || access_is_ordered(*access)
                    {
                        return Err(IrError::verify(
                            "ordered or type-changing access reached scalar SSA promotion",
                        ));
                    }
                    let result = instruction.result.ok_or_else(|| {
                        IrError::verify("promotable local load has no SSA result")
                    })?;
                    let value = resolve_alias(current[&storage], &aliases)?;
                    aliases.insert(result, value);
                }
                FullInstructionKind::Store {
                    address,
                    value,
                    object,
                    access,
                } if address_storage.contains_key(address) => {
                    let storage = address_storage[address];
                    if !eligible_ids.contains(&storage) {
                        retained.push(instruction);
                        continue;
                    }
                    if object.ty != eligible_type(&eligible, storage)? || access_is_ordered(*access)
                    {
                        return Err(IrError::verify(
                            "ordered or type-changing access reached scalar SSA promotion",
                        ));
                    }
                    current.insert(storage, resolve_alias(*value, &aliases)?);
                }
                FullInstructionKind::ZeroInitialize {
                    destination,
                    object,
                } if address_storage.contains_key(destination) => {
                    let storage = address_storage[destination];
                    if !eligible_ids.contains(&storage) {
                        retained.push(instruction);
                        continue;
                    }
                    if object.ty != eligible_type(&eligible, storage)? {
                        return Err(IrError::verify(
                            "type-changing zero initialization reached scalar SSA promotion",
                        ));
                    }
                    let zero = zero_values.get(&storage).copied().ok_or_else(|| {
                        IrError::verify("parameter storage received aggregate zero initialization")
                    })?;
                    current.insert(storage, zero);
                }
                _ => retained.push(instruction),
            }
        }
        block.instructions = retained;
        if let Some(terminator) = &mut block.terminator {
            for edge in terminator_edges_mut(terminator) {
                if edge.target == entry {
                    return Err(IrError::verify(
                        "control flow branches to the function entry block",
                    ));
                }
                for (storage, _) in &eligible {
                    edge.arguments
                        .push(resolve_alias(current[storage], &aliases)?);
                }
            }
        }
    }

    let mut storage_remap = BTreeMap::new();
    let mut retained_storage = Vec::new();
    for mut storage in std::mem::take(&mut function.storage) {
        if eligible_ids.contains(&storage.id) {
            continue;
        }
        let old = storage.id;
        storage.id = StorageId(retained_storage.len() as u32);
        storage_remap.insert(old, storage.id);
        retained_storage.push(storage);
    }
    function.storage = retained_storage;
    for parameter in &mut function.parameters {
        if let Some(storage) = parameter.storage {
            parameter.storage = storage_remap.get(&storage).copied();
        }
    }
    for block in &mut function.blocks {
        for instruction in &mut block.instructions {
            if let FullInstructionKind::AddressOfStorage { storage } = &mut instruction.kind {
                *storage = *storage_remap.get(storage).ok_or_else(|| {
                    IrError::verify("a promoted storage address escaped SSA construction")
                })?;
            }
        }
    }

    compact_values(function, &aliases)
}

fn zero_scalar(types: &TypeStore, ty: TypeId) -> Result<ScalarConstant, IrError> {
    if pointer_pointee(types, ty).is_some() {
        Ok(ScalarConstant::NullPointer)
    } else if types
        .builtin_type(ty)
        .is_some_and(|builtin| builtin.is_floating())
    {
        Ok(ScalarConstant::Floating(0.0))
    } else if types.is_integer(ty) {
        Ok(ScalarConstant::Signed(0))
    } else {
        Err(IrError::verify(
            "a non-scalar automatic object was selected for SSA promotion",
        ))
    }
}

fn eligible_type(eligible: &[(StorageId, TypeId)], storage: StorageId) -> Result<TypeId, IrError> {
    eligible
        .iter()
        .find_map(|(candidate, ty)| (*candidate == storage).then_some(*ty))
        .ok_or_else(|| IrError::verify("unknown promotable storage object"))
}

fn access_is_ordered(access: MemoryAccess) -> bool {
    access.volatile || access.atomic.is_some() || access.non_elidable || access.non_movable
}

fn resolve_alias(
    mut value: ValueId,
    aliases: &BTreeMap<ValueId, ValueId>,
) -> Result<ValueId, IrError> {
    let mut seen = BTreeSet::new();
    while let Some(next) = aliases.get(&value).copied() {
        if !seen.insert(value) {
            return Err(IrError::verify("cyclic SSA alias during scalar promotion"));
        }
        value = next;
    }
    Ok(value)
}

fn compact_values(
    function: &mut FullFunction,
    aliases: &BTreeMap<ValueId, ValueId>,
) -> Result<(), IrError> {
    let old_types = function.value_types.clone();
    let mut remap = BTreeMap::<ValueId, ValueId>::new();
    let mut value_types = Vec::new();
    for block in &function.blocks {
        for old in block.parameters.iter().copied().chain(
            block
                .instructions
                .iter()
                .filter_map(|instruction| instruction.result),
        ) {
            let ty = *old_types
                .get(old.0 as usize)
                .ok_or_else(|| IrError::verify("SSA definition has no type during compaction"))?;
            let new = ValueId(value_types.len() as u32);
            if remap.insert(old, new).is_some() {
                return Err(IrError::verify(
                    "SSA definition was encountered twice during compaction",
                ));
            }
            value_types.push(ty);
        }
    }

    for parameter in &mut function.parameters {
        if let Some(incoming) = &mut parameter.incoming {
            remap_value(incoming, aliases, &remap)?;
        }
    }
    let mut instruction_count = 0u32;
    for block in &mut function.blocks {
        for parameter in &mut block.parameters {
            remap_value(parameter, aliases, &remap)?;
        }
        for instruction in &mut block.instructions {
            if let Some(result) = &mut instruction.result {
                *result = remap[&*result];
            }
            remap_instruction_values(&mut instruction.kind, aliases, &remap)?;
            instruction.id = InstructionId(instruction_count);
            instruction_count = instruction_count
                .checked_add(1)
                .ok_or_else(|| IrError::verify("instruction id space exhausted"))?;
        }
        if let Some(terminator) = &mut block.terminator {
            remap_terminator_values(terminator, aliases, &remap)?;
        }
    }
    function.value_types = value_types;
    function.instruction_count = instruction_count;
    Ok(())
}

fn remap_value(
    value: &mut ValueId,
    aliases: &BTreeMap<ValueId, ValueId>,
    remap: &BTreeMap<ValueId, ValueId>,
) -> Result<(), IrError> {
    let resolved = resolve_alias(*value, aliases)?;
    *value = *remap.get(&resolved).ok_or_else(|| {
        IrError::verify(format!(
            "removed SSA value {} remains in promoted IR",
            resolved.0
        ))
    })?;
    Ok(())
}

fn remap_edge_values(
    edge: &mut FullEdge,
    aliases: &BTreeMap<ValueId, ValueId>,
    remap: &BTreeMap<ValueId, ValueId>,
) -> Result<(), IrError> {
    for argument in &mut edge.arguments {
        remap_value(argument, aliases, remap)?;
    }
    Ok(())
}

fn remap_instruction_values(
    kind: &mut FullInstructionKind,
    aliases: &BTreeMap<ValueId, ValueId>,
    remap: &BTreeMap<ValueId, ValueId>,
) -> Result<(), IrError> {
    let map = |value: &mut ValueId| remap_value(value, aliases, remap);
    match kind {
        FullInstructionKind::Constant(_)
        | FullInstructionKind::AddressConstant { .. }
        | FullInstructionKind::AddressOfGlobal { .. }
        | FullInstructionKind::AddressOfFunction { .. }
        | FullInstructionKind::AddressOfString { .. }
        | FullInstructionKind::AddressOfStorage { .. }
        | FullInstructionKind::MemoryFence { .. } => {}
        FullInstructionKind::ProjectField { base, .. } => map(base)?,
        FullInstructionKind::PointerOffset { base, index, .. } => {
            map(base)?;
            map(index)?;
        }
        FullInstructionKind::PointerDifference { left, right, .. }
        | FullInstructionKind::Binary { left, right, .. } => {
            map(left)?;
            map(right)?;
        }
        FullInstructionKind::Load { address, .. }
        | FullInstructionKind::BitfieldLoad { address, .. }
        | FullInstructionKind::AggregateSnapshot {
            source: address, ..
        } => map(address)?,
        FullInstructionKind::AggregateProject {
            base, projections, ..
        } => {
            map(base)?;
            for projection in projections {
                if let AggregateProjection::Index { index } = projection {
                    map(index)?;
                }
            }
        }
        FullInstructionKind::Store { address, value, .. }
        | FullInstructionKind::BitfieldStore { address, value, .. } => {
            map(address)?;
            map(value)?;
        }
        FullInstructionKind::AtomicReadModifyWrite {
            address, operand, ..
        } => {
            map(address)?;
            map(operand)?;
        }
        FullInstructionKind::AtomicCompareExchange {
            address,
            expected,
            replacement,
            ..
        } => {
            map(address)?;
            map(expected)?;
            map(replacement)?;
        }
        FullInstructionKind::Prefetch { address, .. } => map(address)?,
        FullInstructionKind::ZeroInitialize { destination, .. }
        | FullInstructionKind::StringInitialize { destination, .. } => map(destination)?,
        FullInstructionKind::AggregateCopy {
            destination,
            source,
            ..
        } => {
            map(destination)?;
            map(source)?;
        }
        FullInstructionKind::Convert { operand, .. }
        | FullInstructionKind::Unary { operand, .. }
        | FullInstructionKind::IntegerIntrinsic { operand, .. } => map(operand)?,
        FullInstructionKind::DirectCall { arguments, .. } => {
            for argument in arguments {
                map(argument)?;
            }
        }
        FullInstructionKind::IndirectCall {
            callee, arguments, ..
        } => {
            map(callee)?;
            for argument in arguments {
                map(argument)?;
            }
        }
        FullInstructionKind::VaStart { list, .. }
        | FullInstructionKind::VaArg { list, .. }
        | FullInstructionKind::VaEnd { list } => map(list)?,
        FullInstructionKind::VaCopy {
            destination,
            source,
        } => {
            map(destination)?;
            map(source)?;
        }
    }
    Ok(())
}

fn remap_terminator_values(
    terminator: &mut FullTerminator,
    aliases: &BTreeMap<ValueId, ValueId>,
    remap: &BTreeMap<ValueId, ValueId>,
) -> Result<(), IrError> {
    match terminator {
        FullTerminator::Branch(edge) => remap_edge_values(edge, aliases, remap)?,
        FullTerminator::Conditional {
            condition,
            then_edge,
            else_edge,
        } => {
            remap_value(condition, aliases, remap)?;
            remap_edge_values(then_edge, aliases, remap)?;
            remap_edge_values(else_edge, aliases, remap)?;
        }
        FullTerminator::Switch {
            selector,
            cases,
            default,
        } => {
            remap_value(selector, aliases, remap)?;
            for case in cases {
                remap_edge_values(&mut case.edge, aliases, remap)?;
            }
            remap_edge_values(default, aliases, remap)?;
        }
        FullTerminator::IndirectBranch { selector, targets } => {
            remap_value(selector, aliases, remap)?;
            for target in targets {
                remap_edge_values(target, aliases, remap)?;
            }
        }
        FullTerminator::Return(value) => {
            if let Some(value) = value {
                remap_value(value, aliases, remap)?;
            }
        }
        FullTerminator::Unreachable => {}
    }
    Ok(())
}

fn terminator_edges_mut(terminator: &mut FullTerminator) -> Vec<&mut FullEdge> {
    match terminator {
        FullTerminator::Branch(edge) => vec![edge],
        FullTerminator::Conditional {
            then_edge,
            else_edge,
            ..
        } => vec![then_edge, else_edge],
        FullTerminator::Switch { cases, default, .. } => cases
            .iter_mut()
            .map(|case| &mut case.edge)
            .chain(std::iter::once(default))
            .collect(),
        FullTerminator::IndirectBranch { targets, .. } => targets.iter_mut().collect(),
        FullTerminator::Return(_) | FullTerminator::Unreachable => Vec::new(),
    }
}

impl FunctionBuilder<'_> {
    fn statement(&mut self, statement: &FullTypedStatement) -> Result<(), IrError> {
        use FullTypedStatementKind as S;
        match &statement.kind {
            S::Label {
                label,
                statement: nested,
                ..
            } => {
                let block = *self.label_blocks.get(label).ok_or_else(|| {
                    IrError::lower(
                        LOWERING_ERROR,
                        statement.span,
                        format!("label {} has no preallocated block", label.0),
                    )
                })?;
                self.activate(block)?;
                self.statement(nested)?;
            }
            S::Case {
                value,
                statement: nested,
            } => {
                let context = self.switches.last_mut().ok_or_else(|| {
                    IrError::lower(LOWERING_ERROR, statement.span, "case outside a switch")
                })?;
                let position = context
                    .cases
                    .iter()
                    .position(|(candidate, _)| candidate == value)
                    .ok_or_else(|| {
                        IrError::lower(
                            LOWERING_ERROR,
                            statement.span,
                            format!("case value {value} has no switch dispatch edge"),
                        )
                    })?;
                let (_, block) = context
                    .cases
                    .remove(position)
                    .expect("case position was checked");
                self.activate(block)?;
                self.statement(nested)?;
            }
            S::Default(nested) => {
                let block = self
                    .switches
                    .last_mut()
                    .and_then(|context| context.default.take())
                    .ok_or_else(|| {
                        IrError::lower(
                            LOWERING_ERROR,
                            statement.span,
                            "default label has no switch dispatch edge",
                        )
                    })?;
                self.activate(block)?;
                self.statement(nested)?;
            }
            S::Compound(items) => {
                for item in items {
                    match item {
                        FullTypedBlockItem::Declaration(declaration) => {
                            self.local_declaration(declaration)?;
                        }
                        FullTypedBlockItem::Statement(statement) => self.statement(statement)?,
                        FullTypedBlockItem::Typedef(_)
                        | FullTypedBlockItem::ExternalObject(_)
                        | FullTypedBlockItem::FunctionDeclaration(_)
                        | FullTypedBlockItem::StaticAssert { .. }
                        | FullTypedBlockItem::Pragma(_) => {}
                    }
                }
            }
            S::Expression(expression) => {
                if self.current.is_some()
                    && let Some(expression) = expression
                {
                    let _ = self.expression(expression)?;
                }
            }
            S::If {
                condition,
                then_statement,
                else_statement,
            } => self.if_statement(condition, then_statement, else_statement.as_deref())?,
            S::Switch {
                expression,
                statement: body,
            } => self.switch_statement(expression, body)?,
            S::While {
                condition,
                statement: body,
            } => self.while_statement(condition, body)?,
            S::DoWhile {
                statement: body,
                condition,
            } => self.do_while_statement(body, condition)?,
            S::For {
                initializer,
                condition,
                step,
                statement: body,
            } => self.for_statement(initializer, condition.as_deref(), step.as_deref(), body)?,
            S::Goto { label, .. } => {
                if self.current.is_some() {
                    let target = *self.label_blocks.get(label).ok_or_else(|| {
                        IrError::lower(
                            LOWERING_ERROR,
                            statement.span,
                            format!("goto references unknown label {}", label.0),
                        )
                    })?;
                    self.branch(target)?;
                }
            }
            S::ComputedGoto(expression) => {
                if self.current.is_some() {
                    if self.label_blocks.is_empty() {
                        return Err(IrError::lower(
                            LOWERING_ERROR,
                            statement.span,
                            "a computed goto requires at least one label in its function",
                        ));
                    }
                    let selector = self.expect_value(expression)?;
                    let targets = self
                        .label_blocks
                        .values()
                        .copied()
                        .map(|target| FullEdge {
                            target,
                            arguments: Vec::new(),
                        })
                        .collect();
                    self.terminate(FullTerminator::IndirectBranch { selector, targets })?;
                }
            }
            S::Continue => {
                if self.current.is_some() {
                    let target = self.continue_targets.last().copied().ok_or_else(|| {
                        IrError::lower(LOWERING_ERROR, statement.span, "continue outside a loop")
                    })?;
                    self.branch(target)?;
                }
            }
            S::Break => {
                if self.current.is_some() {
                    let target = self.break_targets.last().copied().ok_or_else(|| {
                        IrError::lower(
                            LOWERING_ERROR,
                            statement.span,
                            "break outside a loop or switch",
                        )
                    })?;
                    self.branch(target)?;
                }
            }
            S::Return(expression) => {
                if self.current.is_some() {
                    let value = expression
                        .as_ref()
                        .map(|expression| self.expect_value(expression))
                        .transpose()?;
                    self.terminate(FullTerminator::Return(value))?;
                }
            }
        }
        Ok(())
    }

    fn local_declaration(
        &mut self,
        declaration: &FullTypedLocalDeclaration,
    ) -> Result<(), IrError> {
        if !declaration.variable_length_bounds.is_empty() {
            return Err(IrError::lower(
                LOWERING_ERROR,
                declaration.span,
                "runtime variable-length declaration bounds are not yet lowered",
            ));
        }
        if declaration.duration != StorageDuration::Automatic || self.current.is_none() {
            return Ok(());
        }
        let storage = *self
            .storage_by_local
            .get(&declaration.local)
            .ok_or_else(|| {
                IrError::lower(
                    LOWERING_ERROR,
                    declaration.span,
                    format!("local `{}` has no preallocated storage", declaration.name),
                )
            })?;
        if let Some(initializer) = &declaration.initializer {
            let address = self.address_of_storage(storage, declaration.span)?;
            let place = LoweredPlace {
                address,
                object: declaration.ty,
                access: access_from_qualified(declaration.ty),
                bitfield: None,
            };
            self.runtime_initializer(place, initializer)?;
        }
        Ok(())
    }

    fn runtime_initializer(
        &mut self,
        destination: LoweredPlace,
        initializer: &FullTypedInitializer,
    ) -> Result<(), IrError> {
        match &initializer.kind {
            FullTypedInitializerKind::Zero => self.emit_effect(
                FullInstructionKind::ZeroInitialize {
                    destination: destination.address,
                    object: destination.object,
                },
                initializer.span,
            ),
            FullTypedInitializerKind::String(string) => {
                let copy_code_units = string_copy_code_units(
                    self.types,
                    self.unit,
                    destination.object,
                    *string,
                    initializer.span,
                )?;
                self.emit_effect(
                    FullInstructionKind::StringInitialize {
                        destination: destination.address,
                        string: *string,
                        object: destination.object,
                        copy_code_units,
                    },
                    initializer.span,
                )
            }
            FullTypedInitializerKind::Scalar(expression) => {
                if is_aggregate(self.types, destination.object.ty) {
                    let source = self.aggregate_source(expression)?;
                    self.aggregate_copy(&destination, &source, initializer.span)
                } else {
                    let value = self.expect_value(expression)?;
                    self.store_place(&destination, value, initializer.span)
                }
            }
            FullTypedInitializerKind::Aggregate(entries) => {
                self.emit_effect(
                    FullInstructionKind::ZeroInitialize {
                        destination: destination.address,
                        object: destination.object,
                    },
                    initializer.span,
                )?;
                for entry in entries {
                    let subobject = self.initializer_subobject(
                        destination.clone(),
                        &entry.path,
                        entry.initializer.span,
                    )?;
                    self.runtime_initializer(subobject, &entry.initializer)?;
                }
                Ok(())
            }
        }
    }

    fn initializer_subobject(
        &mut self,
        mut place: LoweredPlace,
        path: &[InitializerPathElement],
        span: Span,
    ) -> Result<LoweredPlace, IrError> {
        for element in path {
            match element {
                InitializerPathElement::Index(index) => {
                    let Some(TypeKind::Array(array)) =
                        self.types.try_kind(place.object.ty).cloned()
                    else {
                        return Err(IrError::lower(
                            LOWERING_ERROR,
                            span,
                            "initializer index path does not select an array",
                        ));
                    };
                    let index_value = self.emit_result(
                        FullInstructionKind::Constant(ScalarConstant::Unsigned(*index as u128)),
                        QualifiedType::unqualified(TypeId::UNSIGNED_LONG),
                        span,
                    )?;
                    let pointer = self.types.pointer(array.element);
                    let pointer_ty = QualifiedType::unqualified(pointer);
                    let base = self.emit_result(
                        FullInstructionKind::Convert {
                            kind: ScalarConversion::ArrayToPointer,
                            operand: place.address,
                            from: place.object,
                            to: pointer_ty,
                        },
                        pointer_ty,
                        span,
                    )?;
                    let address = self.emit_result(
                        FullInstructionKind::PointerOffset {
                            base,
                            index: index_value,
                            element: array.element,
                            subtract: false,
                        },
                        pointer_ty,
                        span,
                    )?;
                    place = LoweredPlace {
                        address,
                        object: array.element,
                        access: merge_access(place.access, access_from_qualified(array.element)),
                        bitfield: None,
                    };
                }
                InitializerPathElement::Field {
                    index,
                    name,
                    bitfield,
                } => {
                    let record_ty = place.object;
                    let Some(TypeKind::Record(record)) = self.types.try_kind(record_ty.ty).cloned()
                    else {
                        return Err(IrError::lower(
                            LOWERING_ERROR,
                            span,
                            "initializer field path does not select a record",
                        ));
                    };
                    let field_ty = self
                        .types
                        .record(record)
                        .and_then(|record| record.fields.as_ref())
                        .and_then(|fields| fields.get(*index))
                        .map(|field| {
                            QualifiedType::new(
                                field.ty.ty,
                                field.ty.qualifiers | record_ty.qualifiers,
                            )
                        })
                        .ok_or_else(|| {
                            IrError::lower(
                                LOWERING_ERROR,
                                span,
                                format!("initializer references unknown field {index}"),
                            )
                        })?;
                    let pointer = self.types.pointer(field_ty);
                    let address = self.emit_result(
                        FullInstructionKind::ProjectField {
                            base: place.address,
                            record: record_ty,
                            field_index: *index,
                            field_name: name.clone(),
                        },
                        QualifiedType::unqualified(pointer),
                        span,
                    )?;
                    let bitfield = bitfield.as_ref().map(bitfield_descriptor);
                    let bitfield_access = bitfield
                        .as_ref()
                        .map_or(MemoryAccess::default(), |_| place.access);
                    place = LoweredPlace {
                        address,
                        object: field_ty,
                        access: merge_access(bitfield_access, access_from_qualified(field_ty)),
                        bitfield,
                    };
                }
            }
        }
        Ok(place)
    }

    fn if_statement(
        &mut self,
        condition: &FullTypedExpression,
        then_statement: &FullTypedStatement,
        else_statement: Option<&FullTypedStatement>,
    ) -> Result<(), IrError> {
        self.instruction_root();
        let condition = self.expect_value(condition)?;
        let then_block = self.new_block();
        let else_block = self.new_block();
        let merge = self.new_block();
        self.terminate(FullTerminator::Conditional {
            condition,
            then_edge: empty_edge(then_block),
            else_edge: empty_edge(else_block),
        })?;

        self.current = Some(then_block);
        self.statement(then_statement)?;
        if self.current.is_some() {
            self.branch(merge)?;
        }
        self.current = Some(else_block);
        if let Some(statement) = else_statement {
            self.statement(statement)?;
        }
        if self.current.is_some() {
            self.branch(merge)?;
        }
        self.current = Some(merge);
        Ok(())
    }

    fn switch_statement(
        &mut self,
        expression: &FullTypedExpression,
        body: &FullTypedStatement,
    ) -> Result<(), IrError> {
        self.instruction_root();
        let selector = self.expect_value(expression)?;
        let end = self.new_block();
        let mut case_values = Vec::new();
        let mut has_default = false;
        collect_switch_labels(body, &mut case_values, &mut has_default);
        let mut cases = VecDeque::new();
        let mut dispatch = Vec::new();
        for value in case_values {
            let block = self.new_block();
            cases.push_back((value, block));
            dispatch.push(SwitchEdge {
                value,
                edge: empty_edge(block),
            });
        }
        let default_block = has_default.then(|| self.new_block());
        self.terminate(FullTerminator::Switch {
            selector,
            cases: dispatch,
            default: empty_edge(default_block.unwrap_or(end)),
        })?;
        self.switches.push(SwitchContext {
            cases,
            default: default_block,
        });
        self.break_targets.push(end);
        self.current = None;
        self.statement(body)?;
        if self.current.is_some() {
            self.branch(end)?;
        }
        self.break_targets.pop();
        let context = self.switches.pop().expect("switch context was pushed");
        if !context.cases.is_empty() || context.default.is_some() {
            return Err(IrError::lower(
                LOWERING_ERROR,
                body.span,
                "switch labels were not consumed in source order",
            ));
        }
        self.current = Some(end);
        Ok(())
    }

    fn while_statement(
        &mut self,
        condition: &FullTypedExpression,
        body: &FullTypedStatement,
    ) -> Result<(), IrError> {
        self.instruction_root();
        let header = self.new_block();
        let body_block = self.new_block();
        let end = self.new_block();
        self.branch(header)?;
        self.current = Some(header);
        let condition = self.expect_value(condition)?;
        self.terminate(FullTerminator::Conditional {
            condition,
            then_edge: empty_edge(body_block),
            else_edge: empty_edge(end),
        })?;
        self.break_targets.push(end);
        self.continue_targets.push(header);
        self.current = Some(body_block);
        self.statement(body)?;
        if self.current.is_some() {
            self.branch(header)?;
        }
        self.continue_targets.pop();
        self.break_targets.pop();
        self.current = Some(end);
        Ok(())
    }

    fn do_while_statement(
        &mut self,
        body: &FullTypedStatement,
        condition: &FullTypedExpression,
    ) -> Result<(), IrError> {
        self.instruction_root();
        let body_block = self.new_block();
        let condition_block = self.new_block();
        let end = self.new_block();
        self.branch(body_block)?;
        self.break_targets.push(end);
        self.continue_targets.push(condition_block);
        self.current = Some(body_block);
        self.statement(body)?;
        if self.current.is_some() {
            self.branch(condition_block)?;
        }
        self.current = Some(condition_block);
        let condition = self.expect_value(condition)?;
        self.terminate(FullTerminator::Conditional {
            condition,
            then_edge: empty_edge(body_block),
            else_edge: empty_edge(end),
        })?;
        self.continue_targets.pop();
        self.break_targets.pop();
        self.current = Some(end);
        Ok(())
    }

    fn for_statement(
        &mut self,
        initializer: &FullTypedForInitializer,
        condition: Option<&FullTypedExpression>,
        step: Option<&FullTypedExpression>,
        body: &FullTypedStatement,
    ) -> Result<(), IrError> {
        self.instruction_root();
        match initializer {
            FullTypedForInitializer::Empty => {}
            FullTypedForInitializer::Expression(expression) => {
                let _ = self.expression(expression)?;
            }
            FullTypedForInitializer::Declarations(items) => {
                for item in items {
                    if let FullTypedBlockItem::Declaration(declaration) = item {
                        self.local_declaration(declaration)?;
                    }
                }
            }
        }
        let header = self.new_block();
        let body_block = self.new_block();
        let step_block = self.new_block();
        let end = self.new_block();
        self.branch(header)?;
        self.current = Some(header);
        if let Some(condition) = condition {
            let condition = self.expect_value(condition)?;
            self.terminate(FullTerminator::Conditional {
                condition,
                then_edge: empty_edge(body_block),
                else_edge: empty_edge(end),
            })?;
        } else {
            self.branch(body_block)?;
        }
        self.break_targets.push(end);
        self.continue_targets.push(step_block);
        self.current = Some(body_block);
        self.statement(body)?;
        if self.current.is_some() {
            self.branch(step_block)?;
        }
        self.current = Some(step_block);
        if let Some(step) = step {
            let _ = self.expression(step)?;
        }
        if self.current.is_some() {
            self.branch(header)?;
        }
        self.continue_targets.pop();
        self.break_targets.pop();
        self.current = Some(end);
        Ok(())
    }
}

impl FunctionBuilder<'_> {
    fn expression(&mut self, expression: &FullTypedExpression) -> Result<Option<ValueId>, IrError> {
        use FullTypedExpressionKind as E;
        match &expression.kind {
            E::Constant(constant) => self
                .constant(*constant, expression.ty, expression.span)
                .map(Some),
            E::StringLiteral(_) => Err(IrError::lower(
                LOWERING_ERROR,
                expression.span,
                "string literal reached value lowering without array-to-pointer conversion",
            )),
            E::DeclRef(SymbolReference::Enumerator { value }) => self
                .emit_result(
                    FullInstructionKind::Constant(ScalarConstant::Signed(*value)),
                    expression.ty,
                    expression.span,
                )
                .map(Some),
            E::DeclRef(SymbolReference::Function(function)) => self
                .address_of_function(*function, expression.span)
                .map(Some),
            E::DeclRef(_) | E::CompoundLiteral { .. } => {
                if expression.place.is_some() {
                    Err(IrError::lower(
                        LOWERING_ERROR,
                        expression.span,
                        "object lvalue reached value lowering without an explicit conversion",
                    ))
                } else {
                    Err(IrError::lower(
                        LOWERING_ERROR,
                        expression.span,
                        "declaration reference has neither a value nor a place",
                    ))
                }
            }
            E::Conversion {
                kind,
                expression: operand,
            } => self.conversion(*kind, operand, expression.ty, expression.span),
            E::Unary { operator, operand } => {
                let operand = self.expect_value(operand)?;
                let operator = unary_operation(*operator).ok_or_else(|| {
                    IrError::lower(
                        LOWERING_ERROR,
                        expression.span,
                        "addressing or increment unary operator reached scalar unary lowering",
                    )
                })?;
                self.emit_result(
                    FullInstructionKind::Unary { operator, operand },
                    expression.ty,
                    expression.span,
                )
                .map(Some)
            }
            E::Binary {
                operator,
                left,
                right,
            } if matches!(operator, AstBinary::LogicalAnd | AstBinary::LogicalOr) => self
                .logical_expression(*operator, left, right, expression.ty, expression.span)
                .map(Some),
            E::Binary {
                operator,
                left,
                right,
            } => {
                let left = self.expect_value(left)?;
                let right = self.expect_value(right)?;
                self.binary_value(*operator, left, right, expression.ty, expression.span)
                    .map(Some)
            }
            E::AddressOf(operand) => {
                if let E::DeclRef(SymbolReference::Function(function)) = &operand.kind {
                    return self
                        .address_of_function(*function, expression.span)
                        .map(Some);
                }
                let place = self.place(operand)?;
                if place.bitfield.is_some() {
                    return Err(IrError::lower(
                        LOWERING_ERROR,
                        expression.span,
                        "cannot form the address of a bitfield",
                    ));
                }
                Ok(Some(place.address))
            }
            E::Dereference(pointer)
                if self.types.function_signature(expression.ty.ty).is_some() =>
            {
                self.expression(pointer)
            }
            E::Member { .. } if expression.place.is_none() => {
                let place = self.place(expression)?;
                self.load_place(&place, expression.span).map(Some)
            }
            E::Dereference(_) | E::Subscript { .. } | E::Member { .. } => {
                if expression.place.is_some() {
                    Err(IrError::lower(
                        LOWERING_ERROR,
                        expression.span,
                        "place expression reached value lowering without an explicit conversion",
                    ))
                } else {
                    Err(IrError::lower(
                        LOWERING_ERROR,
                        expression.span,
                        "unsupported non-place aggregate member value",
                    ))
                }
            }
            E::Assignment {
                operator,
                target,
                value,
                store,
                compound,
            } => self
                .assignment(
                    *operator,
                    target,
                    value,
                    *store,
                    compound.as_ref(),
                    expression.ty,
                    expression.span,
                )
                .map(Some),
            E::Increment {
                operand,
                decrement,
                postfix,
                store,
            } => self
                .increment(
                    operand,
                    *decrement,
                    *postfix,
                    *store,
                    expression.ty,
                    expression.span,
                )
                .map(Some),
            E::Call {
                callee,
                function,
                arguments,
                variadic_boundary,
            } => self.call(
                callee,
                *function,
                arguments,
                *variadic_boundary,
                expression.ty,
                expression.span,
            ),
            E::Conditional {
                condition,
                then_expression,
                else_expression,
            } => self.conditional_expression(
                condition,
                then_expression,
                else_expression,
                expression.ty,
                expression.span,
            ),
            E::Comma(expressions) => {
                let mut result = None;
                for expression in expressions {
                    result = self.expression(expression)?;
                }
                Ok(result)
            }
            E::BuiltinExpect { value, expected: _ } => self.expression(value),
            E::IntegerIntrinsic { operation, operand } => {
                let operand = self.expect_value(operand)?;
                let operation = match operation {
                    TypedIntegerIntrinsicOperation::ByteSwap64 => {
                        IntegerIntrinsicOperation::ByteSwap64
                    }
                    TypedIntegerIntrinsicOperation::CountLeadingZerosInt => {
                        IntegerIntrinsicOperation::CountLeadingZerosInt
                    }
                    TypedIntegerIntrinsicOperation::CountLeadingZerosLong => {
                        IntegerIntrinsicOperation::CountLeadingZerosLong
                    }
                    TypedIntegerIntrinsicOperation::CountLeadingZerosLongLong => {
                        IntegerIntrinsicOperation::CountLeadingZerosLongLong
                    }
                    TypedIntegerIntrinsicOperation::CountTrailingZerosLongLong => {
                        IntegerIntrinsicOperation::CountTrailingZerosLongLong
                    }
                    TypedIntegerIntrinsicOperation::PopulationCountInt => {
                        IntegerIntrinsicOperation::PopulationCountInt
                    }
                    TypedIntegerIntrinsicOperation::PopulationCountLongLong => {
                        IntegerIntrinsicOperation::PopulationCountLongLong
                    }
                };
                self.emit_result(
                    FullInstructionKind::IntegerIntrinsic { operation, operand },
                    expression.ty,
                    expression.span,
                )
                .map(Some)
            }
            E::Prefetch {
                address,
                write,
                locality,
            } => {
                let address = self.expect_value(address)?;
                self.emit_effect(
                    FullInstructionKind::Prefetch {
                        address,
                        write: *write,
                        locality: *locality,
                    },
                    expression.span,
                )?;
                Ok(None)
            }
            E::AtomicReadModifyWrite {
                operation,
                pointer,
                operand,
                object,
                return_new,
                order,
            } => {
                let address = self.expect_value(pointer)?;
                let operand = self.expect_value(operand)?;
                let operation = match operation {
                    TypedAtomicReadModifyWriteOperation::Add => AtomicReadModifyWriteOperation::Add,
                    TypedAtomicReadModifyWriteOperation::Subtract => {
                        AtomicReadModifyWriteOperation::Subtract
                    }
                    TypedAtomicReadModifyWriteOperation::Exchange => {
                        AtomicReadModifyWriteOperation::Exchange
                    }
                };
                let order = match order {
                    TypedMemoryOrder::SequentiallyConsistent => MemoryOrder::SequentiallyConsistent,
                };
                if *return_new && operation == AtomicReadModifyWriteOperation::Exchange {
                    return Err(IrError::lower(
                        LOWERING_ERROR,
                        expression.span,
                        "atomic exchange cannot request the replacement value",
                    ));
                }
                let value_ty = QualifiedType::unqualified(object.ty);
                self.emit_result(
                    FullInstructionKind::AtomicReadModifyWrite {
                        operation,
                        address,
                        operand,
                        object: *object,
                        return_new: *return_new,
                        order,
                    },
                    value_ty,
                    expression.span,
                )
                .map(Some)
            }
            E::AtomicCompareExchange {
                pointer,
                expected,
                replacement,
                object,
                return_boolean,
                order,
            } => {
                let address = self.expect_value(pointer)?;
                let expected = self.expect_value(expected)?;
                let replacement = self.expect_value(replacement)?;
                let order = match order {
                    TypedMemoryOrder::SequentiallyConsistent => MemoryOrder::SequentiallyConsistent,
                };
                let value_ty = QualifiedType::unqualified(object.ty);
                let old = self.emit_result(
                    FullInstructionKind::AtomicCompareExchange {
                        address,
                        expected,
                        replacement,
                        object: *object,
                        order,
                    },
                    value_ty,
                    expression.span,
                )?;
                if *return_boolean {
                    self.emit_result(
                        FullInstructionKind::Binary {
                            operator: BinaryOperation::Equal,
                            left: old,
                            right: expected,
                        },
                        QualifiedType::unqualified(TypeId::BOOL),
                        expression.span,
                    )
                    .map(Some)
                } else {
                    Ok(Some(old))
                }
            }
            E::Sizeof { size, .. } => self
                .emit_result(
                    FullInstructionKind::Constant(ScalarConstant::Unsigned(*size as u128)),
                    expression.ty,
                    expression.span,
                )
                .map(Some),
            E::Alignof { align, .. } => self
                .emit_result(
                    FullInstructionKind::Constant(ScalarConstant::Unsigned(*align as u128)),
                    expression.ty,
                    expression.span,
                )
                .map(Some),
            E::Offsetof { offset, .. } => self
                .emit_result(
                    FullInstructionKind::Constant(ScalarConstant::Unsigned(*offset as u128)),
                    expression.ty,
                    expression.span,
                )
                .map(Some),
            E::MemoryFence { order } => {
                let order = match order {
                    TypedMemoryOrder::SequentiallyConsistent => MemoryOrder::SequentiallyConsistent,
                };
                self.emit_effect(FullInstructionKind::MemoryFence { order }, expression.span)?;
                Ok(None)
            }
            E::VaStart {
                list,
                last_named_parameter,
            } => {
                let list = self.va_list_address(list)?;
                self.emit_effect(
                    FullInstructionKind::VaStart {
                        list,
                        last_named_parameter: *last_named_parameter,
                    },
                    expression.span,
                )?;
                Ok(None)
            }
            E::VaArg { list, requested } => {
                let list = self.va_list_address(list)?;
                self.emit_result(
                    FullInstructionKind::VaArg {
                        list,
                        requested: *requested,
                    },
                    *requested,
                    expression.span,
                )
                .map(Some)
            }
            E::VaCopy {
                destination,
                source,
            } => {
                let destination = self.va_list_address(destination)?;
                let source = self.va_list_address(source)?;
                self.emit_effect(
                    FullInstructionKind::VaCopy {
                        destination,
                        source,
                    },
                    expression.span,
                )?;
                Ok(None)
            }
            E::VaEnd { list } => {
                let list = self.va_list_address(list)?;
                self.emit_effect(FullInstructionKind::VaEnd { list }, expression.span)?;
                Ok(None)
            }
        }
    }

    fn expect_value(&mut self, expression: &FullTypedExpression) -> Result<ValueId, IrError> {
        self.expression(expression)?.ok_or_else(|| {
            IrError::lower(
                LOWERING_ERROR,
                expression.span,
                "void expression used where a value is required",
            )
        })
    }

    fn va_list_address(&mut self, expression: &FullTypedExpression) -> Result<ValueId, IrError> {
        if matches!(
            self.types.try_kind(expression.ty.ty),
            Some(TypeKind::Array(_))
        ) {
            self.place(expression).map(|place| place.address)
        } else {
            self.expect_value(expression)
        }
    }

    fn constant(
        &mut self,
        constant: ConstantValue,
        ty: QualifiedType,
        span: Span,
    ) -> Result<ValueId, IrError> {
        match constant {
            ConstantValue::Signed(value) => self.emit_result(
                FullInstructionKind::Constant(ScalarConstant::Signed(value)),
                ty,
                span,
            ),
            ConstantValue::Unsigned(value) => self.emit_result(
                FullInstructionKind::Constant(ScalarConstant::Unsigned(value)),
                ty,
                span,
            ),
            ConstantValue::Floating(value) => self.emit_result(
                FullInstructionKind::Constant(ScalarConstant::Floating(value)),
                ty,
                span,
            ),
            ConstantValue::NullPointer => self.emit_result(
                FullInstructionKind::Constant(ScalarConstant::NullPointer),
                ty,
                span,
            ),
            ConstantValue::Address(address) => {
                let target = match address.base {
                    ccc_sema::generic::RelocatableBase::Global(global) => {
                        RelocationTarget::Object(*self.file_data.get(&global).ok_or_else(|| {
                            IrError::lower(
                                LOWERING_ERROR,
                                span,
                                format!("constant references unknown global {}", global.0),
                            )
                        })?)
                    }
                    ccc_sema::generic::RelocatableBase::BlockStatic { function, local } => {
                        RelocationTarget::Object(
                            self.static_data
                                .get(&(function, local))
                                .map(|(data, _)| *data)
                                .ok_or_else(|| {
                                    IrError::lower(
                                        LOWERING_ERROR,
                                        span,
                                        format!(
                                            "constant references unknown block-static object {}:{}",
                                            function.0, local.0
                                        ),
                                    )
                                })?,
                        )
                    }
                    ccc_sema::generic::RelocatableBase::Function(function) => {
                        RelocationTarget::Function(function)
                    }
                    ccc_sema::generic::RelocatableBase::String(string) => {
                        RelocationTarget::String(string)
                    }
                    ccc_sema::generic::RelocatableBase::Label { function, label } => {
                        if function != self.function.id
                            || address.addend != 0
                            || address.one_past
                            || label.0 == u32::MAX
                        {
                            return Err(IrError::lower(
                                LOWERING_ERROR,
                                span,
                                "invalid cross-function or adjusted label address",
                            ));
                        }
                        return self.emit_result(
                            FullInstructionKind::Constant(ScalarConstant::Unsigned(
                                u128::from(label.0) + 1,
                            )),
                            ty,
                            span,
                        );
                    }
                };
                self.emit_result(
                    FullInstructionKind::AddressConstant {
                        target,
                        addend: address.addend,
                        one_past: address.one_past,
                    },
                    ty,
                    span,
                )
            }
        }
    }

    fn zero_value(&mut self, ty: QualifiedType, span: Span) -> Result<ValueId, IrError> {
        let constant = if pointer_pointee(self.types, ty.ty).is_some() {
            ScalarConstant::NullPointer
        } else if self
            .types
            .builtin_type(ty.ty)
            .is_some_and(|builtin| builtin.is_floating())
        {
            ScalarConstant::Floating(0.0)
        } else if self.types.is_integer(ty.ty) {
            ScalarConstant::Signed(0)
        } else {
            return Err(IrError::lower(
                LOWERING_ERROR,
                span,
                "cannot synthesize a scalar fallthrough result for this type",
            ));
        };
        self.emit_result(FullInstructionKind::Constant(constant), ty, span)
    }

    fn conversion(
        &mut self,
        kind: ConversionKind,
        operand: &FullTypedExpression,
        to: QualifiedType,
        span: Span,
    ) -> Result<Option<ValueId>, IrError> {
        match kind {
            ConversionKind::LvalueToValue { access } => {
                let place = self.place(operand)?;
                self.load_place_with_access(&place, access_from_semantics(access), span)
                    .map(Some)
            }
            ConversionKind::ArrayToPointer => {
                let place = self.place(operand)?;
                self.emit_result(
                    FullInstructionKind::Convert {
                        kind: ScalarConversion::ArrayToPointer,
                        operand: place.address,
                        from: operand.ty,
                        to,
                    },
                    to,
                    span,
                )
                .map(Some)
            }
            ConversionKind::FunctionToPointer => {
                let value = self.expect_value(operand)?;
                self.emit_result(
                    FullInstructionKind::Convert {
                        kind: ScalarConversion::FunctionToPointer,
                        operand: value,
                        from: operand.ty,
                        to,
                    },
                    to,
                    span,
                )
                .map(Some)
            }
            ConversionKind::ToVoid => {
                if let Some(value) = self.expression(operand)? {
                    self.emit_effect(
                        FullInstructionKind::Convert {
                            kind: ScalarConversion::ToVoid,
                            operand: value,
                            from: operand.ty,
                            to,
                        },
                        span,
                    )?;
                }
                Ok(None)
            }
            _ => {
                let value = self.expect_value(operand)?;
                let kind = scalar_conversion(kind).ok_or_else(|| {
                    IrError::lower(LOWERING_ERROR, span, "invalid scalar conversion kind")
                })?;
                self.emit_result(
                    FullInstructionKind::Convert {
                        kind,
                        operand: value,
                        from: operand.ty,
                        to,
                    },
                    to,
                    span,
                )
                .map(Some)
            }
        }
    }

    fn place(&mut self, expression: &FullTypedExpression) -> Result<LoweredPlace, IrError> {
        let mut pending = Vec::new();
        if let Some(root) = collect_aggregate_projection(expression, &mut pending) {
            if !is_aggregate(self.types, root.ty.ty) {
                return Err(IrError::lower(
                    LOWERING_ERROR,
                    expression.span,
                    "aggregate projection root does not have aggregate type",
                ));
            }
            let base = self.expect_value(root)?;
            let mut projections = Vec::with_capacity(pending.len());
            let projection_count = pending.len();
            let mut projected_bitfield = None;
            for (position, projection) in pending.into_iter().enumerate() {
                projections.push(match projection {
                    PendingAggregateProjection::Field {
                        index,
                        name,
                        bitfield,
                    } => {
                        if bitfield.is_some() && position + 1 != projection_count {
                            return Err(IrError::lower(
                                LOWERING_ERROR,
                                expression.span,
                                "bitfield is not the final aggregate projection",
                            ));
                        }
                        projected_bitfield = bitfield;
                        AggregateProjection::Field {
                            index,
                            name: name.map(str::to_owned),
                            bitfield,
                        }
                    }
                    PendingAggregateProjection::Index { index } => AggregateProjection::Index {
                        index: self.expect_value(index)?,
                    },
                });
            }
            let pointer = self.types.pointer(expression.ty);
            let address = self.emit_result(
                FullInstructionKind::AggregateProject {
                    base,
                    aggregate: root.ty,
                    projections,
                },
                QualifiedType::unqualified(pointer),
                expression.span,
            )?;
            let semantic_place = expression.place.as_ref();
            return Ok(LoweredPlace {
                address,
                object: expression.ty,
                access: semantic_place.map_or_else(
                    || access_from_qualified(expression.ty),
                    |place| access_from_semantics(place.access),
                ),
                bitfield: projected_bitfield.or_else(|| {
                    semantic_place
                        .and_then(|place| place.bitfield.as_ref())
                        .map(bitfield_descriptor)
                }),
            });
        }
        let semantic_place = expression.place.as_ref().ok_or_else(|| {
            IrError::lower(
                LOWERING_ERROR,
                expression.span,
                "expression does not denote a place",
            )
        })?;
        let address = match &expression.kind {
            FullTypedExpressionKind::DeclRef(SymbolReference::Global(global)) => {
                let data = *self.file_data.get(global).ok_or_else(|| {
                    IrError::lower(
                        LOWERING_ERROR,
                        expression.span,
                        format!("reference to unknown global {}", global.0),
                    )
                })?;
                self.address_of_data(data, expression.ty, expression.span)?
            }
            FullTypedExpressionKind::DeclRef(SymbolReference::Local(local)) => {
                if let Some(data) = self
                    .static_data
                    .get(&(self.function.id, *local))
                    .map(|(data, _)| *data)
                {
                    self.address_of_data(data, expression.ty, expression.span)?
                } else {
                    let storage = *self.storage_by_local.get(local).ok_or_else(|| {
                        IrError::lower(
                            LOWERING_ERROR,
                            expression.span,
                            format!("reference to unknown local {}", local.0),
                        )
                    })?;
                    self.address_of_storage(storage, expression.span)?
                }
            }
            FullTypedExpressionKind::DeclRef(SymbolReference::PredefinedFunctionName(string)) => {
                self.address_of_string(*string, expression.ty, expression.span)?
            }
            FullTypedExpressionKind::CompoundLiteral { local, initializer } => {
                let storage = *self.storage_by_local.get(local).ok_or_else(|| {
                    IrError::lower(
                        LOWERING_ERROR,
                        expression.span,
                        format!("reference to unknown compound literal {}", local.0),
                    )
                })?;
                let address = self.address_of_storage(storage, expression.span)?;
                let destination = LoweredPlace {
                    address,
                    object: expression.ty,
                    access: access_from_qualified(expression.ty),
                    bitfield: None,
                };
                self.runtime_initializer(destination, initializer)?;
                address
            }
            FullTypedExpressionKind::StringLiteral(string) => {
                self.address_of_string(*string, expression.ty, expression.span)?
            }
            FullTypedExpressionKind::Dereference(pointer) => self.expect_value(pointer)?,
            FullTypedExpressionKind::Subscript { base, index } => {
                let base = self.expect_value(base)?;
                let index = self.expect_value(index)?;
                let pointer = self.types.pointer(expression.ty);
                self.emit_result(
                    FullInstructionKind::PointerOffset {
                        base,
                        index,
                        element: expression.ty,
                        subtract: false,
                    },
                    QualifiedType::unqualified(pointer),
                    expression.span,
                )?
            }
            FullTypedExpressionKind::Member {
                base,
                field_index,
                name,
                indirect,
                ..
            } => {
                let (base_address, record) = if *indirect {
                    let address = self.expect_value(base)?;
                    let record = pointer_pointee(self.types, base.ty.ty).ok_or_else(|| {
                        IrError::lower(
                            LOWERING_ERROR,
                            base.span,
                            "indirect member base is not a pointer",
                        )
                    })?;
                    (address, record)
                } else {
                    let place = self.place(base)?;
                    (place.address, place.object)
                };
                let pointer = self.types.pointer(expression.ty);
                self.emit_result(
                    FullInstructionKind::ProjectField {
                        base: base_address,
                        record,
                        field_index: *field_index,
                        field_name: name.clone(),
                    },
                    QualifiedType::unqualified(pointer),
                    expression.span,
                )?
            }
            _ => {
                return Err(IrError::lower(
                    LOWERING_ERROR,
                    expression.span,
                    "unsupported expression shape for place lowering",
                ));
            }
        };
        Ok(LoweredPlace {
            address,
            object: expression.ty,
            access: access_from_semantics(semantic_place.access),
            bitfield: semantic_place.bitfield.as_ref().map(bitfield_descriptor),
        })
    }

    fn address_of_data(
        &mut self,
        global: DataId,
        object: QualifiedType,
        span: Span,
    ) -> Result<ValueId, IrError> {
        let pointer = self.types.pointer(object);
        self.emit_result(
            FullInstructionKind::AddressOfGlobal { global },
            QualifiedType::unqualified(pointer),
            span,
        )
    }

    fn address_of_function(
        &mut self,
        function: FullFunctionId,
        span: Span,
    ) -> Result<ValueId, IrError> {
        let signature = self
            .unit
            .functions
            .get(function.0 as usize)
            .filter(|candidate| candidate.id == function)
            .map(|function| function.signature)
            .ok_or_else(|| {
                IrError::lower(
                    LOWERING_ERROR,
                    span,
                    format!("reference to unknown function {}", function.0),
                )
            })?;
        let pointer = self.types.pointer(QualifiedType::unqualified(signature));
        self.emit_result(
            FullInstructionKind::AddressOfFunction {
                function,
                signature,
            },
            QualifiedType::unqualified(pointer),
            span,
        )
    }

    fn address_of_string(
        &mut self,
        string: StringId,
        object: QualifiedType,
        span: Span,
    ) -> Result<ValueId, IrError> {
        let known = self
            .unit
            .strings
            .get(string.0 as usize)
            .is_some_and(|candidate| candidate.id == string);
        if !known {
            return Err(IrError::lower(
                LOWERING_ERROR,
                span,
                format!("reference to unknown string {}", string.0),
            ));
        }
        let pointer = self.types.pointer(object);
        self.emit_result(
            FullInstructionKind::AddressOfString { string },
            QualifiedType::unqualified(pointer),
            span,
        )
    }

    fn load_place(&mut self, place: &LoweredPlace, span: Span) -> Result<ValueId, IrError> {
        self.load_place_with_access(place, place.access, span)
    }

    fn load_place_with_access(
        &mut self,
        place: &LoweredPlace,
        access: MemoryAccess,
        span: Span,
    ) -> Result<ValueId, IrError> {
        if access.atomic.is_some()
            && (place.bitfield.is_some() || is_aggregate(self.types, place.object.ty))
        {
            return Err(IrError::lower(
                LOWERING_ERROR,
                span,
                "atomic aggregate and atomic bitfield loads are not supported",
            ));
        }
        if let Some(descriptor) = place.bitfield {
            self.emit_result(
                FullInstructionKind::BitfieldLoad {
                    address: place.address,
                    descriptor,
                    access,
                },
                place.object,
                span,
            )
        } else if is_aggregate(self.types, place.object.ty) {
            self.emit_result(
                FullInstructionKind::AggregateSnapshot {
                    source: place.address,
                    object: place.object,
                    access,
                },
                place.object,
                span,
            )
        } else {
            self.emit_result(
                FullInstructionKind::Load {
                    address: place.address,
                    object: place.object,
                    access,
                },
                QualifiedType::unqualified(place.object.ty),
                span,
            )
        }
    }

    fn store_place(
        &mut self,
        place: &LoweredPlace,
        value: ValueId,
        span: Span,
    ) -> Result<(), IrError> {
        if place.access.atomic.is_some()
            && (place.bitfield.is_some() || is_aggregate(self.types, place.object.ty))
        {
            return Err(IrError::lower(
                LOWERING_ERROR,
                span,
                "atomic aggregate and atomic bitfield stores are not supported",
            ));
        }
        if let Some(descriptor) = place.bitfield {
            self.emit_effect(
                FullInstructionKind::BitfieldStore {
                    address: place.address,
                    value,
                    descriptor,
                    access: place.access,
                },
                span,
            )
        } else {
            self.emit_effect(
                FullInstructionKind::Store {
                    address: place.address,
                    value,
                    object: place.object,
                    access: place.access,
                },
                span,
            )
        }
    }

    fn aggregate_source(
        &mut self,
        expression: &FullTypedExpression,
    ) -> Result<LoweredPlace, IrError> {
        let address = self.expect_value(expression)?;
        Ok(LoweredPlace {
            address,
            object: QualifiedType::unqualified(expression.ty.ty),
            access: MemoryAccess::default(),
            bitfield: None,
        })
    }

    fn aggregate_copy(
        &mut self,
        destination: &LoweredPlace,
        source: &LoweredPlace,
        span: Span,
    ) -> Result<(), IrError> {
        if destination.access.atomic.is_some() || source.access.atomic.is_some() {
            return Err(IrError::lower(
                LOWERING_ERROR,
                span,
                "atomic aggregate copies are not supported",
            ));
        }
        self.emit_effect(
            FullInstructionKind::AggregateCopy {
                destination: destination.address,
                source: source.address,
                destination_object: destination.object,
                source_object: source.object,
                destination_access: destination.access,
                source_access: source.access,
                overlap: AggregateOverlap::MayOverlap,
            },
            span,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn assignment(
        &mut self,
        operator: AstAssignment,
        target: &FullTypedExpression,
        value: &FullTypedExpression,
        store: AccessSemantics,
        compound: Option<&CompoundAssignmentPlan>,
        result_ty: QualifiedType,
        span: Span,
    ) -> Result<ValueId, IrError> {
        let mut destination = self.place(target)?;
        destination.access = access_from_semantics(store);
        if is_aggregate(self.types, destination.object.ty) {
            if operator != AstAssignment::Assign || compound.is_some() {
                return Err(IrError::lower(
                    LOWERING_ERROR,
                    span,
                    "compound assignment is not defined for aggregate objects",
                ));
            }
            let source = self.aggregate_source(value)?;
            self.aggregate_copy(&destination, &source, span)?;
            debug_assert_eq!(source.object.ty, result_ty.ty);
            return Ok(source.address);
        }
        let stored = if let Some(plan) = compound {
            if destination.access.atomic.is_some() {
                return Err(IrError::lower(
                    LOWERING_ERROR,
                    span,
                    "atomic compound read-modify-write is not supported",
                ));
            }
            let loaded =
                self.load_place_with_access(&destination, access_from_semantics(plan.load), span)?;
            let loaded = if destination.object.ty != plan.load_ty.ty {
                self.emit_result(
                    FullInstructionKind::Convert {
                        kind: inferred_conversion(self.types, destination.object, plan.load_ty),
                        operand: loaded,
                        from: destination.object,
                        to: plan.load_ty,
                    },
                    plan.load_ty,
                    span,
                )?
            } else {
                loaded
            };
            let right = self.expect_value(value)?;
            let calculated =
                self.binary_value(plan.operator, loaded, right, plan.calculation_ty, span)?;
            if let Some(conversion) = plan.result_conversion {
                self.emit_result(
                    FullInstructionKind::Convert {
                        kind: scalar_conversion(conversion).ok_or_else(|| {
                            IrError::lower(
                                LOWERING_ERROR,
                                span,
                                "compound assignment uses a non-scalar result conversion",
                            )
                        })?,
                        operand: calculated,
                        from: plan.calculation_ty,
                        to: result_ty,
                    },
                    result_ty,
                    span,
                )?
            } else {
                calculated
            }
        } else {
            self.expect_value(value)?
        };
        self.store_place(&destination, stored, span)?;
        Ok(stored)
    }

    fn increment(
        &mut self,
        operand: &FullTypedExpression,
        decrement: bool,
        postfix: bool,
        store: AccessSemantics,
        result_ty: QualifiedType,
        span: Span,
    ) -> Result<ValueId, IrError> {
        let mut place = self.place(operand)?;
        place.access = access_from_semantics(store);
        if place.access.atomic.is_some() {
            return Err(IrError::lower(
                LOWERING_ERROR,
                span,
                "atomic increment and decrement are not supported",
            ));
        }
        let old = self.load_place(&place, span)?;
        let pointer = pointer_pointee(self.types, operand.ty.ty);
        let one_ty = if pointer.is_some() {
            QualifiedType::unqualified(TypeId::INT)
        } else {
            operand.ty
        };
        let one = if self
            .types
            .builtin_type(operand.ty.ty)
            .is_some_and(|builtin| builtin.is_floating())
        {
            ScalarConstant::Floating(1.0)
        } else {
            ScalarConstant::Signed(1)
        };
        let one = self.emit_result(FullInstructionKind::Constant(one), one_ty, span)?;
        let updated = if let Some(element) = pointer {
            self.emit_result(
                FullInstructionKind::PointerOffset {
                    base: old,
                    index: one,
                    element,
                    subtract: decrement,
                },
                result_ty,
                span,
            )?
        } else {
            self.emit_result(
                FullInstructionKind::Binary {
                    operator: if decrement {
                        BinaryOperation::Subtract
                    } else {
                        BinaryOperation::Add
                    },
                    left: old,
                    right: one,
                },
                result_ty,
                span,
            )?
        };
        self.store_place(&place, updated, span)?;
        Ok(if postfix { old } else { updated })
    }

    fn binary_value(
        &mut self,
        operator: AstBinary,
        left: ValueId,
        right: ValueId,
        result_ty: QualifiedType,
        span: Span,
    ) -> Result<ValueId, IrError> {
        let left_ty = self.function.value_types[left.0 as usize];
        let right_ty = self.function.value_types[right.0 as usize];
        let left_pointee = pointer_pointee(self.types, left_ty);
        let right_pointee = pointer_pointee(self.types, right_ty);
        if matches!(operator, AstBinary::Add | AstBinary::Subtract) {
            if let Some(element) = left_pointee
                && self.types.is_integer(right_ty)
            {
                return self.emit_result(
                    FullInstructionKind::PointerOffset {
                        base: left,
                        index: right,
                        element,
                        subtract: operator == AstBinary::Subtract,
                    },
                    result_ty,
                    span,
                );
            }
            if operator == AstBinary::Add
                && let Some(element) = right_pointee
                && self.types.is_integer(left_ty)
            {
                return self.emit_result(
                    FullInstructionKind::PointerOffset {
                        base: right,
                        index: left,
                        element,
                        subtract: false,
                    },
                    result_ty,
                    span,
                );
            }
            if operator == AstBinary::Subtract
                && let (Some(element), Some(_)) = (left_pointee, right_pointee)
            {
                return self.emit_result(
                    FullInstructionKind::PointerDifference {
                        left,
                        right,
                        // Pointer subtraction permits qualified and unqualified
                        // versions of compatible object types. Qualifiers do not
                        // affect the element stride carried by the IR.
                        element: QualifiedType::unqualified(element.ty),
                    },
                    result_ty,
                    span,
                );
            }
        }
        let operator = binary_operation(operator).ok_or_else(|| {
            IrError::lower(
                LOWERING_ERROR,
                span,
                "short-circuit operator reached ordinary binary lowering",
            )
        })?;
        self.emit_result(
            FullInstructionKind::Binary {
                operator,
                left,
                right,
            },
            result_ty,
            span,
        )
    }

    fn logical_expression(
        &mut self,
        operator: AstBinary,
        left: &FullTypedExpression,
        right: &FullTypedExpression,
        result_ty: QualifiedType,
        span: Span,
    ) -> Result<ValueId, IrError> {
        let left = self.expect_value(left)?;
        let right_block = self.new_block();
        let short_block = self.new_block();
        let merge = self.new_block();
        let result = self.add_block_parameter(merge, result_ty);
        let (then_edge, else_edge) = if operator == AstBinary::LogicalAnd {
            (empty_edge(right_block), empty_edge(short_block))
        } else {
            (empty_edge(short_block), empty_edge(right_block))
        };
        self.terminate(FullTerminator::Conditional {
            condition: left,
            then_edge,
            else_edge,
        })?;

        self.current = Some(short_block);
        let short_value = self.emit_result(
            FullInstructionKind::Constant(ScalarConstant::Signed(i128::from(
                operator == AstBinary::LogicalOr,
            ))),
            result_ty,
            span,
        )?;
        self.terminate(FullTerminator::Branch(FullEdge {
            target: merge,
            arguments: vec![short_value],
        }))?;

        self.current = Some(right_block);
        let right = self.expect_value(right)?;
        self.terminate(FullTerminator::Branch(FullEdge {
            target: merge,
            arguments: vec![right],
        }))?;
        self.current = Some(merge);
        Ok(result)
    }

    fn conditional_expression(
        &mut self,
        condition: &FullTypedExpression,
        then_expression: &FullTypedExpression,
        else_expression: &FullTypedExpression,
        result_ty: QualifiedType,
        _span: Span,
    ) -> Result<Option<ValueId>, IrError> {
        let condition = self.expect_value(condition)?;
        let then_block = self.new_block();
        let else_block = self.new_block();
        let merge = self.new_block();
        let result = (!is_void(self.types, result_ty.ty))
            .then(|| self.add_block_parameter(merge, result_ty));
        self.terminate(FullTerminator::Conditional {
            condition,
            then_edge: empty_edge(then_block),
            else_edge: empty_edge(else_block),
        })?;

        self.current = Some(then_block);
        let then_value = self.expression(then_expression)?;
        if self.current.is_some() {
            self.terminate(FullTerminator::Branch(FullEdge {
                target: merge,
                arguments: then_value.into_iter().collect(),
            }))?;
        }
        self.current = Some(else_block);
        let else_value = self.expression(else_expression)?;
        if self.current.is_some() {
            self.terminate(FullTerminator::Branch(FullEdge {
                target: merge,
                arguments: else_value.into_iter().collect(),
            }))?;
        }
        self.current = Some(merge);
        Ok(result)
    }

    fn call(
        &mut self,
        callee: &FullTypedExpression,
        direct: Option<FullFunctionId>,
        arguments: &[FullTypedExpression],
        variadic_boundary: usize,
        result_ty: QualifiedType,
        span: Span,
    ) -> Result<Option<ValueId>, IrError> {
        let (signature, effects) = if let Some(function) = direct {
            let declaration = self
                .unit
                .functions
                .get(function.0 as usize)
                .filter(|candidate| candidate.id == function)
                .ok_or_else(|| {
                    IrError::lower(
                        LOWERING_ERROR,
                        span,
                        format!("call references unknown function {}", function.0),
                    )
                })?;
            (
                declaration.signature,
                CallEffects {
                    no_return: declaration.properties.no_return,
                    ..CallEffects::default()
                },
            )
        } else {
            let signature = pointer_pointee(self.types, callee.ty.ty)
                .map(|pointee| pointee.ty)
                .or_else(|| {
                    self.types
                        .function_signature(callee.ty.ty)
                        .map(|_| callee.ty.ty)
                })
                .ok_or_else(|| {
                    IrError::lower(
                        LOWERING_ERROR,
                        span,
                        "indirect call callee does not point to a function type",
                    )
                })?;
            (signature, CallEffects::default())
        };
        let function_type = self.types.function_signature(signature).ok_or_else(|| {
            IrError::lower(
                LOWERING_ERROR,
                span,
                "call instruction does not carry a canonical function signature",
            )
        })?;
        let callee_value = if direct.is_none() {
            Some(self.expect_value(callee)?)
        } else {
            None
        };
        let arguments = arguments
            .iter()
            .map(|argument| self.expect_value(argument))
            .collect::<Result<Vec<_>, _>>()?;
        let kind = if let Some(function) = direct {
            FullInstructionKind::DirectCall {
                function,
                signature,
                arguments,
                variadic_boundary,
                effects,
            }
        } else {
            FullInstructionKind::IndirectCall {
                callee: callee_value.expect("indirect callee was lowered"),
                signature,
                arguments,
                variadic_boundary,
                effects,
            }
        };
        let result = if is_void(self.types, function_type.result.ty) {
            self.emit_effect(kind, span)?;
            None
        } else {
            Some(self.emit_result(kind, result_ty, span)?)
        };
        if effects.no_return {
            self.terminate(FullTerminator::Unreachable)?;
        }
        Ok(result)
    }
}

fn scalar_conversion(kind: ConversionKind) -> Option<ScalarConversion> {
    Some(match kind {
        ConversionKind::LvalueToValue { .. } => return None,
        ConversionKind::ArrayToPointer => ScalarConversion::ArrayToPointer,
        ConversionKind::FunctionToPointer => ScalarConversion::FunctionToPointer,
        ConversionKind::IntegerPromotion => ScalarConversion::IntegerPromotion,
        ConversionKind::IntegerConversion => ScalarConversion::IntegerConversion,
        ConversionKind::FloatingConversion => ScalarConversion::FloatingConversion,
        ConversionKind::IntegerToFloating => ScalarConversion::IntegerToFloating,
        ConversionKind::FloatingToInteger => ScalarConversion::FloatingToInteger,
        ConversionKind::PointerConversion => ScalarConversion::PointerConversion,
        ConversionKind::QualificationAdjustment => ScalarConversion::QualificationAdjustment,
        ConversionKind::ToBoolean => ScalarConversion::ToBoolean,
        ConversionKind::ToVoid => ScalarConversion::ToVoid,
    })
}

fn inferred_conversion(
    types: &TypeStore,
    from: QualifiedType,
    to: QualifiedType,
) -> ScalarConversion {
    let from_float = types
        .builtin_type(from.ty)
        .is_some_and(|builtin| builtin.is_floating());
    let to_float = types
        .builtin_type(to.ty)
        .is_some_and(|builtin| builtin.is_floating());
    match (
        types.is_integer(from.ty),
        from_float,
        types.is_integer(to.ty),
        to_float,
    ) {
        (true, _, _, true) => ScalarConversion::IntegerToFloating,
        (_, true, true, _) => ScalarConversion::FloatingToInteger,
        (_, true, _, true) => ScalarConversion::FloatingConversion,
        (true, _, true, _) => ScalarConversion::IntegerPromotion,
        _ => ScalarConversion::QualificationAdjustment,
    }
}

fn unary_operation(operator: AstUnary) -> Option<UnaryOperation> {
    Some(match operator {
        AstUnary::Plus => UnaryOperation::Plus,
        AstUnary::Minus => UnaryOperation::Negate,
        AstUnary::BitwiseNot => UnaryOperation::BitwiseNot,
        AstUnary::LogicalNot => UnaryOperation::LogicalNot,
        AstUnary::PrefixIncrement
        | AstUnary::PrefixDecrement
        | AstUnary::Address
        | AstUnary::Dereference => return None,
    })
}

fn binary_operation(operator: AstBinary) -> Option<BinaryOperation> {
    Some(match operator {
        AstBinary::Multiply => BinaryOperation::Multiply,
        AstBinary::Divide => BinaryOperation::Divide,
        AstBinary::Remainder => BinaryOperation::Remainder,
        AstBinary::Add => BinaryOperation::Add,
        AstBinary::Subtract => BinaryOperation::Subtract,
        AstBinary::LeftShift => BinaryOperation::LeftShift,
        AstBinary::RightShift => BinaryOperation::RightShift,
        AstBinary::Less => BinaryOperation::Less,
        AstBinary::LessEqual => BinaryOperation::LessEqual,
        AstBinary::Greater => BinaryOperation::Greater,
        AstBinary::GreaterEqual => BinaryOperation::GreaterEqual,
        AstBinary::Equal => BinaryOperation::Equal,
        AstBinary::NotEqual => BinaryOperation::NotEqual,
        AstBinary::BitwiseAnd => BinaryOperation::BitwiseAnd,
        AstBinary::BitwiseXor => BinaryOperation::BitwiseXor,
        AstBinary::BitwiseOr => BinaryOperation::BitwiseOr,
        AstBinary::LogicalAnd | AstBinary::LogicalOr => return None,
    })
}

fn merge_access(left: MemoryAccess, right: MemoryAccess) -> MemoryAccess {
    let volatile = left.volatile || right.volatile;
    let atomic = left.atomic.or(right.atomic);
    let ordered = volatile || atomic.is_some();
    MemoryAccess {
        volatile,
        atomic,
        non_elidable: ordered || left.non_elidable || right.non_elidable,
        non_movable: ordered || left.non_movable || right.non_movable,
    }
}

fn collect_switch_labels(
    statement: &FullTypedStatement,
    cases: &mut Vec<i128>,
    has_default: &mut bool,
) {
    use FullTypedStatementKind as S;
    match &statement.kind {
        S::Case { value, statement } => {
            cases.push(*value);
            collect_switch_labels(statement, cases, has_default);
        }
        S::Default(statement) => {
            *has_default = true;
            collect_switch_labels(statement, cases, has_default);
        }
        S::Label { statement, .. }
        | S::While { statement, .. }
        | S::DoWhile { statement, .. }
        | S::For { statement, .. } => collect_switch_labels(statement, cases, has_default),
        S::Compound(items) => {
            for item in items {
                if let FullTypedBlockItem::Statement(statement) = item {
                    collect_switch_labels(statement, cases, has_default);
                }
            }
        }
        S::If {
            then_statement,
            else_statement,
            ..
        } => {
            collect_switch_labels(then_statement, cases, has_default);
            if let Some(statement) = else_statement {
                collect_switch_labels(statement, cases, has_default);
            }
        }
        // Labels in a nested switch belong to that switch.
        S::Switch { .. }
        | S::Expression(_)
        | S::Goto { .. }
        | S::ComputedGoto(_)
        | S::Continue
        | S::Break
        | S::Return(_) => {}
    }
}

fn empty_edge(target: BlockId) -> FullEdge {
    FullEdge {
        target,
        arguments: Vec::new(),
    }
}

fn collect_labels(statement: &FullTypedStatement, labels: &mut Vec<LabelId>) {
    use FullTypedStatementKind as S;
    match &statement.kind {
        S::Label {
            label, statement, ..
        } => {
            labels.push(*label);
            collect_labels(statement, labels);
        }
        S::Case { statement, .. } | S::Default(statement) => collect_labels(statement, labels),
        S::Compound(items) => {
            for item in items {
                if let FullTypedBlockItem::Statement(statement) = item {
                    collect_labels(statement, labels);
                }
            }
        }
        S::If {
            then_statement,
            else_statement,
            ..
        } => {
            collect_labels(then_statement, labels);
            if let Some(statement) = else_statement {
                collect_labels(statement, labels);
            }
        }
        S::Switch { statement, .. }
        | S::While { statement, .. }
        | S::DoWhile { statement, .. }
        | S::For { statement, .. } => collect_labels(statement, labels),
        S::Expression(_)
        | S::Goto { .. }
        | S::ComputedGoto(_)
        | S::Continue
        | S::Break
        | S::Return(_) => {}
    }
}

fn bitfield_descriptor(bitfield: &BitfieldPlace) -> BitfieldDescriptor {
    BitfieldDescriptor {
        field_index: bitfield.field_index,
        storage_offset: bitfield.storage_offset,
        storage_size: bitfield.storage_size,
        storage_align: bitfield.storage_align,
        bit_offset: bitfield.bit_offset,
        width: bitfield.width,
        signed: bitfield.signed,
    }
}

fn access_from_semantics(access: AccessSemantics) -> MemoryAccess {
    let ordered = access.volatile || access.atomic;
    MemoryAccess {
        volatile: access.volatile,
        atomic: access.atomic.then_some(MemoryOrder::SequentiallyConsistent),
        non_elidable: ordered,
        non_movable: ordered,
    }
}

fn access_from_qualified(ty: QualifiedType) -> MemoryAccess {
    access_from_semantics(AccessSemantics {
        volatile: ty.qualifiers.contains(TypeQualifiers::VOLATILE),
        atomic: ty.qualifiers.contains(TypeQualifiers::ATOMIC),
    })
}

fn is_void(types: &TypeStore, ty: TypeId) -> bool {
    types.builtin_type(ty) == Some(BuiltinType::Void)
}

fn is_aggregate(types: &TypeStore, ty: TypeId) -> bool {
    matches!(
        types.try_kind(ty),
        Some(TypeKind::Array(_) | TypeKind::Record(_))
    )
}

fn pointer_pointee(types: &TypeStore, ty: TypeId) -> Option<QualifiedType> {
    match types.try_kind(ty) {
        Some(TypeKind::Pointer(pointer)) => Some(pointer.pointee),
        _ => None,
    }
}

fn string_copy_code_units(
    types: &TypeStore,
    unit: &FullTypedTranslationUnit,
    object: QualifiedType,
    string: StringId,
    span: Span,
) -> Result<u64, IrError> {
    let literal_length = unit
        .strings
        .get(string.0 as usize)
        .filter(|candidate| candidate.id == string)
        .map(|literal| literal.code_units.len() as u64)
        .ok_or_else(|| {
            IrError::lower(
                LOWERING_ERROR,
                span,
                format!("initializer references unknown string {}", string.0),
            )
        })?;
    let bound = match types.try_kind(object.ty) {
        Some(TypeKind::Array(array)) => match array.length {
            ArrayLength::Constant(bound) => bound,
            ArrayLength::Incomplete
            | ArrayLength::Variable(_)
            | ArrayLength::UnspecifiedVariable(_) => {
                return Err(IrError::lower(
                    LOWERING_ERROR,
                    span,
                    "string initializer destination does not have a constant bound",
                ));
            }
        },
        _ => {
            return Err(IrError::lower(
                LOWERING_ERROR,
                span,
                "string initializer destination is not an array",
            ));
        }
    };
    Ok(bound.min(literal_length))
}
