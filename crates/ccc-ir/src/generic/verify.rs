use std::collections::{HashSet, VecDeque};

use ccc_types::{
    ArrayLength, BuiltinType, FunctionParameters, QualifiedType, TargetBuiltinType, TypeId,
    TypeKind, TypeQualifiers, TypeStore,
};

use super::{
    AggregateProjection, BlockId, CallEffects, DataOrigin, FullEdge, FullFunction, FullInstruction,
    FullInstructionKind, FullModule, FullTerminator, InitializerGraph, InitializerNodeId,
    InitializerNodeKind, InitializerPath, IrError, MemoryAccess, MemoryResidencyReason,
    RelocationKind, RelocationTarget, ScalarConstant, ScalarConversion, StorageLocation, ValueId,
};

#[derive(Clone, Copy, Debug)]
enum Definition {
    BlockParameter(BlockId),
    Instruction { block: BlockId, position: usize },
}

pub fn verify_frontend(module: &FullModule) -> Result<(), IrError> {
    verify_module_arenas(module)?;
    for global in &module.globals {
        verify_type(&module.types, global.ty, "global object")?;
        if global.emission.binding == ccc_sema::generic::SymbolBinding::Weak {
            if global.linkage != ccc_sema::generic::Linkage::External {
                return Err(IrError::verify(
                    "weak data object does not have external linkage",
                ));
            }
            if global.emission.definition
                == ccc_sema::generic::ObjectDefinitionPolicy::TentativeCommon
            {
                return Err(IrError::verify(
                    "weak data object cannot use tentative common emission",
                ));
            }
        }
        if let Some(initializer) = &global.initializer {
            verify_initializer(module, initializer, global.ty)?;
        }
    }
    for string in &module.strings {
        verify_type(&module.types, string.ty, "string object")?;
        if !matches!(
            module.types.try_kind(string.ty.ty),
            Some(TypeKind::Array(_))
        ) {
            return Err(IrError::verify(format!(
                "string {} does not have array type",
                string.id.0
            )));
        }
        if string.code_units.last().copied() != Some(0) {
            return Err(IrError::verify(format!(
                "string {} does not include its trailing zero code unit",
                string.id.0
            )));
        }
    }
    for function in &module.functions {
        verify_function(module, function)?;
    }
    Ok(())
}

fn verify_module_arenas(module: &FullModule) -> Result<(), IrError> {
    let mut origins = HashSet::new();
    for (index, global) in module.globals.iter().enumerate() {
        if global.id.0 as usize != index {
            return Err(IrError::verify(format!(
                "data object id {} does not match arena index {index}",
                global.id.0
            )));
        }
        let origin_key = match global.source {
            DataOrigin::FileScope(id) => (0_u8, id.0, 0_u32),
            DataOrigin::BlockStatic { function, local } => (1_u8, function.0, local.0),
        };
        if !origins.insert(origin_key) {
            return Err(IrError::verify(format!(
                "data object {} duplicates a source object",
                global.id.0
            )));
        }
    }
    for (index, string) in module.strings.iter().enumerate() {
        if string.id.0 as usize != index {
            return Err(IrError::verify(format!(
                "string id {} does not match arena index {index}",
                string.id.0
            )));
        }
    }
    for (index, function) in module.functions.iter().enumerate() {
        if function.id.0 as usize != index {
            return Err(IrError::verify(format!(
                "function id {} does not match arena index {index}",
                function.id.0
            )));
        }
    }
    Ok(())
}

fn verify_initializer(
    module: &FullModule,
    graph: &InitializerGraph,
    object: QualifiedType,
) -> Result<(), IrError> {
    let root = graph
        .nodes
        .get(graph.root.0 as usize)
        .filter(|node| node.id == graph.root)
        .ok_or_else(|| IrError::verify("initializer graph has an invalid root node"))?;
    if !same_type(root.ty, object) {
        return Err(IrError::verify(
            "initializer root type does not match its data object",
        ));
    }
    let mut referenced = vec![false; graph.nodes.len()];
    let mut visiting = vec![false; graph.nodes.len()];
    verify_initializer_node(module, graph, graph.root, &mut referenced, &mut visiting)?;
    if referenced.iter().any(|visited| !visited) {
        return Err(IrError::verify(
            "initializer graph contains a node unreachable from its root",
        ));
    }
    Ok(())
}

fn verify_initializer_node(
    module: &FullModule,
    graph: &InitializerGraph,
    id: InitializerNodeId,
    referenced: &mut [bool],
    visiting: &mut [bool],
) -> Result<(), IrError> {
    let index = id.0 as usize;
    let node = graph
        .nodes
        .get(index)
        .filter(|node| node.id == id)
        .ok_or_else(|| IrError::verify(format!("initializer references unknown node {}", id.0)))?;
    if visiting[index] {
        return Err(IrError::verify("initializer graph contains a cycle"));
    }
    if referenced[index] {
        return Ok(());
    }
    visiting[index] = true;
    verify_type(&module.types, node.ty, "initializer node")?;
    match &node.kind {
        InitializerNodeKind::Zero => {}
        InitializerNodeKind::Scalar(constant) => {
            if is_aggregate(&module.types, node.ty.ty) {
                return Err(IrError::verify(
                    "aggregate initializer node contains a scalar leaf directly",
                ));
            }
            verify_long_double_constant(&module.types, node.ty.ty, *constant)?;
        }
        InitializerNodeKind::Relocation { target, kind, .. } => {
            verify_relocation(module, *target, *kind)?;
            if !is_pointer_or_integer(&module.types, node.ty.ty) {
                return Err(IrError::verify(
                    "relocation initializer does not have pointer or integer type",
                ));
            }
        }
        InitializerNodeKind::StringData {
            string,
            copy_code_units,
        } => {
            let literal = module
                .strings
                .get(string.0 as usize)
                .filter(|literal| literal.id == *string)
                .ok_or_else(|| {
                    IrError::verify(format!(
                        "initializer references unknown string {}",
                        string.0
                    ))
                })?;
            let bound = array_bound(&module.types, node.ty.ty).ok_or_else(|| {
                IrError::verify("string-data initializer does not initialize a constant array")
            })?;
            if *copy_code_units > bound || *copy_code_units as usize > literal.code_units.len() {
                return Err(IrError::verify(
                    "string-data copy count exceeds the destination or literal",
                ));
            }
        }
        InitializerNodeKind::Repeat { element, count } => {
            let Some(TypeKind::Array(array)) = module.types.try_kind(node.ty.ty) else {
                return Err(IrError::verify(
                    "repeated initializer node does not have array type",
                ));
            };
            let ArrayLength::Constant(bound) = array.length else {
                return Err(IrError::verify(
                    "repeated initializer node does not have a constant array bound",
                ));
            };
            if *count == 0 || *count > bound {
                return Err(IrError::verify(
                    "repeated initializer count is zero or exceeds its array bound",
                ));
            }
            let child = graph
                .nodes
                .get(element.0 as usize)
                .filter(|child| child.id == *element)
                .ok_or_else(|| {
                    IrError::verify(format!(
                        "repeated initializer references unknown node {}",
                        element.0
                    ))
                })?;
            if element.0 >= node.id.0 {
                return Err(IrError::verify(
                    "initializer graph is not in stable child-before-parent order",
                ));
            }
            if !same_type(array.element, child.ty) {
                return Err(IrError::verify(
                    "repeated initializer element type does not match its array element",
                ));
            }
            verify_initializer_node(module, graph, *element, referenced, visiting)?;
        }
        InitializerNodeKind::Aggregate(edges) => {
            if !is_aggregate(&module.types, node.ty.ty) {
                return Err(IrError::verify(
                    "aggregate initializer node does not have aggregate type",
                ));
            }
            for edge in edges {
                let child = graph
                    .nodes
                    .get(edge.node.0 as usize)
                    .filter(|node| node.id == edge.node)
                    .ok_or_else(|| {
                        IrError::verify(format!(
                            "initializer edge references unknown node {}",
                            edge.node.0
                        ))
                    })?;
                if edge.node.0 >= node.id.0 {
                    return Err(IrError::verify(
                        "initializer graph is not in stable child-before-parent order",
                    ));
                }
                let selected = initializer_path_type(&module.types, node.ty, &edge.path)?;
                if !same_type(selected, child.ty) {
                    return Err(IrError::verify(
                        "initializer edge type does not match its selected subobject",
                    ));
                }
                verify_initializer_node(module, graph, edge.node, referenced, visiting)?;
            }
        }
    }
    visiting[index] = false;
    referenced[index] = true;
    Ok(())
}

fn verify_relocation(
    module: &FullModule,
    target: RelocationTarget,
    kind: RelocationKind,
) -> Result<(), IrError> {
    match (target, kind) {
        (RelocationTarget::Object(id), RelocationKind::ObjectAddress) => {
            let object = module
                .globals
                .get(id.0 as usize)
                .filter(|object| object.id == id)
                .ok_or_else(|| {
                    IrError::verify(format!("relocation references unknown data {}", id.0))
                })?;
            if object.duration == ccc_sema::generic::StorageDuration::Thread {
                return Err(IrError::verify(
                    "thread-local object uses a non-TLS relocation kind",
                ));
            }
        }
        (RelocationTarget::Object(id), RelocationKind::ThreadLocalAddress) => {
            let object = module
                .globals
                .get(id.0 as usize)
                .filter(|object| object.id == id)
                .ok_or_else(|| {
                    IrError::verify(format!("relocation references unknown data {}", id.0))
                })?;
            if object.duration != ccc_sema::generic::StorageDuration::Thread {
                return Err(IrError::verify(
                    "non-thread object uses a TLS relocation kind",
                ));
            }
        }
        (RelocationTarget::Function(id), RelocationKind::FunctionAddress) => {
            if module
                .functions
                .get(id.0 as usize)
                .is_none_or(|function| function.id != id)
            {
                return Err(IrError::verify(format!(
                    "relocation references unknown function {}",
                    id.0
                )));
            }
        }
        (RelocationTarget::String(id), RelocationKind::StringAddress) => {
            if module
                .strings
                .get(id.0 as usize)
                .is_none_or(|string| string.id != id)
            {
                return Err(IrError::verify(format!(
                    "relocation references unknown string {}",
                    id.0
                )));
            }
        }
        _ => return Err(IrError::verify("relocation target and kind disagree")),
    }
    Ok(())
}

