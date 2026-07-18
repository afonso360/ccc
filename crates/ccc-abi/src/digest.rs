use ccc_ir::generic as gir;
use ccc_target::{ByteOrder, CallingConvention, EffectiveCompilationConfig};
use ccc_types::{ArrayLength, TypeKind, TypeQualifiers, TypeStore};
use sha2::{Digest as _, Sha256};

use crate::{AbiConfigKey, AbiError, IrShapeDigest, TranslationUnitDigest};

const PSABI_COMMIT: &str = "e1ce098331da5dbd66e1ffc74162380bcc213236";
const PSABI_SOURCE_SHA256: &str =
    "2d42f2ab76b99a3b8456e6bc07e18314e85991119f72db90ca1175328e27b705";

pub fn abi_config_key(config: &EffectiveCompilationConfig) -> Result<AbiConfigKey, AbiError> {
    let calling_convention = config.target.calling_convention().ok_or_else(|| {
        AbiError::new(
            "CCC3504",
            format!(
                "target `{}` does not define a C calling convention",
                config.target.triple
            ),
        )
    })?;
    let layout = config.target.data_layout;
    let data_layout = format!(
        "endian={};char_signed={};bool={}/{};char={}/{};short={}/{};int={}/{};long={}/{};long_long={}/{};pointer={}/{};float={}/{};double={}/{};long_double={}/{};wchar=width:{},signed:{};wint=width:{},signed:{};bitfields={}:{}:{}:{}:{};default_pack={}:{}",
        match layout.byte_order {
            ByteOrder::Little => "little",
            ByteOrder::Big => "big",
        },
        u8::from(layout.char_is_signed),
        layout.bool_width,
        layout.bool_align,
        layout.char_width,
        layout.char_align,
        layout.short_width,
        layout.short_align,
        layout.int_width,
        layout.int_align,
        layout.long_width,
        layout.long_align,
        layout.long_long_width,
        layout.long_long_align,
        layout.pointer_width,
        layout.pointer_align,
        layout.float_width,
        layout.float_align,
        layout.double_width,
        layout.double_align,
        layout.long_double_width,
        layout.long_double_align,
        layout.wchar_width,
        layout.wchar_is_signed as u8,
        layout.wint_width,
        layout.wint_is_signed as u8,
        layout.bitfields.order as u8,
        layout.bitfields.may_cross_storage_units as u8,
        layout.bitfields.coalesce_different_declared_types as u8,
        layout.bitfields.packed_fields_are_contiguous as u8,
        layout.bitfields.zero_width_uses_declared_alignment as u8,
        layout.default_packing.maximum_field_alignment.unwrap_or(0),
        layout.default_packing.minimum_record_alignment,
    );
    Ok(AbiConfigKey {
        schema: "ccc-abi-config-v1",
        target_triple: config.target.triple.to_string(),
        data_layout,
        calling_convention,
        boundary_profile: "sysv-amd64-lp64-v1",
        classifier_revision: 1,
        psabi_commit: PSABI_COMMIT,
        psabi_source_sha256: PSABI_SOURCE_SHA256,
        backend_profile: "cranelift-0.132.0-no-llvm-extensions-no-implicit-sret",
    })
}

pub fn ir_shape_digest(
    module: &gir::FullModule,
    key: &AbiConfigKey,
) -> Result<IrShapeDigest, AbiError> {
    let mut encoder = Encoder::new(b"ccc-ir-shape-v1");
    encode_config_key(&mut encoder, key);
    encode_types(&mut encoder, &module.types)?;
    encoder.len(module.globals.len());
    for global in &module.globals {
        encoder.u32(global.id.0);
        match global.source {
            gir::DataOrigin::FileScope(id) => {
                encoder.tag(0);
                encoder.u32(id.0);
            }
            gir::DataOrigin::BlockStatic { function, local } => {
                encoder.tag(1);
                encoder.u32(function.0);
                encoder.u32(local.0);
            }
        }
        encoder.string(&global.name);
        encoder.qualified(global.ty);
        encoder.tag(global.storage as u8);
        encoder.tag(global.linkage as u8);
        encoder.tag(global.duration as u8);
        encoder.bool(global.tentative);
        encoder.string(&global.emission.symbol_name);
        encoder.tag(global.emission.binding as u8);
        encoder.tag(global.emission.visibility as u8);
        encoder.option_string(global.emission.section.as_deref());
        encoder.option_u64(global.emission.requested_alignment);
        encoder.option_tag(global.emission.tls.map(|tls| tls as u8));
        encoder.tag(global.emission.definition as u8);
        encode_initializer(&mut encoder, global.initializer.as_ref());
        encoder.span(global.span);
    }
    encoder.len(module.strings.len());
    for string in &module.strings {
        encoder.u32(string.id.0);
        encoder.tag(string.encoding as u8);
        encoder.qualified(string.ty);
        encoder.len(string.code_units.len());
        for unit in &string.code_units {
            encoder.u32(*unit);
        }
    }
    encoder.len(module.functions.len());
    for function in &module.functions {
        encode_function(&mut encoder, function);
    }
    Ok(IrShapeDigest(Sha256::digest(encoder.finish()).into()))
}

