use std::fmt::Write;

use ccc_types::TypeStore;

use super::{
    AggregateProjection, BinaryOperation, CallEffects, FullEdge, FullInstructionKind, FullModule,
    FullTerminator, InitializerGraph, InitializerNodeKind, InitializerPath, MemoryAccess,
    RelocationTarget, ScalarConstant, ScalarConversion, UnaryOperation,
};

pub fn dump_frontend_ir(module: &FullModule) -> String {
    let mut output = String::new();
    for global in &module.globals {
        let origin = match global.source {
            super::DataOrigin::FileScope(id) => format!("file:g{}", id.0),
            super::DataOrigin::BlockStatic { function, local } => {
                format!("block-static:f{}:l{}", function.0, local.0)
            }
        };
        let _ = writeln!(
            output,
            "data d{} @{} : {} [{} linkage={:?}{} duration={:?} visibility={:?} definition={:?}{}{}{}]",
            global.id.0,
            global.emission.symbol_name,
            module.types.display_qualified(global.ty),
            origin,
            global.linkage,
            if global.emission.binding == ccc_sema::generic::SymbolBinding::Weak {
                " binding=Weak"
            } else {
                ""
            },
            global.duration,
            global.emission.visibility,
            global.emission.definition,
            global
                .emission
                .section
                .as_ref()
                .map_or_else(String::new, |section| format!(" section={section}")),
            global
                .emission
                .requested_alignment
                .map_or_else(String::new, |alignment| format!(" align={alignment}")),
            global
                .emission
                .tls
                .map_or_else(String::new, |tls| format!(" tls={tls:?}")),
        );
        if let Some(initializer) = &global.initializer {
            dump_initializer(&mut output, &module.types, initializer);
        }
    }
    for string in &module.strings {
        let units = string
            .code_units
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(
            output,
            "string s{} {:?} : {} = [{}]",
            string.id.0,
            string.encoding,
            module.types.display_qualified(string.ty),
            units
        );
    }
    for function in &module.functions {
        if function.entry.is_none() {
            let _ = writeln!(
                output,
                "declare f{} @{} : {} [linkage={:?}{} visibility={:?}]",
                function.id.0,
                function.symbol_name,
                module.types.display(function.signature),
                function.linkage,
                if function.binding == ccc_sema::generic::SymbolBinding::Weak {
                    " binding=Weak"
                } else {
                    ""
                },
                function.visibility,
            );
            continue;
        }
        let parameters = function
            .parameters
            .iter()
            .map(|parameter| {
                let residency = parameter
                    .storage
                    .map_or_else(|| "ssa".to_owned(), |storage| format!("m{}", storage.0));
                format!(
                    "v{} %{}: {} -> {}",
                    parameter.incoming.expect("verified parameter").0,
                    parameter.name,
                    module.types.display_qualified(parameter.ty),
                    residency,
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(
            output,
            "function f{} @{}({}) -> {} [signature={} linkage={:?}{} visibility={:?} inline={} noreturn={}] {{",
            function.id.0,
            function.symbol_name,
            parameters,
            module.types.display_qualified(function.result_type),
            module.types.display(function.signature),
            function.linkage,
            if function.binding == ccc_sema::generic::SymbolBinding::Weak {
                " binding=Weak"
            } else {
                ""
            },
            function.visibility,
            function.properties.inline,
            function.properties.no_return,
        );
        for storage in &function.storage {
            let reasons = storage
                .required_by
                .iter()
                .map(|reason| format!("{reason:?}"))
                .collect::<Vec<_>>()
                .join(",");
            let _ = writeln!(
                output,
                "  storage m{} l{} %{}: {} [{:?}; {}]",
                storage.id.0,
                storage.local.0,
                storage.name,
                module.types.display_qualified(storage.ty),
                storage.location,
                reasons,
            );
        }
        for block in &function.blocks {
            let parameters = block
                .parameters
                .iter()
                .map(|value| {
                    format!(
                        "v{}: {}",
                        value.0,
                        module.types.display(function.value_types[value.0 as usize])
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            let _ = writeln!(output, "  b{}({parameters}):", block.id.0);
            for instruction in &block.instructions {
                let assignment = instruction.result.map_or_else(String::new, |value| {
                    format!(
                        "v{}: {} = ",
                        value.0,
                        module.types.display(function.value_types[value.0 as usize])
                    )
                });
                let _ = writeln!(
                    output,
                    "    i{}: {}{}",
                    instruction.id.0,
                    assignment,
                    display_instruction(module, &instruction.kind)
                );
            }
            let _ = writeln!(
                output,
                "    {}",
                display_terminator(block.terminator.as_ref().expect("verified terminator"))
            );
        }
        output.push_str("}\n");
    }
    output
}

fn dump_initializer(output: &mut String, types: &TypeStore, graph: &InitializerGraph) {
    let _ = writeln!(output, "  initializer root=n{} {{", graph.root.0);
    for node in &graph.nodes {
        let _ = writeln!(
            output,
            "    n{}: {} = {}",
            node.id.0,
            types.display_qualified(node.ty),
            display_initializer_node(&node.kind)
        );
    }
    output.push_str("  }\n");
}

fn display_initializer_node(kind: &InitializerNodeKind) -> String {
    match kind {
        InitializerNodeKind::Zero => "zero".to_owned(),
        InitializerNodeKind::Scalar(constant) => format!("const {}", display_constant(*constant)),
        InitializerNodeKind::Relocation {
            target,
            addend,
            one_past,
            kind,
        } => format!(
            "reloc {:?} {} addend={} one-past={}",
            kind,
            display_target(*target),
            addend,
            one_past
        ),
        InitializerNodeKind::StringData {
            string,
            copy_code_units,
        } => format!("string-data s{} units={copy_code_units}", string.0),
        InitializerNodeKind::Repeat { element, count } => {
            format!("repeat n{} count={count}", element.0)
        }
        InitializerNodeKind::Aggregate(edges) => format!(
            "aggregate [{}]",
            edges
                .iter()
                .map(|edge| format!(
                    "{} -> n{}",
                    display_initializer_path(&edge.path),
                    edge.node.0
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn display_initializer_path(path: &[InitializerPath]) -> String {
    if path.is_empty() {
        return "self".to_owned();
    }
    path.iter()
        .map(|element| match element {
            InitializerPath::Index(index) => format!("[{index}]"),
            InitializerPath::Field {
                index,
                name,
                bitfield,
            } => format!(
                ".{}#{}{}",
                name.as_deref().unwrap_or("<anonymous>"),
                index,
                bitfield.map_or_else(String::new, |descriptor| format!(
                    ":bits({}:{}/{})",
                    descriptor.storage_offset, descriptor.bit_offset, descriptor.width
                ))
            ),
        })
        .collect::<Vec<_>>()
        .join("")
}

fn display_instruction(module: &FullModule, kind: &FullInstructionKind) -> String {
    match kind {
        FullInstructionKind::Constant(constant) => {
            format!("const {}", display_constant(*constant))
        }
        FullInstructionKind::AddressConstant {
            target,
            addend,
            one_past,
        } => format!(
            "address-constant {} addend={} one-past={}",
            display_target(*target),
            addend,
            one_past
        ),
        FullInstructionKind::AddressOfGlobal { global } => format!("address.data d{}", global.0),
        FullInstructionKind::AddressOfFunction {
            function,
            signature,
        } => format!(
            "address.function f{} signature={}",
            function.0,
            module.types.display(*signature)
        ),
        FullInstructionKind::AddressOfString { string } => format!("address.string s{}", string.0),
        FullInstructionKind::AddressOfStorage { storage } => {
            format!("address.storage m{}", storage.0)
        }
        FullInstructionKind::ProjectField {
            base,
            record,
            field_index,
            field_name,
        } => format!(
            "project.field v{} {} .{}#{}",
            base.0,
            module.types.display_qualified(*record),
            field_name.as_deref().unwrap_or("<anonymous>"),
            field_index
        ),
        FullInstructionKind::PointerOffset {
            base,
            index,
            element,
            subtract,
        } => format!(
            "pointer.offset v{}, {}v{} element={}",
            base.0,
            if *subtract { "-" } else { "" },
            index.0,
            module.types.display_qualified(*element)
        ),
        FullInstructionKind::PointerDifference {
            left,
            right,
            element,
        } => format!(
            "pointer.difference v{}, v{} element={}",
            left.0,
            right.0,
            module.types.display_qualified(*element)
        ),
        FullInstructionKind::Load {
            address,
            object,
            access,
        } => format!(
            "load v{} object={} {}",
            address.0,
            module.types.display_qualified(*object),
            display_access(*access)
        ),
        FullInstructionKind::Store {
            address,
            value,
            object,
            access,
        } => format!(
            "store v{} -> v{} object={} {}",
            value.0,
            address.0,
            module.types.display_qualified(*object),
            display_access(*access)
        ),
        FullInstructionKind::BitfieldLoad {
            address,
            descriptor,
            access,
        } => format!(
            "bitfield.load v{} field={} storage={}:{} bit={}/{} signed={} {}",
            address.0,
            descriptor.field_index,
            descriptor.storage_offset,
            descriptor.storage_size,
            descriptor.bit_offset,
            descriptor.width,
            descriptor.signed,
            display_access(*access)
        ),
        FullInstructionKind::BitfieldStore {
            address,
            value,
            descriptor,
            access,
        } => format!(
            "bitfield.store v{} -> v{} field={} storage={}:{} bit={}/{} signed={} {}",
            value.0,
            address.0,
            descriptor.field_index,
            descriptor.storage_offset,
            descriptor.storage_size,
            descriptor.bit_offset,
            descriptor.width,
            descriptor.signed,
            display_access(*access)
        ),
        FullInstructionKind::ZeroInitialize {
            destination,
            object,
        } => format!(
            "initialize.zero v{} object={}",
            destination.0,
            module.types.display_qualified(*object)
        ),
        FullInstructionKind::StringInitialize {
            destination,
            string,
            object,
            copy_code_units,
        } => format!(
            "initialize.string s{} -> v{} object={} units={}",
            string.0,
            destination.0,
            module.types.display_qualified(*object),
            copy_code_units
        ),
        FullInstructionKind::AggregateCopy {
            destination,
            source,
            destination_object,
            source_object,
            destination_access,
            source_access,
            overlap,
        } => format!(
            "aggregate.copy v{} -> v{} source={} destination={} overlap={:?} source-access={} destination-access={}",
            source.0,
            destination.0,
            module.types.display_qualified(*source_object),
            module.types.display_qualified(*destination_object),
            overlap,
            display_access(*source_access),
            display_access(*destination_access),
        ),
        FullInstructionKind::AggregateSnapshot {
            source,
            object,
            access,
        } => format!(
            "aggregate.snapshot v{} object={} {}",
            source.0,
            module.types.display_qualified(*object),
            display_access(*access)
        ),
        FullInstructionKind::AggregateProject {
            base,
            aggregate,
            projections,
        } => format!(
            "aggregate.project v{} object={} path={}",
            base.0,
            module.types.display_qualified(*aggregate),
            projections
                .iter()
                .map(|projection| match projection {
                    AggregateProjection::Field {
                        index,
                        name,
                        bitfield,
                    } => {
                        let field = name.as_ref().map_or_else(
                            || format!("field#{index}"),
                            |name| format!("field#{index}:{name}"),
                        );
                        bitfield.map_or(field.clone(), |descriptor| {
                            format!(
                                "{field}:bits({}:{}/{})",
                                descriptor.storage_offset, descriptor.bit_offset, descriptor.width
                            )
                        })
                    }
                    AggregateProjection::Index { index } => format!("index:v{}", index.0),
                })
                .collect::<Vec<_>>()
                .join("/")
        ),
        FullInstructionKind::Convert {
            kind,
            operand,
            from,
            to,
        } => format!(
            "convert.{} v{} {} -> {}",
            conversion_name(*kind),
            operand.0,
            module.types.display_qualified(*from),
            module.types.display_qualified(*to)
        ),
        FullInstructionKind::Unary { operator, operand } => {
            format!("{} v{}", unary_name(*operator), operand.0)
        }
        FullInstructionKind::Binary {
            operator,
            left,
            right,
        } => format!("{} v{}, v{}", binary_name(*operator), left.0, right.0),
        FullInstructionKind::IntegerIntrinsic { operation, operand } => {
            format!("integer.intrinsic operation={operation:?} v{}", operand.0)
        }
        FullInstructionKind::DirectCall {
            function,
            signature,
            arguments,
            variadic_boundary,
            effects,
        } => format!(
            "call.direct f{} ({}) signature={} variadic-boundary={} {}",
            function.0,
            display_values(arguments),
            module.types.display(*signature),
            variadic_boundary,
            display_call_effects(*effects)
        ),
        FullInstructionKind::IndirectCall {
            callee,
            signature,
            arguments,
            variadic_boundary,
            effects,
        } => format!(
            "call.indirect v{} ({}) signature={} variadic-boundary={} {}",
            callee.0,
            display_values(arguments),
            module.types.display(*signature),
            variadic_boundary,
            display_call_effects(*effects)
        ),
        FullInstructionKind::AtomicReadModifyWrite {
            operation,
            address,
            operand,
            object,
            return_new,
            order,
        } => format!(
            "atomic.rmw operation={operation:?} v{}, v{} object={} return-new={return_new} order={order:?}",
            address.0,
            operand.0,
            module.types.display_qualified(*object)
        ),
        FullInstructionKind::AtomicCompareExchange {
            address,
            expected,
            replacement,
            object,
            order,
        } => format!(
            "atomic.cmpxchg v{}, expected=v{}, replacement=v{} object={} order={order:?}",
            address.0,
            expected.0,
            replacement.0,
            module.types.display_qualified(*object)
        ),
        FullInstructionKind::Prefetch {
            address,
            write,
            locality,
        } => format!("prefetch v{} write={write} locality={locality}", address.0),
        FullInstructionKind::MemoryFence { order } => {
            format!("memory.fence order={order:?}")
        }
        FullInstructionKind::VaStart {
            list,
            last_named_parameter,
        } => format!("va.start v{} last=l{}", list.0, last_named_parameter.0),
        FullInstructionKind::VaArg { list, requested } => format!(
            "va.arg v{} requested={}",
            list.0,
            module.types.display_qualified(*requested)
        ),
        FullInstructionKind::VaCopy {
            destination,
            source,
        } => format!("va.copy v{} -> v{}", source.0, destination.0),
        FullInstructionKind::VaEnd { list } => format!("va.end v{}", list.0),
    }
}

fn display_terminator(terminator: &FullTerminator) -> String {
    match terminator {
        FullTerminator::Branch(edge) => format!("branch {}", display_edge(edge)),
        FullTerminator::Conditional {
            condition,
            then_edge,
            else_edge,
        } => format!(
            "conditional v{} ? {} : {}",
            condition.0,
            display_edge(then_edge),
            display_edge(else_edge)
        ),
        FullTerminator::Switch {
            selector,
            cases,
            default,
        } => format!(
            "switch v{} [{}] default {}",
            selector.0,
            cases
                .iter()
                .map(|case| format!("{} -> {}", case.value, display_edge(&case.edge)))
                .collect::<Vec<_>>()
                .join(", "),
            display_edge(default)
        ),
        FullTerminator::IndirectBranch { selector, targets } => format!(
            "br_table v{} [{}]",
            selector.0,
            targets
                .iter()
                .map(display_edge)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        FullTerminator::Return(Some(value)) => format!("return v{}", value.0),
        FullTerminator::Return(None) => "return".to_owned(),
        FullTerminator::Unreachable => "unreachable".to_owned(),
    }
}

fn display_edge(edge: &FullEdge) -> String {
    format!("b{}({})", edge.target.0, display_values(&edge.arguments))
}

fn display_values(values: &[super::ValueId]) -> String {
    values
        .iter()
        .map(|value| format!("v{}", value.0))
        .collect::<Vec<_>>()
        .join(", ")
}

fn display_constant(constant: ScalarConstant) -> String {
    match constant {
        ScalarConstant::Signed(value) => format!("signed:{value}"),
        ScalarConstant::Unsigned(value) => format!("unsigned:{value}"),
        ScalarConstant::Floating(value) => format!("float:0x{:016x}", value.to_bits()),
        ScalarConstant::NullPointer => "null".to_owned(),
    }
}

fn display_target(target: RelocationTarget) -> String {
    match target {
        RelocationTarget::Object(id) => format!("d{}", id.0),
        RelocationTarget::Function(id) => format!("f{}", id.0),
        RelocationTarget::String(id) => format!("s{}", id.0),
    }
}

fn display_access(access: MemoryAccess) -> String {
    if access == MemoryAccess::default() {
        return "[plain]".to_owned();
    }
    format!(
        "[volatile={} atomic={:?} non-elidable={} non-movable={}]",
        access.volatile, access.atomic, access.non_elidable, access.non_movable
    )
}

fn display_call_effects(effects: CallEffects) -> String {
    format!(
        "[read={} write={} unwind={} noreturn={}]",
        effects.reads_memory, effects.writes_memory, effects.may_unwind, effects.no_return
    )
}

fn conversion_name(kind: ScalarConversion) -> &'static str {
    match kind {
        ScalarConversion::ArrayToPointer => "array-to-pointer",
        ScalarConversion::FunctionToPointer => "function-to-pointer",
        ScalarConversion::IntegerPromotion => "integer-promotion",
        ScalarConversion::IntegerConversion => "integer-conversion",
        ScalarConversion::FloatingConversion => "floating-conversion",
        ScalarConversion::IntegerToFloating => "integer-to-floating",
        ScalarConversion::FloatingToInteger => "floating-to-integer",
        ScalarConversion::PointerConversion => "pointer-conversion",
        ScalarConversion::QualificationAdjustment => "qualification-adjustment",
        ScalarConversion::ToBoolean => "to-boolean",
        ScalarConversion::ToVoid => "to-void",
    }
}

fn unary_name(operator: UnaryOperation) -> &'static str {
    match operator {
        UnaryOperation::Plus => "plus",
        UnaryOperation::Negate => "negate",
        UnaryOperation::BitwiseNot => "bitwise-not",
        UnaryOperation::LogicalNot => "logical-not",
    }
}

fn binary_name(operator: BinaryOperation) -> &'static str {
    match operator {
        BinaryOperation::Multiply => "multiply",
        BinaryOperation::Divide => "divide",
        BinaryOperation::Remainder => "remainder",
        BinaryOperation::Add => "add",
        BinaryOperation::Subtract => "subtract",
        BinaryOperation::LeftShift => "left-shift",
        BinaryOperation::RightShift => "right-shift",
        BinaryOperation::Less => "less",
        BinaryOperation::LessEqual => "less-equal",
        BinaryOperation::Greater => "greater",
        BinaryOperation::GreaterEqual => "greater-equal",
        BinaryOperation::Equal => "equal",
        BinaryOperation::NotEqual => "not-equal",
        BinaryOperation::BitwiseAnd => "bitwise-and",
        BinaryOperation::BitwiseXor => "bitwise-xor",
        BinaryOperation::BitwiseOr => "bitwise-or",
    }
}