fn initializer_path_type(
    types: &TypeStore,
    mut ty: QualifiedType,
    path: &[InitializerPath],
) -> Result<QualifiedType, IrError> {
    for (position, element) in path.iter().enumerate() {
        match element {
            InitializerPath::Index(index) => {
                let Some(TypeKind::Array(array)) = types.try_kind(ty.ty) else {
                    return Err(IrError::verify(
                        "initializer index path traverses a non-array type",
                    ));
                };
                if let ArrayLength::Constant(bound) = array.length
                    && *index >= bound
                {
                    return Err(IrError::verify(
                        "initializer index path exceeds its array bound",
                    ));
                }
                ty = array.element;
            }
            InitializerPath::Field {
                index,
                name,
                bitfield,
            } => {
                let Some(TypeKind::Record(record)) = types.try_kind(ty.ty) else {
                    return Err(IrError::verify(
                        "initializer field path traverses a non-record type",
                    ));
                };
                let field = types
                    .record(*record)
                    .and_then(|record| record.fields.as_ref())
                    .and_then(|fields| fields.get(*index))
                    .ok_or_else(|| IrError::verify("initializer path references unknown field"))?;
                if name
                    .as_ref()
                    .is_some_and(|name| field.name.as_ref() != Some(name))
                {
                    return Err(IrError::verify(
                        "initializer path field name does not match its field index",
                    ));
                }
                if field.bitfield.is_some() != bitfield.is_some() {
                    return Err(IrError::verify(
                        "initializer path does not preserve its bitfield descriptor",
                    ));
                }
                if let Some(bitfield) = bitfield {
                    verify_bitfield(*bitfield)?;
                    if position + 1 != path.len() {
                        return Err(IrError::verify(
                            "initializer path continues through a bitfield",
                        ));
                    }
                }
                ty = field.ty;
            }
        }
    }
    Ok(ty)
}

fn verify_function(module: &FullModule, function: &FullFunction) -> Result<(), IrError> {
    if function.binding == ccc_sema::generic::SymbolBinding::Weak
        && function.linkage != ccc_sema::generic::Linkage::External
    {
        return Err(IrError::verify(
            "weak function does not have external linkage",
        ));
    }
    if function.properties.always_inline && function.properties.no_inline {
        return Err(IrError::verify(format!(
            "function `{}` is both always-inline and noinline",
            function.name
        )));
    }
    let signature = module
        .types
        .function_signature(function.signature)
        .ok_or_else(|| {
            IrError::verify(format!(
                "function `{}` has a non-function signature",
                function.name
            ))
        })?;
    if !same_type(signature.result, function.result_type) {
        return Err(IrError::verify(format!(
            "function `{}` result type disagrees with its signature",
            function.name
        )));
    }
    if function.entry.is_some()
        && let FunctionParameters::Prototype(parameters) = &signature.parameters
    {
        if parameters.len() != function.parameters.len() {
            return Err(IrError::verify(format!(
                "function `{}` parameter count disagrees with its signature",
                function.name
            )));
        }
        for (parameter, expected) in function.parameters.iter().zip(parameters) {
            if parameter.ty.ty != expected.ty {
                return Err(IrError::verify(format!(
                    "function `{}` parameter `{}` has the wrong type",
                    function.name, parameter.name
                )));
            }
        }
    }
    if function.entry.is_none() {
        if !function.blocks.is_empty()
            || !function.storage.is_empty()
            || !function.value_types.is_empty()
            || function.instruction_count != 0
            || function
                .parameters
                .iter()
                .any(|parameter| parameter.incoming.is_some() || parameter.storage.is_some())
        {
            return Err(IrError::verify(format!(
                "function declaration `{}` contains definition state",
                function.name
            )));
        }
        return Ok(());
    }
    let entry = function.entry.expect("entry was checked");
    if function
        .blocks
        .get(entry.0 as usize)
        .is_none_or(|block| block.id != entry)
    {
        return Err(IrError::verify(format!(
            "function `{}` has an invalid entry block",
            function.name
        )));
    }
    for (index, storage) in function.storage.iter().enumerate() {
        if storage.id.0 as usize != index {
            return Err(IrError::verify(format!(
                "function `{}` storage id {} is not stable",
                function.name, storage.id.0
            )));
        }
        if !matches!(
            storage.location,
            StorageLocation::Automatic | StorageLocation::RuntimeSized
        ) {
            return Err(IrError::verify(format!(
                "function `{}` retains static-duration data in runtime storage",
                function.name
            )));
        }
        verify_type(&module.types, storage.ty, "local storage")?;
        if storage
            .requested_alignment
            .is_some_and(|alignment| !alignment.is_power_of_two())
        {
            return Err(IrError::verify(format!(
                "function `{}` storage {} has an invalid requested alignment",
                function.name, storage.id.0
            )));
        }
        if storage.required_by.is_empty() {
            return Err(IrError::verify(format!(
                "function `{}` storage {} has no residency classification",
                function.name, storage.id.0
            )));
        }
    }

    let contains_returns_twice = function.blocks.iter().any(|block| {
        block.instructions.iter().any(|instruction| {
            matches!(
                &instruction.kind,
                FullInstructionKind::DirectCall { effects, .. }
                    | FullInstructionKind::IndirectCall { effects, .. }
                    if effects.returns_twice
            )
        })
    });
    if contains_returns_twice {
        for storage in &function.storage {
            if storage.location == StorageLocation::RuntimeSized {
                return Err(IrError::verify(
                    "returns-twice function contains runtime-sized automatic storage",
                ));
            }
            if storage.location == StorageLocation::Automatic
                && !storage
                    .required_by
                    .contains(&MemoryResidencyReason::ReturnsTwice)
            {
                return Err(IrError::verify(
                    "automatic storage across a returns-twice call is not memory-resident",
                ));
            }
        }
    }

    let mut definitions = vec![None; function.value_types.len()];
    let mut instruction_ids = vec![false; function.instruction_count as usize];
    for (index, block) in function.blocks.iter().enumerate() {
        if block.id.0 as usize != index {
            return Err(IrError::verify(format!(
                "function `{}` block id {} is not stable",
                function.name, block.id.0
            )));
        }
        if block.terminator.is_none() {
            return Err(IrError::verify(format!(
                "function `{}` block {} has no terminator",
                function.name, block.id.0
            )));
        }
        for value in &block.parameters {
            define_value(
                &mut definitions,
                *value,
                Definition::BlockParameter(block.id),
                function,
            )?;
        }
        for (position, instruction) in block.instructions.iter().enumerate() {
            let id_slot = instruction_ids
                .get_mut(instruction.id.0 as usize)
                .ok_or_else(|| {
                    IrError::verify(format!(
                        "function `{}` has out-of-range instruction id {}",
                        function.name, instruction.id.0
                    ))
                })?;
            if *id_slot {
                return Err(IrError::verify(format!(
                    "function `{}` defines instruction {} twice",
                    function.name, instruction.id.0
                )));
            }
            *id_slot = true;
            if let Some(value) = instruction.result {
                define_value(
                    &mut definitions,
                    value,
                    Definition::Instruction {
                        block: block.id,
                        position,
                    },
                    function,
                )?;
            }
        }
    }
    if instruction_ids.iter().any(|seen| !seen) {
        return Err(IrError::verify(format!(
            "function `{}` instruction ids are not dense",
            function.name
        )));
    }
    if definitions.iter().any(Option::is_none) {
        return Err(IrError::verify(format!(
            "function `{}` value ids are not densely defined",
            function.name
        )));
    }
    for parameter in &function.parameters {
        let incoming = parameter.incoming.ok_or_else(|| {
            IrError::verify(format!(
                "function `{}` parameter `{}` has no incoming value",
                function.name, parameter.name
            ))
        })?;
        if !function.blocks[entry.0 as usize]
            .parameters
            .contains(&incoming)
            || value_type(function, incoming)?.ty != parameter.ty.ty
        {
            return Err(IrError::verify(format!(
                "function `{}` parameter `{}` is not an entry block parameter",
                function.name, parameter.name
            )));
        }
    }

    let predecessors = predecessors(function)?;
    let dominators = dominators(function, entry, &predecessors);
    let verifier = FunctionVerifier {
        module,
        function,
        definitions: &definitions,
        dominators: &dominators,
    };
    for block in &function.blocks {
        for (position, instruction) in block.instructions.iter().enumerate() {
            verifier.instruction(block.id, position, instruction)?;
        }
        verifier.terminator(
            block.id,
            block.terminator.as_ref().expect("terminator was checked"),
        )?;
    }
    Ok(())
}

fn define_value(
    definitions: &mut [Option<Definition>],
    value: ValueId,
    definition: Definition,
    function: &FullFunction,
) -> Result<(), IrError> {
    let slot = definitions.get_mut(value.0 as usize).ok_or_else(|| {
        IrError::verify(format!(
            "function `{}` defines out-of-range value {}",
            function.name, value.0
        ))
    })?;
    if slot.is_some() {
        return Err(IrError::verify(format!(
            "function `{}` defines value {} more than once",
            function.name, value.0
        )));
    }
    *slot = Some(definition);
    Ok(())
}