pub fn translation_unit_digest(
    module: &gir::FullModule,
    key: &AbiConfigKey,
    ir: IrShapeDigest,
) -> TranslationUnitDigest {
    let mut encoder = Encoder::new(b"ccc-translation-unit-v1");
    encode_config_key(&mut encoder, key);
    encoder.bytes(&ir.0);
    let mut symbols = module
        .functions
        .iter()
        .map(|function| {
            (
                function.symbol_name.as_str(),
                function.linkage as u8,
                function.binding as u8,
                function.visibility as u8,
                u8::from(function.entry.is_some()),
            )
        })
        .chain(module.globals.iter().map(|global| {
            (
                global.emission.symbol_name.as_str(),
                global.linkage as u8,
                global.emission.binding as u8,
                global.emission.visibility as u8,
                global.emission.definition as u8,
            )
        }))
        .collect::<Vec<_>>();
    symbols.sort_unstable();
    encoder.len(symbols.len());
    for (name, linkage, binding, visibility, policy) in symbols {
        encoder.string(name);
        encoder.tag(linkage);
        encoder.tag(binding);
        encoder.tag(visibility);
        encoder.tag(policy);
    }
    TranslationUnitDigest(Sha256::digest(encoder.finish()).into())
}

fn encode_config_key(encoder: &mut Encoder, key: &AbiConfigKey) {
    encoder.string(key.schema);
    encoder.string(&key.target_triple);
    encoder.string(&key.data_layout);
    encoder.tag(match key.calling_convention {
        CallingConvention::SystemV => 0,
        CallingConvention::WindowsFastcall => 1,
        CallingConvention::AppleAarch64 => 2,
        CallingConvention::WasmBasicCAbi => 3,
        _ => u8::MAX,
    });
    encoder.string(key.boundary_profile);
    encoder.u32(key.classifier_revision);
    encoder.string(key.psabi_commit);
    encoder.string(key.psabi_source_sha256);
    encoder.string(key.backend_profile);
}

fn encode_types(encoder: &mut Encoder, types: &TypeStore) -> Result<(), AbiError> {
    let mut count = 0usize;
    for (id, kind) in types.iter_types() {
        encoder.u32(id.index() as u32);
        match kind {
            TypeKind::Builtin(builtin) => {
                encoder.tag(0);
                encoder.tag(*builtin as u8);
            }
            TypeKind::Pointer(pointer) => {
                encoder.tag(1);
                encoder.qualified(pointer.pointee);
            }
            TypeKind::Array(array) => {
                encoder.tag(2);
                encoder.qualified(array.element);
                match array.length {
                    ArrayLength::Incomplete => encoder.tag(0),
                    ArrayLength::Constant(length) => {
                        encoder.tag(1);
                        encoder.u64(length);
                    }
                    ArrayLength::Variable(id) => {
                        encoder.tag(2);
                        encoder.u32(id.0);
                    }
                    ArrayLength::UnspecifiedVariable(id) => {
                        encoder.tag(3);
                        encoder.u32(id.0);
                    }
                }
            }
            TypeKind::Function(signature) => {
                encoder.tag(3);
                encoder.qualified(signature.result);
                match &signature.parameters {
                    ccc_types::FunctionParameters::Unspecified => encoder.tag(0),
                    ccc_types::FunctionParameters::Prototype(parameters) => {
                        encoder.tag(1);
                        encoder.len(parameters.len());
                        for parameter in parameters {
                            encoder.qualified(*parameter);
                        }
                    }
                }
                encoder.bool(signature.variadic);
            }
            TypeKind::Enum(id) => {
                encoder.tag(4);
                encoder.u32(id.0);
                let definition = types
                    .enumeration(*id)
                    .ok_or_else(|| AbiError::new("CCC3502", format!("enum {} is unknown", id.0)))?;
                encoder.option_string(definition.tag.as_deref());
                encoder.bool(definition.body.is_some());
                if let Some(body) = &definition.body {
                    encoder.type_id(body.underlying);
                    encoder.len(body.enumerators.len());
                    for enumerator in &body.enumerators {
                        encoder.string(&enumerator.name);
                        encoder.i128(enumerator.value);
                    }
                }
            }
            TypeKind::Record(id) => {
                encoder.tag(5);
                encoder.u32(id.0);
                let definition = types.record(*id).ok_or_else(|| {
                    AbiError::new("CCC3502", format!("record {} is unknown", id.0))
                })?;
                encoder.tag(definition.kind as u8);
                encoder.option_string(definition.tag.as_deref());
                encoder.option_u64(definition.packing.maximum_field_alignment);
                encoder.u64(definition.packing.minimum_record_alignment);
                encoder.bool(definition.transparent_union);
                encoder.bool(definition.fields.is_some());
                if let Some(fields) = &definition.fields {
                    encoder.len(fields.len());
                    for field in fields {
                        encoder.option_string(field.name.as_deref());
                        encoder.qualified(field.ty);
                        encoder
                            .option_u64(field.bitfield.map(|bitfield| u64::from(bitfield.width)));
                        encoder.option_u64(field.requested_alignment);
                    }
                }
            }
        }
        count += 1;
    }
    encoder.len(count);
    Ok(())
}

fn encode_initializer(encoder: &mut Encoder, graph: Option<&gir::InitializerGraph>) {
    encoder.bool(graph.is_some());
    let Some(graph) = graph else { return };
    encoder.u32(graph.root.0);
    encoder.len(graph.nodes.len());
    for node in &graph.nodes {
        encoder.u32(node.id.0);
        encoder.qualified(node.ty);
        match &node.kind {
            gir::InitializerNodeKind::Zero => encoder.tag(0),
            gir::InitializerNodeKind::Scalar(constant) => {
                encoder.tag(1);
                encode_constant(encoder, *constant);
            }
            gir::InitializerNodeKind::Relocation {
                target,
                addend,
                one_past,
                kind,
            } => {
                encoder.tag(2);
                encode_relocation(encoder, *target);
                encoder.i128(*addend);
                encoder.bool(*one_past);
                encoder.tag(*kind as u8);
            }
            gir::InitializerNodeKind::StringData {
                string,
                copy_code_units,
            } => {
                encoder.tag(3);
                encoder.u32(string.0);
                encoder.u64(*copy_code_units);
            }
            gir::InitializerNodeKind::Repeat { element, count } => {
                encoder.tag(4);
                encoder.u32(element.0);
                encoder.u64(*count);
            }
            gir::InitializerNodeKind::Aggregate(edges) => {
                encoder.tag(5);
                encoder.len(edges.len());
                for edge in edges {
                    encoder.len(edge.path.len());
                    for path in &edge.path {
                        match path {
                            gir::InitializerPath::Index(index) => {
                                encoder.tag(0);
                                encoder.u64(*index);
                            }
                            gir::InitializerPath::Field {
                                index,
                                name,
                                bitfield,
                            } => {
                                encoder.tag(1);
                                encoder.u64(*index as u64);
                                encoder.option_string(name.as_deref());
                                encoder.bool(bitfield.is_some());
                                if let Some(bitfield) = bitfield {
                                    encode_bitfield(encoder, *bitfield);
                                }
                            }
                        }
                    }
                    encoder.u32(edge.node.0);
                }
            }
        }
    }
}