fn predecessors(function: &FullFunction) -> Result<Vec<Vec<BlockId>>, IrError> {
    let mut predecessors = vec![Vec::new(); function.blocks.len()];
    for block in &function.blocks {
        let terminator = block.terminator.as_ref().expect("terminator was checked");
        for edge in terminator_edges(terminator) {
            let slot = predecessors
                .get_mut(edge.target.0 as usize)
                .ok_or_else(|| {
                    IrError::verify(format!(
                        "function `{}` branches to unknown block {}",
                        function.name, edge.target.0
                    ))
                })?;
            slot.push(block.id);
        }
    }
    Ok(predecessors)
}

fn dominators(
    function: &FullFunction,
    entry: BlockId,
    predecessors: &[Vec<BlockId>],
) -> Vec<HashSet<BlockId>> {
    let mut reachable = HashSet::new();
    let mut queue = VecDeque::from([entry]);
    while let Some(block) = queue.pop_front() {
        if !reachable.insert(block) {
            continue;
        }
        let terminator = function.blocks[block.0 as usize]
            .terminator
            .as_ref()
            .expect("terminator was checked");
        queue.extend(terminator_edges(terminator).map(|edge| edge.target));
    }
    let mut result = vec![HashSet::new(); function.blocks.len()];
    for block in &function.blocks {
        if block.id == entry || !reachable.contains(&block.id) {
            result[block.id.0 as usize].insert(block.id);
        } else {
            result[block.id.0 as usize] = reachable.clone();
        }
    }
    loop {
        let mut changed = false;
        for block in &function.blocks {
            if block.id == entry || !reachable.contains(&block.id) {
                continue;
            }
            let reachable_predecessors = predecessors[block.id.0 as usize]
                .iter()
                .filter(|predecessor| reachable.contains(predecessor));
            let mut intersection: Option<HashSet<BlockId>> = None;
            for predecessor in reachable_predecessors {
                intersection = Some(match intersection {
                    None => result[predecessor.0 as usize].clone(),
                    Some(current) => current
                        .intersection(&result[predecessor.0 as usize])
                        .copied()
                        .collect(),
                });
            }
            let mut next = intersection.unwrap_or_default();
            next.insert(block.id);
            if next != result[block.id.0 as usize] {
                result[block.id.0 as usize] = next;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    result
}

struct FunctionVerifier<'a> {
    module: &'a FullModule,
    function: &'a FullFunction,
    definitions: &'a [Option<Definition>],
    dominators: &'a [HashSet<BlockId>],
}

impl FunctionVerifier<'_> {
    fn instruction(
        &self,
        block: BlockId,
        position: usize,
        instruction: &FullInstruction,
    ) -> Result<(), IrError> {
        for operand in instruction_operands(&instruction.kind) {
            self.use_value(block, position, operand)?;
        }
        self.instruction_types(block, position, instruction)
    }

    fn instruction_types(
        &self,
        block: BlockId,
        position: usize,
        instruction: &FullInstruction,
    ) -> Result<(), IrError> {
        let types = &self.module.types;
        let result = instruction
            .result
            .map(|value| value_type(self.function, value))
            .transpose()?;
        match &instruction.kind {
            FullInstructionKind::Constant(constant) => {
                let ty = require_result(result, instruction, "constant")?;
                if is_aggregate(types, ty.ty) || is_void(types, ty.ty) {
                    return Err(IrError::verify("constant has a non-scalar result type"));
                }
                verify_long_double_constant(types, ty.ty, *constant)?;
            }
            FullInstructionKind::AddressConstant { target, .. } => {
                let ty = require_result(result, instruction, "address constant")?;
                self.verify_target(*target)?;
                if !is_pointer_or_integer(types, ty.ty) {
                    return Err(IrError::verify(
                        "address constant has neither pointer nor integer type",
                    ));
                }
            }
            FullInstructionKind::AddressOfGlobal { global } => {
                let object = self
                    .module
                    .globals
                    .get(global.0 as usize)
                    .filter(|object| object.id == *global)
                    .ok_or_else(|| {
                        IrError::verify(format!("address references unknown data {}", global.0))
                    })?;
                require_pointer_result(types, result, object.ty, instruction)?;
            }
            FullInstructionKind::AddressOfFunction {
                function,
                signature,
            } => {
                let declaration = self
                    .module
                    .functions
                    .get(function.0 as usize)
                    .filter(|declaration| declaration.id == *function)
                    .ok_or_else(|| {
                        IrError::verify(format!(
                            "address references unknown function {}",
                            function.0
                        ))
                    })?;
                if declaration.signature != *signature {
                    return Err(IrError::verify(
                        "function address carries the wrong signature",
                    ));
                }
                require_pointer_result(
                    types,
                    result,
                    QualifiedType::unqualified(*signature),
                    instruction,
                )?;
            }
            FullInstructionKind::AddressOfString { string } => {
                let literal = self
                    .module
                    .strings
                    .get(string.0 as usize)
                    .filter(|literal| literal.id == *string)
                    .ok_or_else(|| {
                        IrError::verify(format!("address references unknown string {}", string.0))
                    })?;
                require_pointer_result(types, result, literal.ty, instruction)?;
            }
            FullInstructionKind::AddressOfStorage { storage } => {
                let object = self
                    .function
                    .storage
                    .get(storage.0 as usize)
                    .filter(|object| object.id == *storage)
                    .ok_or_else(|| {
                        IrError::verify(format!("address references unknown storage {}", storage.0))
                    })?;
                if object.location == StorageLocation::RuntimeSized {
                    return Err(IrError::verify(
                        "runtime-sized storage must be addressed by its allocation result",
                    ));
                }
                require_pointer_result(types, result, object.ty, instruction)?;
            }
            FullInstructionKind::RuntimeSize {
                extents,
                element,
                constant_factor,
            } => {
                if extents.is_empty() || *constant_factor == 0 {
                    return Err(IrError::verify(
                        "runtime size has no dynamic extent or has a zero constant factor",
                    ));
                }
                for extent in extents {
                    if !types.is_integer(self.value_type(*extent)?.ty) {
                        return Err(IrError::verify("runtime size extent is not an integer"));
                    }
                }
                verify_type(types, *element, "runtime size element")?;
                let result = require_result(result, instruction, "runtime size")?;
                if result.ty != TypeId::UNSIGNED_LONG {
                    return Err(IrError::verify(
                        "runtime size result does not have size_t type",
                    ));
                }
            }
            FullInstructionKind::RuntimeSizedAllocate {
                storage,
                size,
                element,
                requested_alignment,
            } => {
                let object = self
                    .function
                    .storage
                    .get(storage.0 as usize)
                    .filter(|object| object.id == *storage)
                    .ok_or_else(|| {
                        IrError::verify(format!(
                            "runtime allocation references unknown storage {}",
                            storage.0
                        ))
                    })?;
                if object.location != StorageLocation::RuntimeSized {
                    return Err(IrError::verify(
                        "runtime allocation references fixed-size storage",
                    ));
                }
                if self.value_type(*size)?.ty != TypeId::UNSIGNED_LONG {
                    return Err(IrError::verify(
                        "runtime allocation size does not have size_t type",
                    ));
                }
                verify_type(types, *element, "runtime allocation element")?;
                if requested_alignment.is_some_and(|alignment| !alignment.is_power_of_two()) {
                    return Err(IrError::verify(
                        "runtime allocation has an invalid requested alignment",
                    ));
                }
                require_pointer_result(types, result, object.ty, instruction)?;
            }
            FullInstructionKind::ProjectField {
                base,
                record,
                field_index,
                field_name,
            } => {
                require_address(types, self.value_type(*base)?, *record, "field projection")?;
                let Some(TypeKind::Record(record_id)) = types.try_kind(record.ty) else {
                    return Err(IrError::verify(
                        "field projection record operand is not a record",
                    ));
                };
                let field = types
                    .record(*record_id)
                    .and_then(|record| record.fields.as_ref())
                    .and_then(|fields| fields.get(*field_index))
                    .ok_or_else(|| {
                        IrError::verify("field projection has an invalid field index")
                    })?;
                if field_name
                    .as_ref()
                    .is_some_and(|name| field.name.as_ref() != Some(name))
                {
                    return Err(IrError::verify(
                        "field projection name disagrees with its field index",
                    ));
                }
                let projected =
                    QualifiedType::new(field.ty.ty, field.ty.qualifiers | record.qualifiers);
                require_pointer_result(types, result, projected, instruction)?;
            }
            FullInstructionKind::PointerOffset {
                base,
                index,
                element,
                ..
            } => {
                require_address(types, self.value_type(*base)?, *element, "pointer offset")?;
                if !types.is_integer(self.value_type(*index)?.ty) {
                    return Err(IrError::verify("pointer offset index is not an integer"));
                }
                require_pointer_result(types, result, *element, instruction)?;
            }
            FullInstructionKind::RuntimePointerOffset {
                base,
                index,
                element,
                stride,
                ..
            } => {
                require_address(types, self.value_type(*base)?, *element, "pointer offset")?;
                if !types.is_integer(self.value_type(*index)?.ty) {
                    return Err(IrError::verify(
                        "runtime pointer offset index is not an integer",
                    ));
                }
                if self.value_type(*stride)?.ty != TypeId::UNSIGNED_LONG {
                    return Err(IrError::verify(
                        "runtime pointer offset stride does not have size_t type",
                    ));
                }
                require_pointer_result(types, result, *element, instruction)?;
            }
            FullInstructionKind::PointerDifference {
                left,
                right,
                element,
            } => {
                require_address_ignoring_pointee_qualifiers(
                    types,
                    self.value_type(*left)?,
                    *element,
                    "pointer difference",
                )?;
                require_address_ignoring_pointee_qualifiers(
                    types,
                    self.value_type(*right)?,
                    *element,
                    "pointer difference",
                )?;
                let result = require_result(result, instruction, "pointer difference")?;
                if !types.is_integer(result.ty) {
                    return Err(IrError::verify(
                        "pointer difference result is not an integer",
                    ));
                }
            }
            FullInstructionKind::RuntimePointerDifference {
                left,
                right,
                element,
                stride,
            } => {
                require_address_ignoring_pointee_qualifiers(
                    types,
                    self.value_type(*left)?,
                    *element,
                    "pointer difference",
                )?;
                require_address_ignoring_pointee_qualifiers(
                    types,
                    self.value_type(*right)?,
                    *element,
                    "pointer difference",
                )?;
                if self.value_type(*stride)?.ty != TypeId::UNSIGNED_LONG {
                    return Err(IrError::verify(
                        "runtime pointer difference stride does not have size_t type",
                    ));
                }
                let result = require_result(result, instruction, "pointer difference")?;
                if !types.is_integer(result.ty) {
                    return Err(IrError::verify(
                        "pointer difference result is not an integer",
                    ));
                }
            }
            FullInstructionKind::Load {
                address,
                object,
                access,
            } => {
                verify_access(*access)?;
                require_address(types, self.value_type(*address)?, *object, "load")?;
                let result = require_result(result, instruction, "load")?;
                if result.ty != object.ty || is_aggregate(types, object.ty) {
                    return Err(IrError::verify(
                        "load result type disagrees with its object",
                    ));
                }
            }
            FullInstructionKind::Store {
                address,
                value,
                object,
                access,
            } => {
                require_no_result(result, instruction, "store")?;
                verify_access(*access)?;
                require_address(types, self.value_type(*address)?, *object, "store")?;
                if self.value_type(*value)?.ty != object.ty || is_aggregate(types, object.ty) {
                    return Err(IrError::verify(
                        "store value type disagrees with its object",
                    ));
                }
            }
            FullInstructionKind::BitfieldLoad {
                address,
                descriptor,
                access,
            } => {
                verify_access(*access)?;
                verify_bitfield(*descriptor)?;
                let pointee = pointer_pointee(types, self.value_type(*address)?.ty)
                    .ok_or_else(|| IrError::verify("bitfield load address is not a pointer"))?;
                let result = require_result(result, instruction, "bitfield load")?;
                if pointee.ty != result.ty {
                    return Err(IrError::verify(
                        "bitfield load result disagrees with its address pointee",
                    ));
                }
            }
            FullInstructionKind::BitfieldStore {
                address,
                value,
                descriptor,
                access,
            } => {
                require_no_result(result, instruction, "bitfield store")?;
                verify_access(*access)?;
                verify_bitfield(*descriptor)?;
                let pointee = pointer_pointee(types, self.value_type(*address)?.ty)
                    .ok_or_else(|| IrError::verify("bitfield store address is not a pointer"))?;
                if pointee.ty != self.value_type(*value)?.ty {
                    return Err(IrError::verify(
                        "bitfield store value disagrees with its field type",
                    ));
                }
            }
            FullInstructionKind::ZeroInitialize {
                destination,
                object,
            } => {
                require_no_result(result, instruction, "zero initialization")?;
                require_address(
                    types,
                    self.value_type(*destination)?,
                    *object,
                    "zero initialization",
                )?;
            }
            FullInstructionKind::StringInitialize {
                destination,
                string,
                object,
                copy_code_units,
            } => {
                require_no_result(result, instruction, "string initialization")?;
                require_address(
                    types,
                    self.value_type(*destination)?,
                    *object,
                    "string initialization",
                )?;
                let literal = self
                    .module
                    .strings
                    .get(string.0 as usize)
                    .filter(|literal| literal.id == *string)
                    .ok_or_else(|| {
                        IrError::verify("string initialization has invalid string id")
                    })?;
                let bound = array_bound(types, object.ty).ok_or_else(|| {
                    IrError::verify("string initialization destination is not a constant array")
                })?;
                if *copy_code_units > bound || *copy_code_units as usize > literal.code_units.len()
                {
                    return Err(IrError::verify(
                        "string initialization copy count is out of bounds",
                    ));
                }
            }
            FullInstructionKind::AggregateCopy {
                destination,
                source,
                destination_object,
                source_object,
                destination_access,
                source_access,
                ..
            } => {
                require_no_result(result, instruction, "aggregate copy")?;
                verify_access(*destination_access)?;
                verify_access(*source_access)?;
                if !is_aggregate(types, destination_object.ty)
                    || destination_object.ty != source_object.ty
                {
                    return Err(IrError::verify(
                        "aggregate copy objects do not have the same aggregate type",
                    ));
                }
                require_address(
                    types,
                    self.value_type(*destination)?,
                    *destination_object,
                    "aggregate copy destination",
                )?;
                let source_ty = self.value_type(*source)?;
                if source_ty.ty == source_object.ty {
                    if !self.is_owned_aggregate_value(*source)? {
                        return Err(IrError::verify(
                            "aggregate copy source is not owned aggregate storage",
                        ));
                    }
                } else {
                    require_address(types, source_ty, *source_object, "aggregate copy source")?;
                }
            }
            FullInstructionKind::AggregateSnapshot {
                source,
                object,
                access,
            } => {
                verify_access(*access)?;
                if !is_aggregate(types, object.ty) {
                    return Err(IrError::verify(
                        "aggregate value does not have aggregate type",
                    ));
                }
                require_address(
                    types,
                    self.value_type(*source)?,
                    *object,
                    "aggregate snapshot",
                )?;
                let result = require_result(result, instruction, "aggregate snapshot")?;
                if result.ty != object.ty {
                    return Err(IrError::verify("aggregate snapshot result type is wrong"));
                }
            }
            FullInstructionKind::AggregateProject {
                base,
                aggregate,
                projections,
            } => {
                if !is_aggregate(types, aggregate.ty) || self.value_type(*base)?.ty != aggregate.ty
                {
                    return Err(IrError::verify(
                        "aggregate projection base disagrees with its aggregate type",
                    ));
                }
                if !self.is_owned_aggregate_value(*base)? {
                    return Err(IrError::verify(
                        "aggregate projection base is not owned aggregate storage",
                    ));
                }
                if projections.is_empty() {
                    return Err(IrError::verify("aggregate projection path is empty"));
                }
                let mut projected = *aggregate;
                let mut bitfield_anchor = None;
                for (position, projection) in projections.iter().enumerate() {
                    projected = match projection {
                        AggregateProjection::Field {
                            index,
                            name,
                            bitfield,
                        } => {
                            let Some(TypeKind::Record(record)) = types.try_kind(projected.ty)
                            else {
                                return Err(IrError::verify(
                                    "aggregate field projection does not follow a record",
                                ));
                            };
                            let field = types
                                .record(*record)
                                .and_then(|record| record.fields.as_ref())
                                .and_then(|fields| fields.get(*index))
                                .ok_or_else(|| {
                                    IrError::verify(
                                        "aggregate projection has an invalid field index",
                                    )
                                })?;
                            if name
                                .as_ref()
                                .is_some_and(|name| field.name.as_ref() != Some(name))
                            {
                                return Err(IrError::verify(
                                    "aggregate projection field name disagrees with its index",
                                ));
                            }
                            if field.bitfield.is_some() != bitfield.is_some() {
                                return Err(IrError::verify(
                                    "aggregate projection does not preserve its bitfield descriptor",
                                ));
                            }
                            if let Some(bitfield) = bitfield {
                                verify_bitfield(*bitfield)?;
                                if bitfield.field_index != *index {
                                    return Err(IrError::verify(
                                        "aggregate projection bitfield descriptor has the wrong field index",
                                    ));
                                }
                                if position + 1 != projections.len() {
                                    return Err(IrError::verify(
                                        "bitfield must be the final aggregate projection",
                                    ));
                                }
                                bitfield_anchor = Some(*bitfield);
                            }
                            QualifiedType::new(
                                field.ty.ty,
                                field.ty.qualifiers | projected.qualifiers,
                            )
                        }
                        AggregateProjection::Index { index } => {
                            let Some(TypeKind::Array(array)) = types.try_kind(projected.ty) else {
                                return Err(IrError::verify(
                                    "aggregate index projection does not follow an array",
                                ));
                            };
                            if !types.is_integer(self.value_type(*index)?.ty) {
                                return Err(IrError::verify(
                                    "aggregate projection index is not an integer",
                                ));
                            }
                            QualifiedType::new(
                                array.element.ty,
                                array.element.qualifiers | projected.qualifiers,
                            )
                        }
                    };
                }
                require_pointer_result(types, result, projected, instruction)?;
                if let Some(descriptor) = bitfield_anchor {
                    let result = instruction
                        .result
                        .expect("aggregate projection result was required above");
                    self.verify_bitfield_projection_consumer(result, descriptor)?;
                }
            }
            FullInstructionKind::Convert {
                kind,
                operand,
                from,
                to,
            } => {
                if *kind == ScalarConversion::ArrayToPointer {
                    let Some(TypeKind::Array(_)) = types.try_kind(from.ty) else {
                        return Err(IrError::verify(
                            "array-to-pointer conversion source is not an array",
                        ));
                    };
                    require_address(types, self.value_type(*operand)?, *from, "array decay")?;
                } else if *kind != ScalarConversion::FunctionToPointer
                    && self.value_type(*operand)?.ty != from.ty
                {
                    return Err(IrError::verify(
                        "conversion operand type disagrees with its source type",
                    ));
                }
                if *kind == ScalarConversion::ToVoid {
                    require_no_result(result, instruction, "conversion to void")?;
                    if !is_void(types, to.ty) {
                        return Err(IrError::verify(
                            "conversion to void does not carry void result type",
                        ));
                    }
                } else {
                    let result = require_result(result, instruction, "conversion")?;
                    if result.ty != to.ty {
                        return Err(IrError::verify(
                            "conversion result type disagrees with its destination type",
                        ));
                    }
                }
            }
            FullInstructionKind::Unary { operator, operand } => {
                let result = require_result(result, instruction, "unary operation")?;
                let operand = self.value_type(*operand)?.ty;
                use super::UnaryOperation as U;
                match operator {
                    U::Plus | U::Negate => {
                        if !types.is_arithmetic(operand) || result.ty != operand {
                            return Err(IrError::verify(
                                "arithmetic unary operation has inconsistent types",
                            ));
                        }
                    }
                    U::BitwiseNot => {
                        if !types.is_integer(operand) || result.ty != operand {
                            return Err(IrError::verify(
                                "bitwise unary operation has inconsistent types",
                            ));
                        }
                    }
                    U::LogicalNot => {
                        if !is_scalar(types, operand) || result.ty != TypeId::INT {
                            return Err(IrError::verify(
                                "logical unary operation has inconsistent types",
                            ));
                        }
                    }
                }
            }
            FullInstructionKind::Binary {
                operator,
                left,
                right,
            } => {
                let result = require_result(result, instruction, "binary operation")?;
                let left = self.value_type(*left)?;
                let right = self.value_type(*right)?;
                use super::BinaryOperation as B;
                match operator {
                    B::LeftShift | B::RightShift => {
                        if !types.is_integer(left.ty)
                            || !types.is_integer(right.ty)
                            || result.ty != left.ty
                        {
                            return Err(IrError::verify(
                                "shift operation has inconsistent integer types",
                            ));
                        }
                    }
                    B::Less
                    | B::LessEqual
                    | B::Greater
                    | B::GreaterEqual
                    | B::Equal
                    | B::NotEqual => {
                        if left.ty != right.ty
                            || !is_scalar(types, left.ty)
                            || !matches!(result.ty, TypeId::INT | TypeId::BOOL)
                        {
                            return Err(IrError::verify(
                                "comparison operation has inconsistent scalar types",
                            ));
                        }
                    }
                    B::Remainder | B::BitwiseAnd | B::BitwiseXor | B::BitwiseOr => {
                        if left.ty != right.ty || !types.is_integer(left.ty) || result.ty != left.ty
                        {
                            return Err(IrError::verify(
                                "integer binary operation has inconsistent types",
                            ));
                        }
                    }
                    B::Multiply | B::Divide | B::Add | B::Subtract => {
                        if left.ty != right.ty
                            || !types.is_arithmetic(left.ty)
                            || result.ty != left.ty
                        {
                            return Err(IrError::verify(
                                "arithmetic binary operation has inconsistent types",
                            ));
                        }
                    }
                }
            }
            FullInstructionKind::IntegerIntrinsic { operation, operand } => {
                let (input, output) = integer_intrinsic_signature(*operation);
                if self.value_type(*operand)?.ty != input {
                    return Err(IrError::verify(
                        "integer intrinsic operand has the wrong exact type",
                    ));
                }
                let result = require_result(result, instruction, "integer intrinsic")?;
                if result.ty != output {
                    return Err(IrError::verify(
                        "integer intrinsic result has the wrong exact type",
                    ));
                }
            }
            FullInstructionKind::MemoryCopy {
                destination,
                source,
                length,
                ..
            } => {
                require_no_result(result, instruction, "memory copy")?;
                let destination = pointer_pointee(types, self.value_type(*destination)?.ty)
                    .ok_or_else(|| IrError::verify("memory copy destination is not a pointer"))?;
                let source = pointer_pointee(types, self.value_type(*source)?.ty)
                    .ok_or_else(|| IrError::verify("memory copy source is not a pointer"))?;
                if destination.ty != TypeId::VOID || source.ty != TypeId::VOID {
                    return Err(IrError::verify(
                        "memory copy operands are not canonical void pointers",
                    ));
                }
                if !types.is_integer(self.value_type(*length)?.ty) {
                    return Err(IrError::verify("memory copy length is not an integer"));
                }
            }
            FullInstructionKind::MemorySet {
                destination,
                value,
                length,
            } => {
                require_no_result(result, instruction, "memory set")?;
                let destination = pointer_pointee(types, self.value_type(*destination)?.ty)
                    .ok_or_else(|| IrError::verify("memory set destination is not a pointer"))?;
                if destination.ty != TypeId::VOID {
                    return Err(IrError::verify(
                        "memory set destination is not a canonical void pointer",
                    ));
                }
                if self.value_type(*value)?.ty != TypeId::INT {
                    return Err(IrError::verify("memory set value does not have type int"));
                }
                if !types.is_integer(self.value_type(*length)?.ty) {
                    return Err(IrError::verify("memory set length is not an integer"));
                }
            }
            FullInstructionKind::DirectCall {
                function,
                signature,
                arguments,
                variadic_boundary,
                effects,
            } => {
                let declaration = self
                    .module
                    .functions
                    .get(function.0 as usize)
                    .filter(|declaration| declaration.id == *function)
                    .ok_or_else(|| IrError::verify("direct call has invalid function id"))?;
                if declaration.signature != *signature {
                    return Err(IrError::verify("direct call carries the wrong signature"));
                }
                self.verify_call(
                    *signature,
                    arguments,
                    *variadic_boundary,
                    result,
                    instruction,
                )?;
                self.verify_returns_twice(effects)?;
                self.verify_noreturn(block, position, effects.no_return)?;
            }
            FullInstructionKind::IndirectCall {
                callee,
                signature,
                arguments,
                variadic_boundary,
                effects,
            } => {
                require_address(
                    types,
                    self.value_type(*callee)?,
                    QualifiedType::unqualified(*signature),
                    "indirect call",
                )?;
                self.verify_call(
                    *signature,
                    arguments,
                    *variadic_boundary,
                    result,
                    instruction,
                )?;
                self.verify_returns_twice(effects)?;
                self.verify_noreturn(block, position, effects.no_return)?;
            }
            FullInstructionKind::AtomicReadModifyWrite {
                address,
                operand,
                object,
                return_new,
                order: _,
                operation,
            } => {
                verify_atomic_object(types, *object)?;
                require_address(types, self.value_type(*address)?, *object, "atomic RMW")?;
                if self.value_type(*operand)?.ty != object.ty {
                    return Err(IrError::verify(
                        "atomic RMW operand type disagrees with its object",
                    ));
                }
                let result = require_result(result, instruction, "atomic RMW")?;
                if result.ty != object.ty {
                    return Err(IrError::verify(
                        "atomic RMW result type disagrees with its object",
                    ));
                }
                if *return_new && *operation == super::AtomicReadModifyWriteOperation::Exchange {
                    return Err(IrError::verify(
                        "atomic exchange cannot return a derived replacement value",
                    ));
                }
            }
            FullInstructionKind::AtomicCompareExchange {
                address,
                expected,
                replacement,
                object,
                order: _,
            } => {
                verify_atomic_object(types, *object)?;
                require_address(
                    types,
                    self.value_type(*address)?,
                    *object,
                    "atomic compare-exchange",
                )?;
                if self.value_type(*expected)?.ty != object.ty
                    || self.value_type(*replacement)?.ty != object.ty
                {
                    return Err(IrError::verify(
                        "atomic compare-exchange values disagree with its object",
                    ));
                }
                let result = require_result(result, instruction, "atomic compare-exchange")?;
                if result.ty != object.ty {
                    return Err(IrError::verify(
                        "atomic compare-exchange result type disagrees with its object",
                    ));
                }
            }
            FullInstructionKind::Prefetch {
                address,
                write: _,
                locality,
            } => {
                require_no_result(result, instruction, "prefetch")?;
                require_address(
                    types,
                    self.value_type(*address)?,
                    QualifiedType::new(TypeId::VOID, TypeQualifiers::CONST),
                    "prefetch",
                )?;
                if *locality > 3 {
                    return Err(IrError::verify(
                        "prefetch locality is outside the supported range",
                    ));
                }
            }
            FullInstructionKind::MemoryFence { order: _ } => {
                require_no_result(result, instruction, "memory fence")?;
            }
            FullInstructionKind::CompilerBarrier { memory: _ } => {
                require_no_result(result, instruction, "compiler barrier")?;
            }
            FullInstructionKind::OpaqueScalar { operand } => {
                let result = require_result(result, instruction, "opaque scalar")?;
                let operand = self.value_type(*operand)?;
                if result.ty != operand.ty || !is_pointer_or_integer(types, result.ty) {
                    return Err(IrError::verify(
                        "opaque scalar operand and result must have the same integer or pointer type",
                    ));
                }
            }
            FullInstructionKind::CodeLayoutHint(hint) => {
                require_no_result(result, instruction, "code layout hint")?;
                if matches!(hint, super::CodeLayoutHint::AlignToPowerOfTwo(power) if !matches!(power, 3..=6))
                {
                    return Err(IrError::verify(
                        "code alignment hint is outside the certified range",
                    ));
                }
            }
            FullInstructionKind::X86Cpuid {
                leaf,
                subleaf,
                eax,
                ebx,
                ecx,
                edx,
            } => {
                require_no_result(result, instruction, "x86 CPUID")?;
                if self.value_type(*leaf)?.ty != TypeId::UNSIGNED_INT {
                    return Err(IrError::verify("CPUID leaf is not unsigned int"));
                }
                if let Some(subleaf) = subleaf
                    && self.value_type(*subleaf)?.ty != TypeId::UNSIGNED_INT
                {
                    return Err(IrError::verify("CPUID subleaf is not unsigned int"));
                }
                if [eax, ebx, ecx, edx].iter().all(|output| output.is_none()) {
                    return Err(IrError::verify("CPUID has no retained output"));
                }
                for output in [eax, ebx, ecx, edx].into_iter().flatten() {
                    require_address(
                        types,
                        self.value_type(*output)?,
                        QualifiedType::unqualified(TypeId::UNSIGNED_INT),
                        "CPUID output",
                    )?;
                }
            }
            FullInstructionKind::X86Rdtsc { low, high } => {
                require_no_result(result, instruction, "x86 RDTSC")?;
                for output in [low, high] {
                    require_address(
                        types,
                        self.value_type(*output)?,
                        QualifiedType::unqualified(TypeId::UNSIGNED_INT),
                        "RDTSC output",
                    )?;
                }
            }
            FullInstructionKind::VaStart {
                list,
                last_named_parameter,
            } => {
                require_no_result(result, instruction, "va_start")?;
                self.verify_va_list_address(*list)?;
                let signature = types
                    .function_signature(self.function.signature)
                    .ok_or_else(|| IrError::verify("function signature is not a function type"))?;
                if !signature.variadic {
                    return Err(IrError::verify("va_start occurs in a nonvariadic function"));
                }
                if self
                    .function
                    .parameters
                    .last()
                    .is_none_or(|parameter| parameter.local != *last_named_parameter)
                {
                    return Err(IrError::verify(
                        "va_start does not name the final fixed parameter",
                    ));
                }
            }
            FullInstructionKind::VaArg { list, requested } => {
                self.verify_va_list_address(*list)?;
                if is_void(types, requested.ty)
                    || matches!(
                        types.try_kind(requested.ty),
                        Some(TypeKind::Array(_) | TypeKind::Function(_))
                    )
                {
                    return Err(IrError::verify("va_arg requested a non-object type"));
                }
                if is_variably_modified(types, requested.ty) {
                    return Err(IrError::verify("va_arg requested a variably modified type"));
                }
                if requested.qualifiers.contains(TypeQualifiers::ATOMIC)
                    || changed_by_default_argument_promotions(types, requested.ty)
                {
                    return Err(IrError::verify(
                        "va_arg requested a type changed by the default argument promotions",
                    ));
                }
                if matches!(
                    types.try_kind(requested.ty),
                    Some(TypeKind::Record(record))
                        if types.record(*record).is_none_or(|record| !record.is_complete())
                ) {
                    return Err(IrError::verify("va_arg requested an incomplete type"));
                }
                let result = require_result(result, instruction, "va_arg")?;
                if result.ty != requested.ty {
                    return Err(IrError::verify("va_arg result has the wrong type"));
                }
            }
            FullInstructionKind::VaCopy {
                destination,
                source,
            } => {
                require_no_result(result, instruction, "va_copy")?;
                self.verify_va_list_address(*destination)?;
                self.verify_va_list_address(*source)?;
            }
            FullInstructionKind::VaEnd { list } => {
                require_no_result(result, instruction, "va_end")?;
                self.verify_va_list_address(*list)?;
            }
        }
        Ok(())
    }

    fn is_owned_aggregate_value(&self, value: ValueId) -> Result<bool, IrError> {
        let definition = self
            .definitions
            .get(value.0 as usize)
            .and_then(|definition| *definition)
            .ok_or_else(|| IrError::verify("aggregate value has no definition"))?;
        Ok(match definition {
            Definition::BlockParameter(_) => true,
            Definition::Instruction { block, position } => matches!(
                self.function.blocks[block.0 as usize].instructions[position].kind,
                FullInstructionKind::AggregateSnapshot { .. }
                    | FullInstructionKind::DirectCall { .. }
                    | FullInstructionKind::IndirectCall { .. }
                    | FullInstructionKind::VaArg { .. }
            ),
        })
    }

    fn verify_va_list_address(&self, value: ValueId) -> Result<(), IrError> {
        let types = &self.module.types;
        let va_list = types
            .target_builtin_id(TargetBuiltinType::VaList)
            .ok_or_else(|| IrError::verify("variadic IR has no target va_list type"))?;
        let pointee = pointer_pointee(types, self.value_type(value)?.ty)
            .ok_or_else(|| IrError::verify("va_list operand is not an address"))?;
        let parameter_element = match types.try_kind(va_list) {
            Some(TypeKind::Array(array)) => Some(array.element.ty),
            _ => None,
        };
        if pointee.ty != va_list && parameter_element != Some(pointee.ty) {
            return Err(IrError::verify(
                "va_list operand points to an unrelated object",
            ));
        }
        Ok(())
    }

    fn verify_call(
        &self,
        signature: TypeId,
        arguments: &[ValueId],
        variadic_boundary: usize,
        result: Option<QualifiedType>,
        instruction: &FullInstruction,
    ) -> Result<(), IrError> {
        let signature = self
            .module
            .types
            .function_signature(signature)
            .ok_or_else(|| IrError::verify("call signature is not a function type"))?;
        let fixed = match &signature.parameters {
            FunctionParameters::Unspecified => 0,
            FunctionParameters::Prototype(parameters) => {
                if arguments.len() < parameters.len()
                    || (!signature.variadic && arguments.len() != parameters.len())
                {
                    return Err(IrError::verify(
                        "call argument count disagrees with its signature",
                    ));
                }
                for (argument, expected) in arguments.iter().zip(parameters) {
                    if self.value_type(*argument)?.ty != expected.ty {
                        return Err(IrError::verify(
                            "call argument type disagrees with its signature",
                        ));
                    }
                }
                parameters.len()
            }
        };
        for argument in arguments {
            if is_aggregate(&self.module.types, self.value_type(*argument)?.ty)
                && !self.is_owned_aggregate_value(*argument)?
            {
                return Err(IrError::verify(
                    "call aggregate argument is not owned aggregate storage",
                ));
            }
        }
        if variadic_boundary != fixed {
            return Err(IrError::verify(
                "call variadic boundary disagrees with its signature",
            ));
        }
        if is_void(&self.module.types, signature.result.ty) {
            require_no_result(result, instruction, "void call")?;
        } else {
            let result = require_result(result, instruction, "call")?;
            if result.ty != signature.result.ty {
                return Err(IrError::verify("call result has the wrong type"));
            }
        }
        Ok(())
    }

    fn verify_returns_twice(&self, effects: &CallEffects) -> Result<(), IrError> {
        if effects.returns_twice {
            if effects.no_return {
                return Err(IrError::verify(
                    "a call cannot be both noreturn and returns-twice",
                ));
            }
            if !effects.reads_memory || !effects.writes_memory {
                return Err(IrError::verify(
                    "a returns-twice call must conservatively read and write memory",
                ));
            }
        }
        Ok(())
    }

    fn verify_noreturn(
        &self,
        block: BlockId,
        position: usize,
        no_return: bool,
    ) -> Result<(), IrError> {
        if no_return {
            let body = &self.function.blocks[block.0 as usize];
            if position + 1 != body.instructions.len()
                || !matches!(body.terminator, Some(FullTerminator::Unreachable))
            {
                return Err(IrError::verify(
                    "noreturn call is not immediately followed by unreachable",
                ));
            }
        }
        Ok(())
    }

    fn terminator(&self, block: BlockId, terminator: &FullTerminator) -> Result<(), IrError> {
        for operand in terminator_operands(terminator) {
            self.use_value(block, usize::MAX, operand)?;
        }
        match terminator {
            FullTerminator::Branch(edge) => self.edge(block, edge)?,
            FullTerminator::Conditional {
                condition,
                then_edge,
                else_edge,
            } => {
                if !is_scalar(&self.module.types, self.value_type(*condition)?.ty) {
                    return Err(IrError::verify(
                        "conditional branch condition is not scalar",
                    ));
                }
                self.edge(block, then_edge)?;
                self.edge(block, else_edge)?;
            }
            FullTerminator::Switch {
                selector,
                cases,
                default,
            } => {
                if !self.module.types.is_integer(self.value_type(*selector)?.ty) {
                    return Err(IrError::verify("switch selector is not an integer"));
                }
                let mut values = HashSet::new();
                for case in cases {
                    if !values.insert(case.value) {
                        return Err(IrError::verify("switch contains duplicate case values"));
                    }
                    self.edge(block, &case.edge)?;
                }
                self.edge(block, default)?;
            }
            FullTerminator::IndirectBranch { selector, targets } => {
                if pointer_pointee(&self.module.types, self.value_type(*selector)?.ty).is_none() {
                    return Err(IrError::verify(
                        "computed goto selector does not have pointer type",
                    ));
                }
                if targets.is_empty() {
                    return Err(IrError::verify("computed goto has no target blocks"));
                }
                let mut blocks = HashSet::new();
                for target in targets {
                    if !blocks.insert(target.target) {
                        return Err(IrError::verify(
                            "computed goto contains duplicate target blocks",
                        ));
                    }
                    if !target.arguments.is_empty() {
                        return Err(IrError::verify(
                            "computed goto target edge must not carry arguments",
                        ));
                    }
                    if self
                        .function
                        .blocks
                        .get(target.target.0 as usize)
                        .filter(|destination| destination.id == target.target)
                        .is_some_and(|destination| !destination.parameters.is_empty())
                    {
                        return Err(IrError::verify(
                            "computed goto target block must not have parameters",
                        ));
                    }
                    self.edge(block, target)?;
                }
            }
            FullTerminator::Return(value) => match value {
                Some(value) => {
                    if self.value_type(*value)?.ty != self.function.result_type.ty {
                        return Err(IrError::verify("return value has the wrong type"));
                    }
                    if is_aggregate(&self.module.types, self.function.result_type.ty)
                        && !self.is_owned_aggregate_value(*value)?
                    {
                        return Err(IrError::verify(
                            "return value is not owned aggregate storage",
                        ));
                    }
                    if is_void(&self.module.types, self.function.result_type.ty) {
                        return Err(IrError::verify("void function returns a value"));
                    }
                }
                None if !is_void(&self.module.types, self.function.result_type.ty) => {
                    return Err(IrError::verify("non-void function returns without a value"));
                }
                None => {}
            },
            FullTerminator::Unreachable => {}
        }
        Ok(())
    }

    fn edge(&self, source: BlockId, edge: &FullEdge) -> Result<(), IrError> {
        let target = self
            .function
            .blocks
            .get(edge.target.0 as usize)
            .filter(|block| block.id == edge.target)
            .ok_or_else(|| {
                IrError::verify(format!("edge references unknown block {}", edge.target.0))
            })?;
        if edge.arguments.len() != target.parameters.len() {
            return Err(IrError::verify(format!(
                "edge from block {} to block {} has the wrong arity",
                source.0, edge.target.0
            )));
        }
        for (argument, parameter) in edge.arguments.iter().zip(&target.parameters) {
            if !same_type(self.value_type(*argument)?, self.value_type(*parameter)?) {
                return Err(IrError::verify(format!(
                    "edge from block {} to block {} has an argument type mismatch",
                    source.0, edge.target.0
                )));
            }
            if is_aggregate(&self.module.types, self.value_type(*argument)?.ty)
                && !self.is_owned_aggregate_value(*argument)?
            {
                return Err(IrError::verify(
                    "edge aggregate argument is not owned aggregate storage",
                ));
            }
        }
        Ok(())
    }

    fn use_value(&self, block: BlockId, position: usize, value: ValueId) -> Result<(), IrError> {
        let definition = self
            .definitions
            .get(value.0 as usize)
            .and_then(|definition| *definition)
            .ok_or_else(|| IrError::verify(format!("use of undefined value {}", value.0)))?;
        match definition {
            Definition::BlockParameter(definition_block) if definition_block == block => Ok(()),
            Definition::Instruction {
                block: definition_block,
                position: definition_position,
            } if definition_block == block && definition_position < position => Ok(()),
            Definition::BlockParameter(definition_block)
            | Definition::Instruction {
                block: definition_block,
                ..
            } if definition_block != block
                && self.dominators[block.0 as usize].contains(&definition_block) =>
            {
                Ok(())
            }
            _ => Err(IrError::verify(format!(
                "value {} does not dominate its use in block {}",
                value.0, block.0
            ))),
        }
    }

    fn value_type(&self, value: ValueId) -> Result<QualifiedType, IrError> {
        value_type(self.function, value)
    }

    fn verify_target(&self, target: RelocationTarget) -> Result<(), IrError> {
        match target {
            RelocationTarget::Object(id)
                if self
                    .module
                    .globals
                    .get(id.0 as usize)
                    .is_some_and(|object| object.id == id) =>
            {
                Ok(())
            }
            RelocationTarget::Function(id)
                if self
                    .module
                    .functions
                    .get(id.0 as usize)
                    .is_some_and(|function| function.id == id) =>
            {
                Ok(())
            }
            RelocationTarget::String(id)
                if self
                    .module
                    .strings
                    .get(id.0 as usize)
                    .is_some_and(|string| string.id == id) =>
            {
                Ok(())
            }
            _ => Err(IrError::verify("address constant has an invalid target")),
        }
    }

    fn verify_bitfield_projection_consumer(
        &self,
        value: ValueId,
        expected: super::BitfieldDescriptor,
    ) -> Result<(), IrError> {
        let mut matching_loads = 0_u32;
        for block in &self.function.blocks {
            for instruction in &block.instructions {
                let uses = instruction_operands(&instruction.kind)
                    .into_iter()
                    .filter(|operand| *operand == value)
                    .count();
                if uses == 0 {
                    continue;
                }
                match &instruction.kind {
                    FullInstructionKind::BitfieldLoad {
                        address,
                        descriptor,
                        ..
                    } if *address == value && *descriptor == expected && uses == 1 => {
                        matching_loads += 1;
                    }
                    FullInstructionKind::BitfieldLoad { .. } => {
                        return Err(IrError::verify(
                            "aggregate bitfield projection and load descriptors disagree",
                        ));
                    }
                    _ => {
                        return Err(IrError::verify(
                            "aggregate bitfield projection is not consumed by a bitfield load",
                        ));
                    }
                }
            }
            if block
                .terminator
                .as_ref()
                .is_some_and(|terminator| terminator_operands(terminator).contains(&value))
            {
                return Err(IrError::verify(
                    "aggregate bitfield projection escapes through a terminator",
                ));
            }
        }
        if matching_loads != 1 {
            return Err(IrError::verify(
                "aggregate bitfield projection must have exactly one bitfield load consumer",
            ));
        }
        Ok(())
    }
}

pub(super) fn instruction_operands(kind: &FullInstructionKind) -> Vec<ValueId> {
    match kind {
        FullInstructionKind::Constant(_)
        | FullInstructionKind::AddressConstant { .. }
        | FullInstructionKind::AddressOfGlobal { .. }
        | FullInstructionKind::AddressOfFunction { .. }
        | FullInstructionKind::AddressOfString { .. }
        | FullInstructionKind::AddressOfStorage { .. }
        | FullInstructionKind::MemoryFence { .. }
        | FullInstructionKind::CompilerBarrier { .. }
        | FullInstructionKind::CodeLayoutHint(_) => Vec::new(),
        FullInstructionKind::RuntimeSize { extents, .. } => extents.clone(),
        FullInstructionKind::RuntimeSizedAllocate { size, .. } => vec![*size],
        FullInstructionKind::ProjectField { base, .. } => vec![*base],
        FullInstructionKind::PointerOffset { base, index, .. } => vec![*base, *index],
        FullInstructionKind::RuntimePointerOffset {
            base,
            index,
            stride,
            ..
        } => vec![*base, *index, *stride],
        FullInstructionKind::PointerDifference { left, right, .. }
        | FullInstructionKind::Binary { left, right, .. } => vec![*left, *right],
        FullInstructionKind::RuntimePointerDifference {
            left,
            right,
            stride,
            ..
        } => vec![*left, *right, *stride],
        FullInstructionKind::Load { address, .. }
        | FullInstructionKind::BitfieldLoad { address, .. }
        | FullInstructionKind::ZeroInitialize {
            destination: address,
            ..
        }
        | FullInstructionKind::StringInitialize {
            destination: address,
            ..
        }
        | FullInstructionKind::AggregateSnapshot {
            source: address, ..
        } => vec![*address],
        FullInstructionKind::AggregateProject {
            base, projections, ..
        } => std::iter::once(*base)
            .chain(
                projections
                    .iter()
                    .filter_map(|projection| match projection {
                        AggregateProjection::Field { .. } => None,
                        AggregateProjection::Index { index } => Some(*index),
                    }),
            )
            .collect(),
        FullInstructionKind::Store { address, value, .. }
        | FullInstructionKind::BitfieldStore { address, value, .. } => vec![*address, *value],
        FullInstructionKind::AtomicReadModifyWrite {
            address, operand, ..
        } => vec![*address, *operand],
        FullInstructionKind::AtomicCompareExchange {
            address,
            expected,
            replacement,
            ..
        } => vec![*address, *expected, *replacement],
        FullInstructionKind::AggregateCopy {
            destination,
            source,
            ..
        } => vec![*destination, *source],
        FullInstructionKind::MemoryCopy {
            destination,
            source,
            length,
            ..
        } => vec![*destination, *source, *length],
        FullInstructionKind::MemorySet {
            destination,
            value,
            length,
        } => vec![*destination, *value, *length],
        FullInstructionKind::Convert { operand, .. }
        | FullInstructionKind::Unary { operand, .. }
        | FullInstructionKind::IntegerIntrinsic { operand, .. }
        | FullInstructionKind::OpaqueScalar { operand } => vec![*operand],
        FullInstructionKind::X86Cpuid {
            leaf,
            subleaf,
            eax,
            ebx,
            ecx,
            edx,
        } => std::iter::once(*leaf)
            .chain(subleaf.iter().copied())
            .chain([eax, ebx, ecx, edx].into_iter().filter_map(|value| *value))
            .collect(),
        FullInstructionKind::X86Rdtsc { low, high } => vec![*low, *high],
        FullInstructionKind::Prefetch { address, .. } => vec![*address],
        FullInstructionKind::DirectCall { arguments, .. } => arguments.clone(),
        FullInstructionKind::IndirectCall {
            callee, arguments, ..
        } => std::iter::once(*callee)
            .chain(arguments.iter().copied())
            .collect(),
        FullInstructionKind::VaStart { list, .. }
        | FullInstructionKind::VaArg { list, .. }
        | FullInstructionKind::VaEnd { list } => vec![*list],
        FullInstructionKind::VaCopy {
            destination,
            source,
        } => vec![*destination, *source],
    }
}

pub(super) fn terminator_operands(terminator: &FullTerminator) -> Vec<ValueId> {
    match terminator {
        FullTerminator::Branch(edge) => edge.arguments.clone(),
        FullTerminator::Conditional {
            condition,
            then_edge,
            else_edge,
        } => std::iter::once(*condition)
            .chain(then_edge.arguments.iter().copied())
            .chain(else_edge.arguments.iter().copied())
            .collect(),
        FullTerminator::Switch {
            selector,
            cases,
            default,
        } => std::iter::once(*selector)
            .chain(
                cases
                    .iter()
                    .flat_map(|case| case.edge.arguments.iter().copied()),
            )
            .chain(default.arguments.iter().copied())
            .collect(),
        FullTerminator::IndirectBranch { selector, targets } => std::iter::once(*selector)
            .chain(
                targets
                    .iter()
                    .flat_map(|target| target.arguments.iter().copied()),
            )
            .collect(),
        FullTerminator::Return(value) => value.iter().copied().collect(),
        FullTerminator::Unreachable => Vec::new(),
    }
}

fn terminator_edges(terminator: &FullTerminator) -> impl Iterator<Item = &FullEdge> {
    let mut edges = Vec::new();
    match terminator {
        FullTerminator::Branch(edge) => edges.push(edge),
        FullTerminator::Conditional {
            then_edge,
            else_edge,
            ..
        } => {
            edges.push(then_edge);
            edges.push(else_edge);
        }
        FullTerminator::Switch { cases, default, .. } => {
            edges.extend(cases.iter().map(|case| &case.edge));
            edges.push(default);
        }
        FullTerminator::IndirectBranch { targets, .. } => edges.extend(targets),
        FullTerminator::Return(_) | FullTerminator::Unreachable => {}
    }
    edges.into_iter()
}

fn require_result(
    result: Option<QualifiedType>,
    instruction: &FullInstruction,
    name: &str,
) -> Result<QualifiedType, IrError> {
    result.ok_or_else(|| {
        IrError::verify(format!(
            "{name} instruction {} has no result",
            instruction.id.0
        ))
    })
}

fn require_no_result(
    result: Option<QualifiedType>,
    instruction: &FullInstruction,
    name: &str,
) -> Result<(), IrError> {
    if result.is_some() {
        Err(IrError::verify(format!(
            "{name} instruction {} unexpectedly has a result",
            instruction.id.0
        )))
    } else {
        Ok(())
    }
}

fn require_pointer_result(
    types: &TypeStore,
    result: Option<QualifiedType>,
    pointee: QualifiedType,
    instruction: &FullInstruction,
) -> Result<(), IrError> {
    let result = require_result(result, instruction, "address")?;
    require_address(types, result, pointee, "address result")
}

fn require_address(
    types: &TypeStore,
    address: QualifiedType,
    object: QualifiedType,
    context: &str,
) -> Result<(), IrError> {
    let pointee = pointer_pointee(types, address.ty)
        .ok_or_else(|| IrError::verify(format!("{context} operand is not a pointer")))?;
    if !same_type(pointee, object) {
        return Err(IrError::verify(format!(
            "{context} pointer has the wrong pointee type"
        )));
    }
    Ok(())
}

fn require_address_ignoring_pointee_qualifiers(
    types: &TypeStore,
    address: QualifiedType,
    object: QualifiedType,
    context: &str,
) -> Result<(), IrError> {
    let pointee = pointer_pointee(types, address.ty)
        .ok_or_else(|| IrError::verify(format!("{context} operand is not a pointer")))?;
    if pointee.ty != object.ty {
        return Err(IrError::verify(format!(
            "{context} pointer has the wrong pointee type"
        )));
    }
    Ok(())
}

fn verify_access(access: MemoryAccess) -> Result<(), IrError> {
    if (access.volatile || access.atomic.is_some()) && (!access.non_elidable || !access.non_movable)
    {
        return Err(IrError::verify(
            "ordered memory access is missing non-elidable/non-movable markers",
        ));
    }
    Ok(())
}

fn verify_atomic_object(types: &TypeStore, object: QualifiedType) -> Result<(), IrError> {
    if object.qualifiers.contains(TypeQualifiers::CONST) {
        return Err(IrError::verify("atomic operation modifies a const object"));
    }
    let integer = types.is_integer(object.ty);
    if !integer && pointer_pointee(types, object.ty).is_none() {
        return Err(IrError::verify(
            "atomic operation object is not an integer or pointer",
        ));
    }
    Ok(())
}

fn integer_intrinsic_signature(operation: super::IntegerIntrinsicOperation) -> (TypeId, TypeId) {
    use super::IntegerIntrinsicOperation as O;
    match operation {
        O::ByteSwap64 => (TypeId::UNSIGNED_LONG, TypeId::UNSIGNED_LONG),
        O::CountLeadingZerosInt | O::PopulationCountInt | O::CountTrailingZerosInt => {
            (TypeId::UNSIGNED_INT, TypeId::INT)
        }
        O::CountLeadingZerosLong => (TypeId::UNSIGNED_LONG, TypeId::INT),
        O::CountLeadingZerosLongLong
        | O::CountTrailingZerosLongLong
        | O::PopulationCountLongLong => (TypeId::UNSIGNED_LONG_LONG, TypeId::INT),
    }
}

fn verify_bitfield(bitfield: super::BitfieldDescriptor) -> Result<(), IrError> {
    let bits = bitfield
        .storage_size
        .checked_mul(8)
        .ok_or_else(|| IrError::verify("bitfield storage size overflows its bit representation"))?;
    if bitfield.width == 0
        || bitfield.storage_size == 0
        || bitfield.storage_align == 0
        || !bitfield.storage_align.is_power_of_two()
        || u64::from(bitfield.bit_offset) + u64::from(bitfield.width) > bits
    {
        return Err(IrError::verify("bitfield descriptor is out of bounds"));
    }
    Ok(())
}

fn verify_type(types: &TypeStore, ty: QualifiedType, context: &str) -> Result<(), IrError> {
    if types.contains(ty.ty) {
        Ok(())
    } else {
        Err(IrError::verify(format!(
            "{context} references an unknown type"
        )))
    }
}

fn value_type(function: &FullFunction, value: ValueId) -> Result<QualifiedType, IrError> {
    function
        .value_types
        .get(value.0 as usize)
        .copied()
        .map(QualifiedType::unqualified)
        .ok_or_else(|| IrError::verify(format!("value {} has no type", value.0)))
}

fn same_type(left: QualifiedType, right: QualifiedType) -> bool {
    left.ty == right.ty && left.qualifiers == right.qualifiers
}

fn pointer_pointee(types: &TypeStore, ty: TypeId) -> Option<QualifiedType> {
    match types.try_kind(ty) {
        Some(TypeKind::Pointer(pointer)) => Some(pointer.pointee),
        _ => None,
    }
}

fn is_void(types: &TypeStore, ty: TypeId) -> bool {
    types.builtin_type(ty) == Some(BuiltinType::Void)
}

fn is_variably_modified(types: &TypeStore, ty: TypeId) -> bool {
    match types.try_kind(ty) {
        Some(TypeKind::Array(array)) => {
            matches!(
                array.length,
                ArrayLength::Variable(_) | ArrayLength::UnspecifiedVariable(_)
            ) || is_variably_modified(types, array.element.ty)
        }
        Some(TypeKind::Pointer(pointer)) => is_variably_modified(types, pointer.pointee.ty),
        _ => false,
    }
}

fn changed_by_default_argument_promotions(types: &TypeStore, ty: TypeId) -> bool {
    if matches!(
        types.builtin_type(ty),
        Some(
            BuiltinType::Bool
                | BuiltinType::Char
                | BuiltinType::SignedChar
                | BuiltinType::UnsignedChar
                | BuiltinType::Short
                | BuiltinType::UnsignedShort
                | BuiltinType::Float
        )
    ) {
        return true;
    }
    let Some(TypeKind::Enum(id)) = types.try_kind(ty) else {
        return false;
    };
    let Some(underlying) = types
        .enumeration(*id)
        .and_then(|enumeration| enumeration.body.as_ref())
        .map(|body| body.underlying)
    else {
        return true;
    };
    matches!(
        types.builtin_type(underlying),
        Some(
            BuiltinType::Bool
                | BuiltinType::Char
                | BuiltinType::SignedChar
                | BuiltinType::UnsignedChar
                | BuiltinType::Short
                | BuiltinType::UnsignedShort
        )
    )
}

fn is_aggregate(types: &TypeStore, ty: TypeId) -> bool {
    matches!(
        types.try_kind(ty),
        Some(TypeKind::Array(_) | TypeKind::Record(_))
    )
}

fn verify_long_double_constant(
    types: &TypeStore,
    ty: TypeId,
    constant: ScalarConstant,
) -> Result<(), IrError> {
    let ScalarConstant::LongDouble(value) = constant else {
        return Ok(());
    };
    if types.builtin_type(ty) != Some(BuiltinType::LongDouble) {
        return Err(IrError::verify(
            "exact long-double constant has a non-long-double result type",
        ));
    }
    match value.format {
        ccc_target::LongDoubleFormat::Binary64 => Err(IrError::verify(
            "binary64 long double must use the native floating constant representation",
        )),
        ccc_target::LongDoubleFormat::X87Extended if value.bytes[10..] != [0; 6] => Err(
            IrError::verify("x87 long-double constant has nonzero ABI padding"),
        ),
        ccc_target::LongDoubleFormat::X87Extended | ccc_target::LongDoubleFormat::IeeeBinary128 => {
            Ok(())
        }
    }
}

fn is_pointer_or_integer(types: &TypeStore, ty: TypeId) -> bool {
    pointer_pointee(types, ty).is_some() || types.is_integer(ty)
}

fn is_scalar(types: &TypeStore, ty: TypeId) -> bool {
    types.is_arithmetic(ty) || pointer_pointee(types, ty).is_some()
}

fn array_bound(types: &TypeStore, ty: TypeId) -> Option<u64> {
    match types.try_kind(ty) {
        Some(TypeKind::Array(array)) => match array.length {
            ArrayLength::Constant(bound) => Some(bound),
            ArrayLength::Incomplete
            | ArrayLength::Variable(_)
            | ArrayLength::UnspecifiedVariable(_) => None,
        },
        _ => None,
    }
}