fn encode_function(encoder: &mut Encoder, function: &gir::FullFunction) {
    encoder.u32(function.id.0);
    encoder.string(&function.name);
    encoder.type_id(function.signature);
    encoder.tag(function.storage_class as u8);
    encoder.tag(function.linkage as u8);
    encoder.tag(function.binding as u8);
    encoder.tag(function.visibility as u8);
    encoder.bool(function.properties.inline);
    encoder.bool(function.properties.no_return);
    encoder.string(&function.symbol_name);
    encoder.qualified(function.result_type);
    encoder.len(function.parameters.len());
    for parameter in &function.parameters {
        encoder.u32(parameter.local.0);
        encoder.string(&parameter.name);
        encoder.qualified(parameter.ty);
        encoder.option_u64(parameter.incoming.map(|value| u64::from(value.0)));
        encoder.option_u64(parameter.storage.map(|storage| u64::from(storage.0)));
        encoder.span(parameter.span);
    }
    encoder.len(function.storage.len());
    for storage in &function.storage {
        encoder.u32(storage.id.0);
        encoder.u32(storage.local.0);
        encoder.string(&storage.name);
        encoder.qualified(storage.ty);
        encoder.tag(storage.duration as u8);
        encoder.tag(storage.location as u8);
        encoder.option_u64(storage.requested_alignment);
        encoder.len(storage.required_by.len());
        for reason in &storage.required_by {
            encoder.tag(*reason as u8);
        }
        encoder.span(storage.span);
    }
    encoder.option_u64(function.entry.map(|block| u64::from(block.0)));
    encoder.len(function.value_types.len());
    for ty in &function.value_types {
        encoder.type_id(*ty);
    }
    encoder.u32(function.instruction_count);
    encoder.len(function.blocks.len());
    for block in &function.blocks {
        encoder.u32(block.id.0);
        encoder.len(block.parameters.len());
        for parameter in &block.parameters {
            encoder.u32(parameter.0);
        }
        encoder.len(block.instructions.len());
        for instruction in &block.instructions {
            encoder.u32(instruction.id.0);
            encoder.option_u64(instruction.result.map(|value| u64::from(value.0)));
            encode_instruction(encoder, &instruction.kind);
            encoder.span(instruction.span);
        }
        encoder.bool(block.terminator.is_some());
        if let Some(terminator) = &block.terminator {
            encode_terminator(encoder, terminator);
        }
    }
    encoder.span(function.span);
}

fn encode_instruction(encoder: &mut Encoder, instruction: &gir::FullInstructionKind) {
    use gir::FullInstructionKind as I;
    match instruction {
        I::Constant(value) => {
            encoder.tag(0);
            encode_constant(encoder, *value);
        }
        I::AddressConstant {
            target,
            addend,
            one_past,
        } => {
            encoder.tag(1);
            encode_relocation(encoder, *target);
            encoder.i128(*addend);
            encoder.bool(*one_past);
        }
        I::AddressOfGlobal { global } => {
            encoder.tag(2);
            encoder.u32(global.0);
        }
        I::AddressOfFunction {
            function,
            signature,
        } => {
            encoder.tag(3);
            encoder.u32(function.0);
            encoder.type_id(*signature);
        }
        I::AddressOfString { string } => {
            encoder.tag(4);
            encoder.u32(string.0);
        }
        I::AddressOfStorage { storage } => {
            encoder.tag(5);
            encoder.u32(storage.0);
        }
        I::ProjectField {
            base,
            record,
            field_index,
            field_name,
        } => {
            encoder.tag(6);
            encoder.u32(base.0);
            encoder.qualified(*record);
            encoder.u64(*field_index as u64);
            encoder.option_string(field_name.as_deref());
        }
        I::PointerOffset {
            base,
            index,
            element,
            subtract,
        } => {
            encoder.tag(7);
            encoder.u32(base.0);
            encoder.u32(index.0);
            encoder.qualified(*element);
            encoder.bool(*subtract);
        }
        I::PointerDifference {
            left,
            right,
            element,
        } => {
            encoder.tag(8);
            encoder.u32(left.0);
            encoder.u32(right.0);
            encoder.qualified(*element);
        }
        I::Load {
            address,
            object,
            access,
        } => {
            encoder.tag(9);
            encoder.u32(address.0);
            encoder.qualified(*object);
            encode_access(encoder, *access);
        }
        I::Store {
            address,
            value,
            object,
            access,
        } => {
            encoder.tag(10);
            encoder.u32(address.0);
            encoder.u32(value.0);
            encoder.qualified(*object);
            encode_access(encoder, *access);
        }
        I::BitfieldLoad {
            address,
            descriptor,
            access,
        } => {
            encoder.tag(11);
            encoder.u32(address.0);
            encode_bitfield(encoder, *descriptor);
            encode_access(encoder, *access);
        }
        I::BitfieldStore {
            address,
            value,
            descriptor,
            access,
        } => {
            encoder.tag(12);
            encoder.u32(address.0);
            encoder.u32(value.0);
            encode_bitfield(encoder, *descriptor);
            encode_access(encoder, *access);
        }
        I::ZeroInitialize {
            destination,
            object,
        } => {
            encoder.tag(13);
            encoder.u32(destination.0);
            encoder.qualified(*object);
        }
        I::StringInitialize {
            destination,
            string,
            object,
            copy_code_units,
        } => {
            encoder.tag(14);
            encoder.u32(destination.0);
            encoder.u32(string.0);
            encoder.qualified(*object);
            encoder.u64(*copy_code_units);
        }
        I::AggregateCopy {
            destination,
            source,
            destination_object,
            source_object,
            destination_access,
            source_access,
            overlap,
        } => {
            encoder.tag(15);
            encoder.u32(destination.0);
            encoder.u32(source.0);
            encoder.qualified(*destination_object);
            encoder.qualified(*source_object);
            encode_access(encoder, *destination_access);
            encode_access(encoder, *source_access);
            encoder.tag(*overlap as u8);
        }
        I::AggregateSnapshot {
            source,
            object,
            access,
        } => {
            encoder.tag(16);
            encoder.u32(source.0);
            encoder.qualified(*object);
            encode_access(encoder, *access);
        }
        I::AggregateProject {
            base,
            aggregate,
            projections,
        } => {
            encoder.tag(17);
            encoder.u32(base.0);
            encoder.qualified(*aggregate);
            encoder.len(projections.len());
            for projection in projections {
                match projection {
                    gir::AggregateProjection::Field {
                        index,
                        name,
                        bitfield,
                    } => {
                        encoder.tag(0);
                        encoder.u64(*index as u64);
                        encoder.option_string(name.as_deref());
                        encoder.bool(bitfield.is_some());
                        if let Some(bitfield) = bitfield {
                            encode_bitfield(encoder, *bitfield);
                        }
                    }
                    gir::AggregateProjection::Index { index } => {
                        encoder.tag(1);
                        encoder.u32(index.0);
                    }
                }
            }
        }
        I::Convert {
            kind,
            operand,
            from,
            to,
        } => {
            encoder.tag(18);
            encoder.tag(*kind as u8);
            encoder.u32(operand.0);
            encoder.qualified(*from);
            encoder.qualified(*to);
        }
        I::Unary { operator, operand } => {
            encoder.tag(19);
            encoder.tag(*operator as u8);
            encoder.u32(operand.0);
        }
        I::Binary {
            operator,
            left,
            right,
        } => {
            encoder.tag(20);
            encoder.tag(*operator as u8);
            encoder.u32(left.0);
            encoder.u32(right.0);
        }
        I::DirectCall {
            function,
            signature,
            arguments,
            variadic_boundary,
            effects,
        } => {
            encoder.tag(21);
            encoder.u32(function.0);
            encoder.type_id(*signature);
            encode_values(encoder, arguments);
            encoder.u64(*variadic_boundary as u64);
            encode_effects(encoder, *effects);
        }
        I::IndirectCall {
            callee,
            signature,
            arguments,
            variadic_boundary,
            effects,
        } => {
            encoder.tag(22);
            encoder.u32(callee.0);
            encoder.type_id(*signature);
            encode_values(encoder, arguments);
            encoder.u64(*variadic_boundary as u64);
            encode_effects(encoder, *effects);
        }
        I::VaStart {
            list,
            last_named_parameter,
        } => {
            encoder.tag(23);
            encoder.u32(list.0);
            encoder.u32(last_named_parameter.0);
        }
        I::VaArg { list, requested } => {
            encoder.tag(24);
            encoder.u32(list.0);
            encoder.qualified(*requested);
        }
        I::VaCopy {
            destination,
            source,
        } => {
            encoder.tag(25);
            encoder.u32(destination.0);
            encoder.u32(source.0);
        }
        I::VaEnd { list } => {
            encoder.tag(26);
            encoder.u32(list.0);
        }
        I::MemoryFence { order } => {
            encoder.tag(27);
            encoder.tag(*order as u8);
        }
        I::AtomicReadModifyWrite {
            operation,
            address,
            operand,
            object,
            return_new,
            order,
        } => {
            encoder.tag(28);
            encoder.tag(*operation as u8);
            encoder.u32(address.0);
            encoder.u32(operand.0);
            encoder.qualified(*object);
            encoder.bool(*return_new);
            encoder.tag(*order as u8);
        }
        I::AtomicCompareExchange {
            address,
            expected,
            replacement,
            object,
            order,
        } => {
            encoder.tag(29);
            encoder.u32(address.0);
            encoder.u32(expected.0);
            encoder.u32(replacement.0);
            encoder.qualified(*object);
            encoder.tag(*order as u8);
        }
        I::IntegerIntrinsic { operation, operand } => {
            encoder.tag(30);
            encoder.tag(*operation as u8);
            encoder.u32(operand.0);
        }
        I::Prefetch {
            address,
            write,
            locality,
        } => {
            encoder.tag(31);
            encoder.u32(address.0);
            encoder.bool(*write);
            encoder.tag(*locality);
        }
        I::RuntimeSizedAllocate {
            storage,
            extents,
            element,
            constant_factor,
            requested_alignment,
        } => {
            // Append-only instruction tag: existing ABI digests retain their
            // byte-for-byte encoding when runtime-sized storage is unused.
            encoder.tag(32);
            encoder.u32(storage.0);
            encoder.len(extents.len());
            for extent in extents {
                encoder.u32(extent.0);
            }
            encoder.qualified(*element);
            encoder.u64(*constant_factor);
            encoder.option_u64(*requested_alignment);
        }
        I::RuntimePointerOffset {
            base,
            index,
            element,
            extents,
            subtract,
        } => {
            encoder.tag(33);
            encoder.u32(base.0);
            encoder.u32(index.0);
            encoder.qualified(*element);
            encoder.len(extents.len());
            for extent in extents {
                encoder.u32(extent.0);
            }
            encoder.bool(*subtract);
        }
        I::RuntimePointerDifference {
            left,
            right,
            element,
            extents,
        } => {
            encoder.tag(34);
            encoder.u32(left.0);
            encoder.u32(right.0);
            encoder.qualified(*element);
            encoder.len(extents.len());
            for extent in extents {
                encoder.u32(extent.0);
            }
        }
    }
}

fn encode_terminator(encoder: &mut Encoder, terminator: &gir::FullTerminator) {
    match terminator {
        gir::FullTerminator::Branch(edge) => {
            encoder.tag(0);
            encode_edge(encoder, edge);
        }
        gir::FullTerminator::Conditional {
            condition,
            then_edge,
            else_edge,
        } => {
            encoder.tag(1);
            encoder.u32(condition.0);
            encode_edge(encoder, then_edge);
            encode_edge(encoder, else_edge);
        }
        gir::FullTerminator::Switch {
            selector,
            cases,
            default,
        } => {
            encoder.tag(2);
            encoder.u32(selector.0);
            encoder.len(cases.len());
            for case in cases {
                encoder.i128(case.value);
                encode_edge(encoder, &case.edge);
            }
            encode_edge(encoder, default);
        }
        gir::FullTerminator::Return(value) => {
            encoder.tag(3);
            encoder.option_u64(value.map(|value| u64::from(value.0)));
        }
        gir::FullTerminator::Unreachable => encoder.tag(4),
        gir::FullTerminator::IndirectBranch { selector, targets } => {
            encoder.tag(5);
            encoder.u32(selector.0);
            encoder.len(targets.len());
            for target in targets {
                encode_edge(encoder, target);
            }
        }
    }
}

fn encode_edge(encoder: &mut Encoder, edge: &gir::FullEdge) {
    encoder.u32(edge.target.0);
    encode_values(encoder, &edge.arguments);
}

fn encode_values(encoder: &mut Encoder, values: &[gir::ValueId]) {
    encoder.len(values.len());
    for value in values {
        encoder.u32(value.0);
    }
}

fn encode_constant(encoder: &mut Encoder, constant: gir::ScalarConstant) {
    match constant {
        gir::ScalarConstant::Signed(value) => {
            encoder.tag(0);
            encoder.i128(value);
        }
        gir::ScalarConstant::Unsigned(value) => {
            encoder.tag(1);
            encoder.u128(value);
        }
        gir::ScalarConstant::Floating(value) => {
            encoder.tag(2);
            encoder.u64(value.to_bits());
        }
        gir::ScalarConstant::NullPointer => encoder.tag(3),
    }
}

fn encode_relocation(encoder: &mut Encoder, target: gir::RelocationTarget) {
    match target {
        gir::RelocationTarget::Object(id) => {
            encoder.tag(0);
            encoder.u32(id.0);
        }
        gir::RelocationTarget::Function(id) => {
            encoder.tag(1);
            encoder.u32(id.0);
        }
        gir::RelocationTarget::String(id) => {
            encoder.tag(2);
            encoder.u32(id.0);
        }
    }
}

fn encode_access(encoder: &mut Encoder, access: gir::MemoryAccess) {
    encoder.bool(access.volatile);
    encoder.option_tag(access.atomic.map(|order| order as u8));
    encoder.bool(access.non_elidable);
    encoder.bool(access.non_movable);
}

fn encode_bitfield(encoder: &mut Encoder, descriptor: gir::BitfieldDescriptor) {
    encoder.u64(descriptor.field_index as u64);
    encoder.u64(descriptor.storage_offset);
    encoder.u64(descriptor.storage_size);
    encoder.u64(descriptor.storage_align);
    encoder.u32(descriptor.bit_offset);
    encoder.u32(descriptor.width);
    encoder.bool(descriptor.signed);
}

fn encode_effects(encoder: &mut Encoder, effects: gir::CallEffects) {
    encoder.bool(effects.reads_memory);
    encoder.bool(effects.writes_memory);
    encoder.bool(effects.may_unwind);
    encoder.bool(effects.no_return);
}

struct Encoder {
    bytes: Vec<u8>,
}

impl Encoder {
    fn new(domain: &[u8]) -> Self {
        let mut this = Self { bytes: Vec::new() };
        this.bytes(domain);
        this
    }
    fn finish(self) -> Vec<u8> {
        self.bytes
    }
    fn tag(&mut self, value: u8) {
        self.u8(value);
    }
    fn u8(&mut self, value: u8) {
        self.bytes.push(value);
    }
    fn bool(&mut self, value: bool) {
        self.u8(u8::from(value));
    }
    fn u32(&mut self, value: u32) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }
    fn u64(&mut self, value: u64) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }
    fn u128(&mut self, value: u128) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }
    fn i128(&mut self, value: i128) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }
    fn len(&mut self, value: usize) {
        self.u64(value as u64);
    }
    fn bytes(&mut self, value: &[u8]) {
        self.len(value.len());
        self.bytes.extend_from_slice(value);
    }
    fn string(&mut self, value: &str) {
        self.bytes(value.as_bytes());
    }
    fn type_id(&mut self, value: ccc_types::TypeId) {
        self.u64(value.index() as u64);
    }
    fn qualified(&mut self, value: ccc_types::QualifiedType) {
        self.type_id(value.ty);
        self.bool(value.qualifiers.contains(TypeQualifiers::CONST));
        self.bool(value.qualifiers.contains(TypeQualifiers::VOLATILE));
        self.bool(value.qualifiers.contains(TypeQualifiers::RESTRICT));
        self.bool(value.qualifiers.contains(TypeQualifiers::ATOMIC));
    }
    fn option_u64(&mut self, value: Option<u64>) {
        self.bool(value.is_some());
        if let Some(value) = value {
            self.u64(value);
        }
    }
    fn option_tag(&mut self, value: Option<u8>) {
        self.bool(value.is_some());
        if let Some(value) = value {
            self.tag(value);
        }
    }
    fn option_string(&mut self, value: Option<&str>) {
        self.bool(value.is_some());
        if let Some(value) = value {
            self.string(value);
        }
    }
    fn span(&mut self, span: ccc_session::Span) {
        self.u32(span.file.index());
        self.u64(span.start as u64);
        self.u64(span.end as u64);
        self.u32(span.origin.index());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_key_excludes_tool_paths() {
        let mut left = EffectiveCompilationConfig::default();
        let mut right = left.clone();
        left.resource_dir = Some("/first".into());
        right.resource_dir = Some("/second".into());
        assert_eq!(
            abi_config_key(&left).unwrap(),
            abi_config_key(&right).unwrap()
        );
    }
}
