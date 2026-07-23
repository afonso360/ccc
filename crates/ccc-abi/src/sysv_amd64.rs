use ccc_target::{
    Architecture, BinaryFormat, CallingConvention, EffectiveCompilationConfig, Environment,
    LongDoubleFormat, OperatingSystem,
};
use ccc_types::{
    BuiltinType, FunctionParameters, FunctionType, LayoutShape, RecordKind, TypeId, TypeKind,
    TypeStore,
};

use crate::{AbiError, boundary_value_alignment, model::*};

pub fn plan_function_type(
    types: &TypeStore,
    signature: TypeId,
    config: &EffectiveCompilationConfig,
) -> Result<NativeBoundaryPlan, AbiError> {
    validate_target(config)?;
    let signature = function_signature(types, signature)?;
    if signature.variadic {
        return Err(AbiError::new(
            "CCC3510",
            "a variadic boundary must use an explicit bridge plan",
        ));
    }
    if signature_requires_bridge(types, &signature) {
        return Err(AbiError::new(
            "CCC3510",
            "this fixed boundary requires the explicit System V bridge plan",
        ));
    }
    plan_native_signature(types, &signature, config)
}

pub fn plan_boundary_type(
    types: &TypeStore,
    signature: TypeId,
    config: &EffectiveCompilationConfig,
) -> Result<BoundaryPlan, AbiError> {
    validate_target(config)?;
    let signature = function_signature(types, signature)?;
    if signature.variadic {
        let FunctionParameters::Prototype(parameters) = &signature.parameters else {
            return Err(AbiError::new(
                "CCC3506",
                "a function type without a prototype has no ABI boundary plan",
            ));
        };
        let actual = parameters
            .iter()
            .map(|parameter| parameter.ty)
            .collect::<Vec<_>>();
        Ok(BoundaryPlan::Bridge(plan_bridge(
            types,
            &signature,
            parameters,
            &actual,
            parameters.len(),
            BridgeKind::VariadicEntry,
            config,
        )?))
    } else if signature_requires_bridge(types, &signature) {
        let FunctionParameters::Prototype(parameters) = &signature.parameters else {
            return Err(AbiError::new(
                "CCC3506",
                "a function type without a prototype has no fixed ABI plan",
            ));
        };
        let actual = parameters
            .iter()
            .map(|parameter| parameter.ty)
            .collect::<Vec<_>>();
        Ok(BoundaryPlan::Bridge(plan_bridge(
            types,
            &signature,
            parameters,
            &actual,
            parameters.len(),
            BridgeKind::FixedEntry,
            config,
        )?))
    } else {
        Ok(BoundaryPlan::Native(plan_native_signature(
            types, &signature, config,
        )?))
    }
}

pub fn plan_fixed_call(
    types: &TypeStore,
    signature: TypeId,
    actual_types: &[TypeId],
    config: &EffectiveCompilationConfig,
) -> Result<BridgeBoundaryPlan, AbiError> {
    validate_target(config)?;
    let signature = function_signature(types, signature)?;
    if signature.variadic {
        return Err(AbiError::new(
            "CCC3511",
            "a variadic function type requires a variadic call plan",
        ));
    }
    let FunctionParameters::Prototype(fixed) = &signature.parameters else {
        return Err(AbiError::new(
            "CCC3506",
            "a function type without a prototype has no fixed call plan",
        ));
    };
    if !signature_requires_bridge(types, &signature) {
        return Err(AbiError::new(
            "CCC3511",
            "this fixed call does not require an explicit System V bridge",
        ));
    }
    plan_bridge(
        types,
        &signature,
        fixed,
        actual_types,
        fixed.len(),
        BridgeKind::FixedCall,
        config,
    )
}

pub fn plan_variadic_call(
    types: &TypeStore,
    signature: TypeId,
    actual_types: &[TypeId],
    variadic_boundary: usize,
    config: &EffectiveCompilationConfig,
) -> Result<BridgeBoundaryPlan, AbiError> {
    validate_target(config)?;
    let signature = function_signature(types, signature)?;
    if !signature.variadic {
        return Err(AbiError::new(
            "CCC3511",
            "a nonvariadic function type does not require a variadic call bridge",
        ));
    }
    let FunctionParameters::Prototype(fixed) = &signature.parameters else {
        return Err(AbiError::new(
            "CCC3506",
            "a function type without a prototype has no variadic bridge plan",
        ));
    };
    plan_bridge(
        types,
        &signature,
        fixed,
        actual_types,
        variadic_boundary,
        BridgeKind::VariadicCall,
        config,
    )
}

pub fn plan_unprototyped_call(
    types: &TypeStore,
    signature: TypeId,
    promoted_actual_types: &[TypeId],
    config: &EffectiveCompilationConfig,
) -> Result<BridgeBoundaryPlan, AbiError> {
    validate_target(config)?;
    let signature = function_signature(types, signature)?;
    if !matches!(signature.parameters, FunctionParameters::Unspecified) || signature.variadic {
        return Err(AbiError::new(
            "CCC3511",
            "a function type with a prototype does not require an unprototyped call bridge",
        ));
    }
    plan_bridge(
        types,
        &signature,
        &[],
        promoted_actual_types,
        0,
        BridgeKind::UnprototypedCall,
        config,
    )
}

pub fn classify_type(
    types: &TypeStore,
    ty: TypeId,
    config: &EffectiveCompilationConfig,
) -> Result<ClassifiedType, AbiError> {
    validate_target(config)?;
    classify(types, ty, config, "boundary")
}

pub fn plan_va_arg(
    types: &TypeStore,
    ty: TypeId,
    config: &EffectiveCompilationConfig,
) -> Result<VaArgPlan, AbiError> {
    let classified = classify_type(types, ty, config)?;
    if classified.passing == PassingMode::Void {
        return Err(AbiError::new("CCC3514", "`va_arg` cannot request `void`"));
    }
    let gp_slots = classified
        .pieces
        .iter()
        .filter(|piece| piece.class == AbiClass::Integer)
        .count() as u8;
    let sse_slots = classified
        .pieces
        .iter()
        .filter(|piece| matches!(piece.class, AbiClass::Sse | AbiClass::SseUp))
        .count() as u8;
    let overflow_align = boundary_value_alignment(types, &classified, config)?.max(8);
    if overflow_align > 16 {
        return Err(AbiError::new(
            "CCC3513",
            format!(
                "variadic type `{}` requires unsupported {}-byte overflow alignment",
                types.display(ty),
                overflow_align
            ),
        ));
    }
    Ok(VaArgPlan {
        result_size: classified.size,
        result_align: classified.align,
        overflow_size: align_up(classified.size, 8)?,
        overflow_align,
        classified,
        gp_slots,
        sse_slots,
        indirect: false,
    })
}

pub(crate) fn validate_target(config: &EffectiveCompilationConfig) -> Result<(), AbiError> {
    if config.target.triple.architecture != Architecture::X86_64
        || config.target.triple.operating_system != OperatingSystem::Linux
        || config.target.triple.environment != Environment::Gnu
        || config.target.triple.binary_format != BinaryFormat::Elf
        || config.target.pointer_width() != Some(64)
        || config.target.int_width() != Some(32)
        || config.target.data_layout.int_width != 32
        || config.target.data_layout.long_width != 64
        || config.target.data_layout.pointer_width != 64
    {
        return Err(AbiError::new(
            "CCC3504",
            format!(
                "target `{}` does not provide the x86-64 ELF ABI profile",
                config.target.triple
            ),
        ));
    }
    if config.target.calling_convention() != Some(CallingConvention::SystemV) {
        return Err(AbiError::new(
            "CCC3504",
            format!(
                "target `{}` does not use the System V calling convention",
                config.target.triple
            ),
        ));
    }
    Ok(())
}

fn function_signature(types: &TypeStore, signature: TypeId) -> Result<FunctionType, AbiError> {
    types.function_signature(signature).ok_or_else(|| {
        AbiError::new(
            "CCC3505",
            format!(
                "type `{}` is not a function type and has no function ABI plan",
                types.display(signature)
            ),
        )
    })
}

fn signature_requires_bridge(types: &TypeStore, signature: &FunctionType) -> bool {
    fn contains(types: &TypeStore, ty: TypeId, active: &mut Vec<TypeId>) -> bool {
        if matches!(
            types.builtin_type(ty),
            Some(BuiltinType::Int128 | BuiltinType::UnsignedInt128 | BuiltinType::LongDouble)
        ) {
            return true;
        }
        if active.contains(&ty) {
            return false;
        }
        active.push(ty);
        let result = match types.try_kind(ty) {
            Some(TypeKind::Array(array)) => contains(types, array.element.ty, active),
            Some(TypeKind::Record(id)) => types
                .record(*id)
                .and_then(|record| record.fields.as_ref())
                .is_some_and(|fields| {
                    fields
                        .iter()
                        .any(|field| contains(types, field.ty.ty, active))
                }),
            _ => false,
        };
        active.pop();
        result
    }

    contains(types, signature.result.ty, &mut Vec::new())
        || match &signature.parameters {
            FunctionParameters::Prototype(parameters) => parameters
                .iter()
                .any(|parameter| contains(types, parameter.ty, &mut Vec::new())),
            FunctionParameters::Unspecified => false,
        }
}

fn plan_native_signature(
    types: &TypeStore,
    signature: &FunctionType,
    config: &EffectiveCompilationConfig,
) -> Result<NativeBoundaryPlan, AbiError> {
    let FunctionParameters::Prototype(parameters) = &signature.parameters else {
        return Err(AbiError::new(
            "CCC3506",
            "a function type without a prototype has no fixed ABI plan",
        ));
    };

    let result_classified = classify(types, signature.result.ty, config, "return")?;
    reject_array_boundary(types, signature.result.ty, "return")?;
    let indirect = result_classified.passing == PassingMode::Memory;
    let mut clif_parameters = Vec::new();
    if indirect {
        clif_parameters.push(NativeCarrierPlan {
            abi_param_index: 0,
            source_index: None,
            piece_index: None,
            source_offset: 0,
            valid_bytes: 8,
            class: AbiClass::Integer,
            carrier: AbiCarrier::I64,
            extension: IntegerExtension::None,
            purpose: NativePurpose::StructReturn,
        });
    }
    let mut gp_used = u8::from(indirect);
    let mut sse_used = 0u8;
    let mut planned_parameters = Vec::with_capacity(parameters.len());
    for (source_index, parameter) in parameters.iter().enumerate() {
        reject_array_boundary(types, parameter.ty, "parameter")?;
        let mut classified = classify(types, parameter.ty, config, "parameter")?;
        if classified.passing == PassingMode::Void {
            return Err(AbiError::new(
                "CCC3507",
                "`void` cannot appear as a function parameter type",
            ));
        }
        let source_index = source_index as u32;
        let mut carrier_indices = Vec::new();
        match classified.passing {
            PassingMode::Scalar => {
                let scalar = boundary_scalar(types, parameter.ty, config, "parameter")?;
                let class = scalar_class(scalar);
                consume_scalar_register(scalar, &mut gp_used, &mut sse_used);
                carrier_indices.push(push_native_carrier(
                    &mut clif_parameters,
                    Some(source_index),
                    None,
                    0,
                    scalar_size(scalar),
                    class,
                    scalar_carrier(scalar),
                    scalar_extension(scalar),
                    NativePurpose::Normal,
                )?);
            }
            PassingMode::Registers => {
                let (gp_needed, sse_needed) = register_counts(&classified);
                if gp_used + gp_needed > 6 || sse_used + sse_needed > 8 {
                    classified.passing = PassingMode::Memory;
                    carrier_indices.push(push_struct_argument(
                        &mut clif_parameters,
                        source_index,
                        classified.size,
                    )?);
                } else {
                    gp_used += gp_needed;
                    sse_used += sse_needed;
                    for piece in &classified.pieces {
                        carrier_indices.push(push_native_carrier(
                            &mut clif_parameters,
                            Some(source_index),
                            Some(piece.index),
                            piece.offset,
                            piece.valid_bytes,
                            piece.class,
                            piece_carrier(piece),
                            IntegerExtension::None,
                            NativePurpose::Normal,
                        )?);
                    }
                }
            }
            PassingMode::Memory => carrier_indices.push(push_struct_argument(
                &mut clif_parameters,
                source_index,
                classified.size,
            )?),
            PassingMode::Void => unreachable!(),
        }
        planned_parameters.push(NativeParameterPlan {
            source_index,
            ty: parameter.ty,
            classified,
            carrier_indices,
        });
    }

    let mut clif_results = Vec::new();
    let result = match result_classified.passing {
        PassingMode::Void => NativeResultPlan::Void,
        PassingMode::Scalar => {
            let scalar = boundary_scalar(types, signature.result.ty, config, "return")?;
            let carrier_index = push_native_carrier(
                &mut clif_results,
                None,
                None,
                0,
                scalar_size(scalar),
                scalar_class(scalar),
                scalar_carrier(scalar),
                scalar_extension(scalar),
                NativePurpose::Normal,
            )?;
            NativeResultPlan::Scalar {
                ty: signature.result.ty,
                carrier_index,
            }
        }
        PassingMode::Registers => {
            let mut carrier_indices = Vec::new();
            for piece in &result_classified.pieces {
                carrier_indices.push(push_native_carrier(
                    &mut clif_results,
                    None,
                    Some(piece.index),
                    piece.offset,
                    piece.valid_bytes,
                    piece.class,
                    piece_carrier(piece),
                    IntegerExtension::None,
                    NativePurpose::Normal,
                )?);
            }
            NativeResultPlan::RegisterAggregate {
                classified: result_classified,
                carrier_indices,
            }
        }
        PassingMode::Memory => NativeResultPlan::Indirect {
            classified: result_classified,
            sret_parameter_index: 0,
        },
    };

    Ok(NativeBoundaryPlan {
        calling_convention: CallingConvention::SystemV,
        parameters: planned_parameters,
        result,
        clif_parameters,
        clif_results,
        variadic: false,
    })
}

fn push_struct_argument(
    carriers: &mut Vec<NativeCarrierPlan>,
    source_index: u32,
    size: u64,
) -> Result<u32, AbiError> {
    let padded = u32::try_from(align_up(size, 8)?).map_err(|_| {
        AbiError::new(
            "CCC3503",
            "aggregate ABI staging size exceeds the backend limit",
        )
    })?;
    push_native_carrier(
        carriers,
        Some(source_index),
        None,
        0,
        8,
        AbiClass::Memory,
        AbiCarrier::I64,
        IntegerExtension::None,
        NativePurpose::StructArgument(padded),
    )
}

#[allow(clippy::too_many_arguments)]
fn push_native_carrier(
    carriers: &mut Vec<NativeCarrierPlan>,
    source_index: Option<u32>,
    piece_index: Option<u8>,
    source_offset: u64,
    valid_bytes: u8,
    class: AbiClass,
    carrier: AbiCarrier,
    extension: IntegerExtension,
    purpose: NativePurpose,
) -> Result<u32, AbiError> {
    let index = u32::try_from(carriers.len())
        .map_err(|_| AbiError::new("CCC3503", "ABI carrier index overflow"))?;
    carriers.push(NativeCarrierPlan {
        abi_param_index: index,
        source_index,
        piece_index,
        source_offset,
        valid_bytes,
        class,
        carrier,
        extension,
        purpose,
    });
    Ok(index)
}

fn plan_bridge(
    types: &TypeStore,
    signature: &FunctionType,
    fixed: &[ccc_types::QualifiedType],
    actual_types: &[TypeId],
    variadic_boundary: usize,
    kind: BridgeKind,
    config: &EffectiveCompilationConfig,
) -> Result<BridgeBoundaryPlan, AbiError> {
    if variadic_boundary != fixed.len() || actual_types.len() < fixed.len() {
        return Err(AbiError::new(
            "CCC3512",
            format!(
                "variadic call has boundary {variadic_boundary} and {} actual types for {} fixed parameters",
                actual_types.len(),
                fixed.len()
            ),
        ));
    }
    for (index, (actual, expected)) in actual_types.iter().zip(fixed).enumerate() {
        if *actual != expected.ty {
            return Err(AbiError::new(
                "CCC3512",
                format!(
                    "variadic call fixed argument {index} has type `{}`, expected `{}`",
                    types.display(*actual),
                    types.display(expected.ty)
                ),
            ));
        }
    }

    let result = classify(types, signature.result.ty, config, "return")?;
    reject_array_boundary(types, signature.result.ty, "return")?;
    let hidden_return = result.passing == PassingMode::Memory;
    let mut gp_used = u8::from(hidden_return);
    let mut xmm_used = 0u8;
    let mut stack_size = 0u64;
    let mut parameters = Vec::with_capacity(actual_types.len());
    let mut parameter_pieces = Vec::new();
    let mut fixed_stack_end = (variadic_boundary == 0).then_some(0);

    for (source_index, ty) in actual_types.iter().copied().enumerate() {
        reject_array_boundary(types, ty, "parameter")?;
        let classified = classify(types, ty, config, "parameter")?;
        if classified.passing == PassingMode::Void {
            return Err(AbiError::new(
                "CCC3507",
                "`void` cannot appear as a function parameter type",
            ));
        }
        let extension = if classified.passing == PassingMode::Scalar
            && types.builtin_type(ty) != Some(BuiltinType::LongDouble)
        {
            scalar_extension(boundary_scalar(types, ty, config, "parameter")?)
        } else {
            IntegerExtension::None
        };
        allocate_bridge_argument(
            &classified,
            boundary_value_alignment(types, &classified, config)?,
            source_index as u32,
            extension,
            &mut gp_used,
            &mut xmm_used,
            &mut stack_size,
            &mut parameter_pieces,
        )?;
        parameters.push(classified);
        if source_index + 1 == variadic_boundary {
            fixed_stack_end = Some(stack_size);
        }
    }
    let fixed_stack_end = fixed_stack_end.unwrap_or(stack_size);
    let stack_size = u32::try_from(align_up(stack_size, 16)?)
        .map_err(|_| AbiError::new("CCC3503", "variadic bridge stack payload is too large"))?;
    let result_extension = if result.passing == PassingMode::Scalar
        && types.builtin_type(signature.result.ty) != Some(BuiltinType::LongDouble)
    {
        scalar_extension(boundary_scalar(
            types,
            signature.result.ty,
            config,
            "return",
        )?)
    } else {
        IntegerExtension::None
    };
    let result_pieces = bridge_result_pieces(&result, result_extension);
    Ok(BridgeBoundaryPlan {
        abi_identity: ccc_target::AbiIdentity::SysvAmd64Lp64,
        calling_convention: CallingConvention::SystemV,
        kind,
        parameters,
        parameter_pieces,
        result,
        result_pieces,
        hidden_return,
        overflow_arg_offset: u32::try_from(fixed_stack_end).map_err(|_| {
            AbiError::new("CCC3503", "variadic overflow argument offset is too large")
        })?,
        stack_size,
        gp_used,
        xmm_used,
        variadic_sse_count: xmm_used.min(8),
    })
}

#[allow(clippy::too_many_arguments)]
fn allocate_bridge_argument(
    classified: &ClassifiedType,
    boundary_alignment: u64,
    source_index: u32,
    extension: IntegerExtension,
    gp_used: &mut u8,
    sse_used: &mut u8,
    stack_size: &mut u64,
    pieces: &mut Vec<BridgePiecePlan>,
) -> Result<(), AbiError> {
    let (gp_needed, sse_needed) = register_counts(classified);
    let has_x87 = classified.classes.iter().any(|class| {
        matches!(
            class,
            AbiClass::X87 | AbiClass::X87Up | AbiClass::ComplexX87
        )
    });
    let registers_fit = classified.passing != PassingMode::Memory
        && !has_x87
        && *gp_used + gp_needed <= 6
        && *sse_used + sse_needed <= 8;
    if registers_fit {
        for piece in effective_pieces(classified) {
            let location = match piece.class {
                AbiClass::Integer => {
                    let register = RegisterSlot::integer(*gp_used);
                    *gp_used += 1;
                    BridgeLocation::Register(register)
                }
                AbiClass::Sse | AbiClass::SseUp => {
                    let register = RegisterSlot::float(*sse_used);
                    *sse_used += 1;
                    BridgeLocation::Register(register)
                }
                _ => unreachable!("unsupported bridge register class"),
            };
            pieces.push(BridgePiecePlan {
                source_index: Some(source_index),
                piece,
                extension,
                indirect: false,
                location,
            });
        }
        return Ok(());
    }

    *stack_size = align_up(*stack_size, boundary_alignment.max(8))?;
    for piece in memory_pieces(classified) {
        let piece_offset = u32::try_from(*stack_size + piece.offset)
            .map_err(|_| AbiError::new("CCC3503", "bridge stack offset overflow"))?;
        pieces.push(BridgePiecePlan {
            source_index: Some(source_index),
            piece,
            extension,
            indirect: false,
            location: BridgeLocation::Stack {
                offset: piece_offset,
            },
        });
    }
    *stack_size = stack_size
        .checked_add(align_up(classified.size, 8)?)
        .ok_or_else(|| AbiError::new("CCC3503", "bridge stack size overflow"))?;
    Ok(())
}

fn bridge_result_pieces(
    classified: &ClassifiedType,
    extension: IntegerExtension,
) -> Vec<BridgePiecePlan> {
    if !matches!(
        classified.passing,
        PassingMode::Scalar | PassingMode::Registers
    ) {
        return Vec::new();
    }
    if classified.classes.first() == Some(&AbiClass::X87) {
        return vec![BridgePiecePlan {
            source_index: None,
            piece: AbiPiece {
                index: 0,
                offset: 0,
                valid_bytes: 10,
                class: AbiClass::X87,
            },
            extension: IntegerExtension::None,
            indirect: false,
            location: BridgeLocation::Register(RegisterSlot::x87()),
        }];
    }
    let mut gp = 0usize;
    let mut sse = 0usize;
    effective_pieces(classified)
        .into_iter()
        .map(|piece| {
            let location = match piece.class {
                AbiClass::Integer => {
                    let location = BridgeLocation::Register(RegisterSlot::integer(gp as u8));
                    gp += 1;
                    location
                }
                AbiClass::Sse | AbiClass::SseUp => {
                    let location = BridgeLocation::Register(RegisterSlot::float(sse as u8));
                    sse += 1;
                    location
                }
                _ => unreachable!("unsupported bridge result class"),
            };
            BridgePiecePlan {
                source_index: None,
                piece,
                extension,
                indirect: false,
                location,
            }
        })
        .collect()
}

fn classify(
    types: &TypeStore,
    ty: TypeId,
    config: &EffectiveCompilationConfig,
    boundary: &str,
) -> Result<ClassifiedType, AbiError> {
    if types.builtin_type(ty) == Some(BuiltinType::Void) {
        return Ok(ClassifiedType {
            ty,
            size: 0,
            align: 1,
            classes: Vec::new(),
            pieces: Vec::new(),
            passing: PassingMode::Void,
        });
    }
    reject_unsupported_recursive(types, ty, config, boundary)?;
    let layout = target_layout(types, ty, config)?;
    if !is_aggregate(types, ty) {
        if types.builtin_type(ty) == Some(BuiltinType::LongDouble) {
            if layout.size != 16
                || layout.align != 16
                || config.target.data_layout.long_double_format != LongDoubleFormat::X87Extended
            {
                return Err(AbiError::new(
                    "CCC3509",
                    format!(
                        "native `long double` {boundary} type `{}` is not the enabled x87 storage format",
                        types.display(ty)
                    ),
                ));
            }
            return Ok(ClassifiedType {
                ty,
                size: layout.size,
                align: layout.align,
                classes: vec![AbiClass::X87, AbiClass::X87Up],
                pieces: vec![AbiPiece {
                    index: 0,
                    offset: 0,
                    valid_bytes: 10,
                    class: AbiClass::X87,
                }],
                passing: PassingMode::Scalar,
            });
        }
        let scalar = boundary_scalar(types, ty, config, boundary)?;
        let class = scalar_class(scalar);
        let valid_bytes = u8::try_from(layout.size)
            .map_err(|_| AbiError::new("CCC3503", "scalar ABI width is too large"))?;
        let pieces = if layout.size == 16 && class == AbiClass::Integer {
            vec![
                AbiPiece {
                    index: 0,
                    offset: 0,
                    valid_bytes: 8,
                    class,
                },
                AbiPiece {
                    index: 1,
                    offset: 8,
                    valid_bytes: 8,
                    class,
                },
            ]
        } else {
            vec![AbiPiece {
                index: 0,
                offset: 0,
                valid_bytes,
                class,
            }]
        };
        return Ok(ClassifiedType {
            ty,
            size: layout.size,
            align: layout.align,
            classes: vec![class; pieces.len()],
            pieces,
            passing: PassingMode::Scalar,
        });
    }
    if layout.size > 16 {
        return Ok(memory_classification(ty, layout.size, layout.align));
    }
    let eightbytes = usize::try_from(layout.size.div_ceil(8))
        .map_err(|_| AbiError::new("CCC3503", "aggregate ABI width is too large"))?;
    let mut classes = vec![AbiClass::NoClass; eightbytes];
    if classify_at(types, ty, 0, config, &mut classes)? {
        return Ok(memory_classification(ty, layout.size, layout.align));
    }
    cleanup_classes(&mut classes);
    if classes.contains(&AbiClass::Memory)
        || classes.iter().any(|class| {
            matches!(
                class,
                AbiClass::X87 | AbiClass::X87Up | AbiClass::ComplexX87
            )
        })
    {
        return Ok(memory_classification(ty, layout.size, layout.align));
    }
    let pieces = classes
        .iter()
        .copied()
        .enumerate()
        .filter(|(_, class)| *class != AbiClass::NoClass)
        .map(|(index, class)| AbiPiece {
            index: index as u8,
            offset: (index * 8) as u64,
            valid_bytes: layout.size.saturating_sub((index * 8) as u64).min(8) as u8,
            class,
        })
        .collect();
    Ok(ClassifiedType {
        ty,
        size: layout.size,
        align: layout.align,
        classes,
        pieces,
        passing: PassingMode::Registers,
    })
}

fn classify_at(
    types: &TypeStore,
    ty: TypeId,
    offset: u64,
    config: &EffectiveCompilationConfig,
    classes: &mut [AbiClass],
) -> Result<bool, AbiError> {
    let layout = target_layout(types, ty, config)?;
    if layout.align > 1 && !offset.is_multiple_of(layout.align) {
        return Ok(true);
    }
    match types
        .try_kind(ty)
        .ok_or_else(|| AbiError::new("CCC3501", format!("unknown type {}", ty.index())))?
    {
        TypeKind::Builtin(builtin) => {
            let class = match builtin {
                BuiltinType::Float16 | BuiltinType::Float | BuiltinType::Double => AbiClass::Sse,
                BuiltinType::LongDouble => AbiClass::X87,
                BuiltinType::Void => AbiClass::NoClass,
                _ => AbiClass::Integer,
            };
            mark_range(classes, offset, layout.size, class);
        }
        TypeKind::Pointer(_) | TypeKind::Enum(_) => {
            mark_range(classes, offset, layout.size, AbiClass::Integer);
        }
        TypeKind::Array(array) => {
            let LayoutShape::Array { length, stride } = layout.shape else {
                unreachable!()
            };
            for index in 0..length {
                if classify_at(
                    types,
                    array.element.ty,
                    offset + index * stride,
                    config,
                    classes,
                )? {
                    return Ok(true);
                }
            }
        }
        TypeKind::Record(id) => {
            let definition = types
                .record(*id)
                .ok_or_else(|| AbiError::new("CCC3502", format!("record {} is unknown", id.0)))?;
            let fields = definition.fields.as_ref().ok_or_else(|| {
                AbiError::new(
                    "CCC3502",
                    format!("type `{}` is incomplete", types.display(ty)),
                )
            })?;
            let LayoutShape::Record(record_layout) = layout.shape else {
                unreachable!()
            };
            for field_layout in &record_layout.fields {
                let field = &fields[field_layout.index];
                if let Some(bitfield) = field_layout.bitfield {
                    if bitfield.width != 0 {
                        // Unlike an ordinary member, a packed bitfield does not
                        // force MEMORY merely because its access unit is not
                        // naturally aligned. Only bytes occupied by the field's
                        // bits contribute INTEGER classes; widening the access
                        // unit must not contaminate an adjacent eightbyte.
                        let leading_bits = u64::from(bitfield.bit_offset);
                        let occupied_offset = bitfield
                            .storage_offset
                            .checked_add(leading_bits / 8)
                            .ok_or_else(|| {
                                AbiError::new("CCC3503", "bitfield ABI offset overflow")
                            })?;
                        let occupied_size = (leading_bits % 8)
                            .checked_add(u64::from(bitfield.width))
                            .ok_or_else(|| AbiError::new("CCC3503", "bitfield ABI width overflow"))?
                            .div_ceil(8);
                        mark_range(
                            classes,
                            offset.checked_add(occupied_offset).ok_or_else(|| {
                                AbiError::new("CCC3503", "bitfield ABI offset overflow")
                            })?,
                            occupied_size,
                            AbiClass::Integer,
                        );
                    }
                    continue;
                }
                let field_offset = match definition.kind {
                    RecordKind::Struct => offset + field_layout.offset,
                    RecordKind::Union => offset,
                };
                if classify_at(types, field.ty.ty, field_offset, config, classes)? {
                    return Ok(true);
                }
            }
        }
        TypeKind::AlignmentAdjusted(adjusted) => {
            if !types
                .builtin_type(adjusted.underlying)
                .is_some_and(BuiltinType::is_integer)
            {
                return Err(AbiError::new(
                    "CCC3508",
                    format!(
                        "alignment-adjusted type `{}` has no scalar ABI representation",
                        types.display(ty)
                    ),
                ));
            }
            let underlying = target_layout(types, adjusted.underlying, config)?;
            if underlying.align > 1 && !offset.is_multiple_of(underlying.align) {
                // The SysV aggregate classifier's unaligned-field rule uses
                // the scalar's natural ABI alignment. A typedef may lower the
                // C object alignment without making a containing aggregate
                // register-passable at that offset.
                return Ok(true);
            }
            mark_range(classes, offset, layout.size, AbiClass::Integer);
        }
        TypeKind::Function(_) => {
            return Err(AbiError::new(
                "CCC3501",
                format!(
                    "function type `{}` must be adjusted to a pointer before ABI planning",
                    types.display(ty)
                ),
            ));
        }
    }
    Ok(false)
}

fn mark_range(classes: &mut [AbiClass], offset: u64, size: u64, class: AbiClass) {
    if size == 0 {
        return;
    }
    let first = (offset / 8) as usize;
    let last = ((offset + size - 1) / 8) as usize;
    for index in first..=last {
        if let Some(slot) = classes.get_mut(index) {
            *slot = merge_class(*slot, class);
        }
    }
}

fn merge_class(left: AbiClass, right: AbiClass) -> AbiClass {
    use AbiClass::*;
    if left == right {
        return left;
    }
    if left == NoClass {
        return right;
    }
    if right == NoClass {
        return left;
    }
    if left == Memory || right == Memory {
        return Memory;
    }
    if left == Integer || right == Integer {
        return Integer;
    }
    if matches!(left, X87 | X87Up | ComplexX87) || matches!(right, X87 | X87Up | ComplexX87) {
        return Memory;
    }
    Sse
}

fn cleanup_classes(classes: &mut [AbiClass]) {
    for index in 0..classes.len() {
        if classes[index] == AbiClass::SseUp
            && (index == 0 || !matches!(classes[index - 1], AbiClass::Sse | AbiClass::SseUp))
        {
            classes[index] = AbiClass::Sse;
        }
        if classes[index] == AbiClass::X87Up && (index == 0 || classes[index - 1] != AbiClass::X87)
        {
            classes.fill(AbiClass::Memory);
            return;
        }
    }
}

fn reject_unsupported_recursive(
    types: &TypeStore,
    ty: TypeId,
    config: &EffectiveCompilationConfig,
    boundary: &str,
) -> Result<(), AbiError> {
    let layout = target_layout(types, ty, config)?;
    if layout.align > 16 {
        return Err(AbiError::new(
            "CCC3513",
            format!(
                "{boundary} type `{}` requires unsupported {}-byte alignment",
                types.display(ty),
                layout.align
            ),
        ));
    }
    match types.try_kind(ty) {
        Some(TypeKind::Builtin(BuiltinType::LongDouble)) => Ok(()),
        Some(TypeKind::Array(array)) => {
            reject_unsupported_recursive(types, array.element.ty, config, boundary)
        }
        Some(TypeKind::Record(id)) => {
            let fields = types
                .record(*id)
                .and_then(|record| record.fields.as_ref())
                .ok_or_else(|| {
                    AbiError::new(
                        "CCC3502",
                        format!("type `{}` is incomplete", types.display(ty)),
                    )
                })?;
            for field in fields {
                reject_unsupported_recursive(types, field.ty.ty, config, boundary)?;
            }
            Ok(())
        }
        Some(TypeKind::Function(_)) => Err(AbiError::new(
            "CCC3501",
            format!(
                "function {boundary} type `{}` must be adjusted to a pointer before ABI planning",
                types.display(ty)
            ),
        )),
        Some(_) => Ok(()),
        None => Err(AbiError::new(
            "CCC3501",
            format!("unknown type {} has no ABI plan", ty.index()),
        )),
    }
}

fn reject_array_boundary(types: &TypeStore, ty: TypeId, boundary: &str) -> Result<(), AbiError> {
    if matches!(types.try_kind(ty), Some(TypeKind::Array(_))) {
        return Err(AbiError::new(
            "CCC3501",
            format!(
                "array {boundary} type `{}` must be adjusted before ABI planning",
                types.display(ty)
            ),
        ));
    }
    Ok(())
}

fn boundary_scalar(
    types: &TypeStore,
    ty: TypeId,
    config: &EffectiveCompilationConfig,
    boundary: &str,
) -> Result<AbiScalar, AbiError> {
    if let Some(builtin) = types.builtin_type(ty) {
        return builtin_scalar(types, ty, builtin, config, boundary);
    }
    match types.try_kind(ty) {
        Some(TypeKind::Pointer(_)) => Ok(AbiScalar::Pointer { bits: 64 }),
        Some(TypeKind::Enum(id)) => {
            let underlying = types
                .enumeration(*id)
                .and_then(|definition| definition.body.as_ref())
                .ok_or_else(|| {
                    AbiError::new(
                        "CCC3502",
                        format!("type `{}` is incomplete", types.display(ty)),
                    )
                })?
                .underlying;
            boundary_scalar(types, underlying, config, boundary)
        }
        _ => Err(AbiError::new(
            "CCC3508",
            format!(
                "aggregate {boundary} type `{}` has no scalar ABI representation",
                types.display(ty)
            ),
        )),
    }
}

fn builtin_scalar(
    types: &TypeStore,
    ty: TypeId,
    builtin: BuiltinType,
    config: &EffectiveCompilationConfig,
    boundary: &str,
) -> Result<AbiScalar, AbiError> {
    match builtin {
        BuiltinType::Void => Err(AbiError::new(
            "CCC3507",
            "`void` has no scalar ABI representation",
        )),
        BuiltinType::LongDouble => Err(AbiError::new(
            "CCC3509",
            format!(
                "native x87 `long double` {boundary} type `{}` requires the explicit System V bridge",
                types.display(ty)
            ),
        )),
        BuiltinType::Float16 => Ok(AbiScalar::Float16),
        BuiltinType::Float => Ok(AbiScalar::Float32),
        BuiltinType::Double => Ok(AbiScalar::Float64),
        builtin if builtin.is_integer() => {
            let layout = target_layout(types, ty, config)?;
            let bits = u8::try_from(layout.size * 8)
                .map_err(|_| AbiError::new("CCC3503", "integer ABI width is too large"))?;
            let signed = match builtin {
                BuiltinType::Char => config.target.data_layout.char_is_signed,
                BuiltinType::SignedChar
                | BuiltinType::Short
                | BuiltinType::Int
                | BuiltinType::Long
                | BuiltinType::LongLong
                | BuiltinType::Int128 => true,
                _ => false,
            };
            Ok(if signed {
                AbiScalar::SignedInteger { bits }
            } else {
                AbiScalar::UnsignedInteger { bits }
            })
        }
        _ => unreachable!(),
    }
}

fn target_layout(
    types: &TypeStore,
    ty: TypeId,
    config: &EffectiveCompilationConfig,
) -> Result<ccc_types::TypeLayout, AbiError> {
    types.layout_of(ty, config).map_err(|error| {
        AbiError::new(
            "CCC3502",
            format!("type `{}` has no target layout: {error}", types.display(ty)),
        )
    })
}

fn is_aggregate(types: &TypeStore, ty: TypeId) -> bool {
    matches!(
        types.try_kind(ty),
        Some(TypeKind::Array(_) | TypeKind::Record(_))
    )
}

fn memory_classification(ty: TypeId, size: u64, align: u64) -> ClassifiedType {
    ClassifiedType {
        ty,
        size,
        align,
        classes: vec![AbiClass::Memory],
        pieces: memory_piece_geometry(size, AbiClass::Memory),
        passing: PassingMode::Memory,
    }
}

fn memory_piece_geometry(size: u64, class: AbiClass) -> Vec<AbiPiece> {
    (0..size.div_ceil(8))
        .map(|index| AbiPiece {
            index: index.min(u64::from(u8::MAX)) as u8,
            offset: index * 8,
            valid_bytes: size.saturating_sub(index * 8).min(8) as u8,
            class,
        })
        .collect()
}

fn effective_pieces(classified: &ClassifiedType) -> Vec<AbiPiece> {
    classified.pieces.clone()
}

fn memory_pieces(classified: &ClassifiedType) -> Vec<AbiPiece> {
    memory_piece_geometry(classified.size, AbiClass::Memory)
}

fn register_counts(classified: &ClassifiedType) -> (u8, u8) {
    let mut gp = 0u8;
    let mut sse = 0u8;
    for piece in effective_pieces(classified) {
        match piece.class {
            AbiClass::Integer => gp += 1,
            AbiClass::Sse | AbiClass::SseUp => sse += 1,
            _ => {}
        }
    }
    (gp, sse)
}

fn consume_scalar_register(scalar: AbiScalar, gp_used: &mut u8, sse_used: &mut u8) {
    match (scalar_class(scalar), scalar_size(scalar)) {
        (AbiClass::Integer, 16) if *gp_used <= 4 => *gp_used += 2,
        (AbiClass::Integer, _) if *gp_used < 6 => *gp_used += 1,
        (AbiClass::Sse, _) if *sse_used < 8 => *sse_used += 1,
        _ => {}
    }
}

fn scalar_class(scalar: AbiScalar) -> AbiClass {
    match scalar {
        AbiScalar::Float16 | AbiScalar::Float32 | AbiScalar::Float64 => AbiClass::Sse,
        _ => AbiClass::Integer,
    }
}

fn scalar_size(scalar: AbiScalar) -> u8 {
    match scalar {
        AbiScalar::SignedInteger { bits }
        | AbiScalar::UnsignedInteger { bits }
        | AbiScalar::Pointer { bits } => bits / 8,
        AbiScalar::Float16 => 2,
        AbiScalar::Float32 => 4,
        AbiScalar::Float64 => 8,
    }
}

fn scalar_carrier(scalar: AbiScalar) -> AbiCarrier {
    match scalar {
        AbiScalar::SignedInteger { bits }
        | AbiScalar::UnsignedInteger { bits }
        | AbiScalar::Pointer { bits } => match bits {
            8 => AbiCarrier::I8,
            16 => AbiCarrier::I16,
            32 => AbiCarrier::I32,
            64 => AbiCarrier::I64,
            128 => AbiCarrier::I128,
            _ => unreachable!("unsupported scalar width"),
        },
        AbiScalar::Float16 => AbiCarrier::F16,
        AbiScalar::Float32 => AbiCarrier::F32,
        AbiScalar::Float64 => AbiCarrier::F64,
    }
}

fn scalar_extension(scalar: AbiScalar) -> IntegerExtension {
    match scalar {
        AbiScalar::SignedInteger { bits } if bits < 32 => IntegerExtension::Signed,
        AbiScalar::UnsignedInteger { bits } if bits < 32 => IntegerExtension::Unsigned,
        _ => IntegerExtension::None,
    }
}

fn piece_carrier(piece: &AbiPiece) -> AbiCarrier {
    match piece.class {
        AbiClass::Integer => AbiCarrier::I64,
        AbiClass::Sse | AbiClass::SseUp if piece.valid_bytes <= 4 => AbiCarrier::F32,
        AbiClass::Sse | AbiClass::SseUp => AbiCarrier::F64,
        _ => unreachable!("unsupported native piece class"),
    }
}

fn align_up(value: u64, align: u64) -> Result<u64, AbiError> {
    if align == 0 || !align.is_power_of_two() {
        return Err(AbiError::new("CCC3503", "invalid ABI alignment"));
    }
    value
        .checked_add(align - 1)
        .map(|value| value & !(align - 1))
        .ok_or_else(|| AbiError::new("CCC3503", "ABI size overflow"))
}

#[cfg(test)]
mod tests {
    use ccc_target::PackingPolicy;
    use ccc_types::{ArrayLength, ArrayType, Field, FunctionType, RecordKind};

    use super::*;

    fn record(types: &mut TypeStore, fields: Vec<Field>) -> TypeId {
        record_with(types, RecordKind::Struct, fields, PackingPolicy::NATIVE)
    }

    fn record_with(
        types: &mut TypeStore,
        kind: RecordKind,
        fields: Vec<Field>,
        packing: PackingPolicy,
    ) -> TypeId {
        let (id, ty) = types.declare_record(kind, None);
        types
            .complete_record_with_packing(id, fields, packing)
            .unwrap();
        ty
    }

    #[test]
    fn classifies_integer_sse_mixed_and_memory_aggregates() {
        let mut types = TypeStore::default();
        let integers = record(
            &mut types,
            vec![
                Field::named("a", TypeId::LONG),
                Field::named("b", TypeId::INT),
            ],
        );
        let mixed = record(
            &mut types,
            vec![
                Field::named("a", TypeId::DOUBLE),
                Field::named("b", TypeId::INT),
            ],
        );
        let large_array = types.array(ArrayType {
            element: TypeId::LONG.into(),
            length: ArrayLength::Constant(3),
        });
        let config = EffectiveCompilationConfig::default();
        assert_eq!(
            classify_type(&types, integers, &config).unwrap().classes,
            [AbiClass::Integer, AbiClass::Integer]
        );
        assert_eq!(
            classify_type(&types, mixed, &config).unwrap().classes,
            [AbiClass::Sse, AbiClass::Integer]
        );
        assert_eq!(
            classify_type(&types, large_array, &config).unwrap().passing,
            PassingMode::Memory
        );
    }

    #[test]
    fn native_plan_rolls_an_entire_aggregate_to_memory() {
        let mut types = TypeStore::default();
        let pair = record(
            &mut types,
            vec![
                Field::named("a", TypeId::LONG),
                Field::named("b", TypeId::LONG),
            ],
        );
        let signature = types.function_type(FunctionType::prototype(
            TypeId::VOID,
            vec![
                TypeId::LONG.into(),
                TypeId::LONG.into(),
                TypeId::LONG.into(),
                TypeId::LONG.into(),
                TypeId::LONG.into(),
                pair.into(),
            ],
        ));
        let plan =
            plan_function_type(&types, signature, &EffectiveCompilationConfig::default()).unwrap();
        let aggregate = &plan.parameters[5];
        assert_eq!(aggregate.classified.passing, PassingMode::Memory);
        assert!(matches!(
            plan.clif_parameters[aggregate.carrier_indices[0] as usize].purpose,
            NativePurpose::StructArgument(16)
        ));
    }

    #[test]
    fn indirect_result_consumes_the_first_gp_argument_position() {
        let mut types = TypeStore::default();
        let result = record(
            &mut types,
            vec![
                Field::named("a", TypeId::LONG),
                Field::named("b", TypeId::LONG),
                Field::named("c", TypeId::LONG),
            ],
        );
        let signature =
            types.function_type(FunctionType::prototype(result, vec![TypeId::LONG.into()]));
        let plan =
            plan_function_type(&types, signature, &EffectiveCompilationConfig::default()).unwrap();
        assert!(matches!(plan.result, NativeResultPlan::Indirect { .. }));
        assert_eq!(plan.clif_parameters[0].purpose, NativePurpose::StructReturn);
        assert_eq!(plan.clif_parameters[1].source_index, Some(0));
    }

    #[test]
    fn va_arg_plan_counts_mixed_register_files() {
        let mut types = TypeStore::default();
        let mixed = record(
            &mut types,
            vec![
                Field::named("x", TypeId::DOUBLE),
                Field::named("i", TypeId::LONG),
            ],
        );
        let plan = plan_va_arg(&types, mixed, &EffectiveCompilationConfig::default()).unwrap();
        assert_eq!((plan.gp_slots, plan.sse_slots), (1, 1));
        assert_eq!(plan.overflow_size, 16);
    }

    #[test]
    fn named_aggregate_sizes_have_exact_eightbyte_geometry() {
        let mut types = TypeStore::default();
        let one = record(&mut types, vec![Field::named("x", TypeId::CHAR)]);
        let eight = record(&mut types, vec![Field::named("x", TypeId::LONG)]);
        let nine = record_with(
            &mut types,
            RecordKind::Struct,
            vec![
                Field::named("x", TypeId::LONG),
                Field::named("y", TypeId::CHAR),
            ],
            PackingPolicy::PACKED,
        );
        let sixteen = record(
            &mut types,
            vec![
                Field::named("x", TypeId::LONG),
                Field::named("y", TypeId::LONG),
            ],
        );
        let seventeen = types.array(ArrayType {
            element: TypeId::CHAR.into(),
            length: ArrayLength::Constant(17),
        });
        let config = EffectiveCompilationConfig::default();
        let cases = [
            (one, 1, PassingMode::Registers, vec![1]),
            (eight, 8, PassingMode::Registers, vec![8]),
            (nine, 9, PassingMode::Registers, vec![8, 1]),
            (sixteen, 16, PassingMode::Registers, vec![8, 8]),
            (seventeen, 17, PassingMode::Memory, vec![8, 8, 1]),
        ];
        for (ty, size, passing, valid_bytes) in cases {
            let classified = classify_type(&types, ty, &config).unwrap();
            assert_eq!(classified.size, size);
            assert_eq!(classified.passing, passing);
            assert_eq!(
                classified
                    .pieces
                    .iter()
                    .map(|piece| piece.valid_bytes)
                    .collect::<Vec<_>>(),
                valid_bytes
            );
        }
    }

    #[test]
    fn nested_arrays_unions_and_merge_rules_use_canonical_layouts() {
        let mut types = TypeStore::default();
        let doubles = types.array(ArrayType {
            element: TypeId::DOUBLE.into(),
            length: ArrayLength::Constant(2),
        });
        let merged_union = record_with(
            &mut types,
            RecordKind::Union,
            vec![
                Field::named("integer", TypeId::LONG),
                Field::named("floating", TypeId::DOUBLE),
            ],
            PackingPolicy::NATIVE,
        );
        let nested = record(
            &mut types,
            vec![
                Field::named("merged", merged_union),
                Field::named("tail", TypeId::DOUBLE),
            ],
        );
        let config = EffectiveCompilationConfig::default();
        assert_eq!(
            classify_type(&types, doubles, &config).unwrap().classes,
            [AbiClass::Sse, AbiClass::Sse]
        );
        assert_eq!(
            classify_type(&types, merged_union, &config)
                .unwrap()
                .classes,
            [AbiClass::Integer]
        );
        assert_eq!(
            classify_type(&types, nested, &config).unwrap().classes,
            [AbiClass::Integer, AbiClass::Sse]
        );
        assert_eq!(merge_class(AbiClass::NoClass, AbiClass::Sse), AbiClass::Sse);
        assert_eq!(
            merge_class(AbiClass::Integer, AbiClass::Sse),
            AbiClass::Integer
        );
        assert_eq!(
            merge_class(AbiClass::Memory, AbiClass::Sse),
            AbiClass::Memory
        );
        assert_eq!(merge_class(AbiClass::X87, AbiClass::Sse), AbiClass::Memory);
        assert_eq!(merge_class(AbiClass::Sse, AbiClass::SseUp), AbiClass::Sse);
    }

    #[test]
    fn packed_unaligned_members_fall_back_but_crossing_bitfields_use_integer_classes() {
        let mut types = TypeStore::default();
        let packed = record_with(
            &mut types,
            RecordKind::Struct,
            vec![
                Field::named("prefix", TypeId::CHAR),
                Field::named("value", TypeId::INT),
            ],
            PackingPolicy::PACKED,
        );
        let prefix = types.array(ArrayType {
            element: TypeId::CHAR.into(),
            length: ArrayLength::Constant(7),
        });
        let bitfield = record_with(
            &mut types,
            RecordKind::Struct,
            vec![
                Field::named("prefix", prefix),
                Field::bitfield(Some("bits".to_owned()), TypeId::UNSIGNED_LONG, 16),
            ],
            PackingPolicy::PACKED,
        );
        let config = EffectiveCompilationConfig::default();
        assert_eq!(
            classify_type(&types, packed, &config).unwrap().passing,
            PassingMode::Memory
        );
        let layout = types.layout_of(bitfield, &config).unwrap();
        let LayoutShape::Record(layout) = layout.shape else {
            panic!("bitfield case must be a record")
        };
        let storage = layout.fields[1].bitfield.unwrap();
        assert_eq!(storage.storage_offset, 7);
        assert!(
            storage.storage_offset / 8 != (storage.storage_offset + storage.storage_size - 1) / 8
        );
        assert_eq!(
            classify_type(&types, bitfield, &config).unwrap(),
            ClassifiedType {
                ty: bitfield,
                size: 9,
                align: 1,
                classes: vec![AbiClass::Integer, AbiClass::Integer],
                pieces: vec![
                    AbiPiece {
                        index: 0,
                        offset: 0,
                        valid_bytes: 8,
                        class: AbiClass::Integer,
                    },
                    AbiPiece {
                        index: 1,
                        offset: 8,
                        valid_bytes: 1,
                        class: AbiClass::Integer,
                    },
                ],
                passing: PassingMode::Registers,
            }
        );

        let prefix = types.array(ArrayType {
            element: TypeId::CHAR.into(),
            length: ArrayLength::Constant(5),
        });
        let widened_access = record_with(
            &mut types,
            RecordKind::Struct,
            vec![
                Field::named("prefix", prefix),
                Field::bitfield(Some("bits".to_owned()), TypeId::UNSIGNED_LONG, 24),
                Field::named("tail", TypeId::DOUBLE),
            ],
            PackingPolicy::PACKED,
        );
        let layout = types.layout_of(widened_access, &config).unwrap();
        let LayoutShape::Record(layout) = layout.shape else {
            panic!("widened-access case must be a record")
        };
        let storage = layout.fields[1].bitfield.unwrap();
        assert_eq!((storage.storage_offset, storage.storage_size), (5, 4));
        assert_eq!(
            classify_type(&types, widened_access, &config)
                .unwrap()
                .classes,
            [AbiClass::Integer, AbiClass::Sse]
        );
    }

    #[test]
    fn independent_gp_and_sse_exhaustion_roll_back_whole_aggregates() {
        let mut types = TypeStore::default();
        let integer_pair = record(
            &mut types,
            vec![
                Field::named("a", TypeId::LONG),
                Field::named("b", TypeId::LONG),
            ],
        );
        let sse_pair = record(
            &mut types,
            vec![
                Field::named("a", TypeId::DOUBLE),
                Field::named("b", TypeId::DOUBLE),
            ],
        );
        let mut gp_parameters = vec![TypeId::LONG.into(); 5];
        gp_parameters.push(integer_pair.into());
        gp_parameters.push(TypeId::LONG.into());
        let gp_signature =
            types.function_type(FunctionType::prototype(TypeId::VOID, gp_parameters));
        let mut sse_parameters = vec![TypeId::DOUBLE.into(); 7];
        sse_parameters.push(sse_pair.into());
        sse_parameters.push(TypeId::DOUBLE.into());
        let sse_signature =
            types.function_type(FunctionType::prototype(TypeId::VOID, sse_parameters));
        let config = EffectiveCompilationConfig::default();
        let gp = plan_function_type(&types, gp_signature, &config).unwrap();
        assert_eq!(gp.parameters[5].classified.passing, PassingMode::Memory);
        assert_eq!(gp.parameters[6].classified.passing, PassingMode::Scalar);
        let sse = plan_function_type(&types, sse_signature, &config).unwrap();
        assert_eq!(sse.parameters[7].classified.passing, PassingMode::Memory);
        assert_eq!(sse.parameters[8].classified.passing, PassingMode::Scalar);
    }

    #[test]
    fn variadic_stack_prefix_and_sse_count_are_exact() {
        let mut types = TypeStore::default();
        let fixed = vec![TypeId::LONG.into(); 7];
        let signature = types.function_type(FunctionType::variadic(TypeId::VOID, fixed));
        let actual = vec![TypeId::LONG; 8];
        let config = EffectiveCompilationConfig::default();
        let plan = plan_variadic_call(&types, signature, &actual, 7, &config).unwrap();
        assert_eq!(plan.overflow_arg_offset, 8);
        assert_eq!(plan.stack_size, 16);
        assert!(plan.parameter_pieces.iter().any(|piece| {
            piece.source_index == Some(6) && piece.location == BridgeLocation::Stack { offset: 0 }
        }));
        assert!(plan.parameter_pieces.iter().any(|piece| {
            piece.source_index == Some(7) && piece.location == BridgeLocation::Stack { offset: 8 }
        }));

        let floating_signature =
            types.function_type(FunctionType::variadic(TypeId::VOID, Vec::new()));
        for (count, expected) in [(0, 0), (1, 1), (8, 8), (9, 8)] {
            let actual = vec![TypeId::DOUBLE; count];
            let plan = plan_variadic_call(&types, floating_signature, &actual, 0, &config).unwrap();
            assert_eq!(plan.variadic_sse_count, expected);
        }
    }

    #[test]
    fn unprototyped_calls_use_the_promoted_actual_signature_and_variadic_register_count() {
        let mut types = TypeStore::default();
        let signature = types.function_type(FunctionType::unspecified(TypeId::INT));
        let actual = [TypeId::INT, TypeId::DOUBLE, TypeId::INT];
        let config = EffectiveCompilationConfig::default();
        let plan = plan_unprototyped_call(&types, signature, &actual, &config).unwrap();

        assert_eq!(plan.kind, BridgeKind::UnprototypedCall);
        assert_eq!(plan.parameters.len(), actual.len());
        assert_eq!(
            plan.parameters
                .iter()
                .map(|parameter| parameter.ty)
                .collect::<Vec<_>>(),
            actual
        );
        assert_eq!((plan.gp_used, plan.xmm_used), (2, 1));
        assert_eq!(plan.variadic_sse_count, 1);
        assert_eq!(plan.overflow_arg_offset, 0);
        assert_eq!(plan.stack_size, 0);
        assert!(plan.parameter_pieces.iter().any(|piece| {
            piece.source_index == Some(1)
                && piece.location == BridgeLocation::Register(RegisterSlot::float(0))
        }));
    }

    #[test]
    fn exact_target_profile_rejects_other_sysv_elf_environments() {
        let types = TypeStore::default();
        let mut non_linux = EffectiveCompilationConfig::default();
        non_linux.target.triple.operating_system = OperatingSystem::Freebsd;
        non_linux.target.triple.environment = Environment::Unknown;
        assert_eq!(
            classify_type(&types, TypeId::INT, &non_linux)
                .unwrap_err()
                .code,
            "CCC3504"
        );

        let mut non_gnu = EffectiveCompilationConfig::default();
        non_gnu.target.triple.environment = Environment::Musl;
        assert_eq!(
            classify_type(&types, TypeId::INT, &non_gnu)
                .unwrap_err()
                .code,
            "CCC3504"
        );
    }

    #[test]
    fn wide_scalar_has_two_integer_eightbytes_and_va_arg_alignment() {
        let types = TypeStore::default();
        let config = EffectiveCompilationConfig::default();
        let classified = classify_type(&types, TypeId::UNSIGNED_INT128, &config).unwrap();
        assert_eq!(classified.size, 16);
        assert_eq!(classified.align, 16);
        assert_eq!(classified.passing, PassingMode::Scalar);
        assert_eq!(classified.classes, [AbiClass::Integer, AbiClass::Integer]);
        assert_eq!(
            classified
                .pieces
                .iter()
                .map(|piece| (piece.index, piece.offset, piece.valid_bytes, piece.class))
                .collect::<Vec<_>>(),
            [(0, 0, 8, AbiClass::Integer), (1, 8, 8, AbiClass::Integer),]
        );

        let va_arg = plan_va_arg(&types, TypeId::UNSIGNED_INT128, &config).unwrap();
        assert_eq!((va_arg.gp_slots, va_arg.sse_slots), (2, 0));
        assert_eq!((va_arg.overflow_size, va_arg.overflow_align), (16, 16));
    }

    #[test]
    fn x87_scalars_use_aligned_stack_arguments_and_st0_results() {
        let mut types = TypeStore::default();
        let config = EffectiveCompilationConfig::default();
        let classified = classify_type(&types, TypeId::LONG_DOUBLE, &config).unwrap();
        assert_eq!(classified.size, 16);
        assert_eq!(classified.align, 16);
        assert_eq!(classified.passing, PassingMode::Scalar);
        assert_eq!(classified.classes, [AbiClass::X87, AbiClass::X87Up]);
        assert_eq!(
            classified.pieces,
            [AbiPiece {
                index: 0,
                offset: 0,
                valid_bytes: 10,
                class: AbiClass::X87,
            }]
        );

        let signature = types.function_type(FunctionType::prototype(
            TypeId::LONG_DOUBLE,
            vec![TypeId::LONG_DOUBLE.into()],
        ));
        let BoundaryPlan::Bridge(entry) = plan_boundary_type(&types, signature, &config).unwrap()
        else {
            panic!("x87 definition must use a generated bridge")
        };
        assert_eq!(entry.kind, BridgeKind::FixedEntry);
        assert_eq!(entry.stack_size, 16);
        assert_eq!((entry.gp_used, entry.xmm_used), (0, 0));
        assert_eq!(
            entry
                .parameter_pieces
                .iter()
                .map(|piece| (piece.piece.offset, piece.location))
                .collect::<Vec<_>>(),
            [
                (0, BridgeLocation::Stack { offset: 0 }),
                (8, BridgeLocation::Stack { offset: 8 }),
            ]
        );
        assert_eq!(entry.result_pieces.len(), 1);
        assert_eq!(
            entry.result_pieces[0].location,
            BridgeLocation::Register(RegisterSlot::x87())
        );
        assert!(!entry.hidden_return);

        let call = plan_fixed_call(&types, signature, &[TypeId::LONG_DOUBLE], &config).unwrap();
        assert_eq!(call.kind, BridgeKind::FixedCall);
        assert_eq!(call.stack_size, 16);

        let va_arg = plan_va_arg(&types, TypeId::LONG_DOUBLE, &config).unwrap();
        assert_eq!((va_arg.gp_slots, va_arg.sse_slots), (0, 0));
        assert_eq!((va_arg.overflow_align, va_arg.overflow_size), (16, 16));
    }

    #[test]
    fn aggregates_containing_x87_values_remain_memory_classified() {
        let mut types = TypeStore::default();
        let wrapper = record(&mut types, vec![Field::named("value", TypeId::LONG_DOUBLE)]);
        let config = EffectiveCompilationConfig::default();
        let classified = classify_type(&types, wrapper, &config).unwrap();
        assert_eq!(classified.passing, PassingMode::Memory);
        assert_eq!((classified.size, classified.align), (16, 16));

        let signature = types.function_type(FunctionType::prototype(wrapper, vec![wrapper.into()]));
        let BoundaryPlan::Bridge(plan) = plan_boundary_type(&types, signature, &config).unwrap()
        else {
            panic!("an x87-containing aggregate must use a generated bridge")
        };
        assert!(plan.hidden_return);
        assert_eq!(plan.gp_used, 1);
        assert_eq!(plan.stack_size, 16);
    }

    #[test]
    fn wide_fixed_bridge_rolls_back_the_pair_and_reuses_the_stranded_gp_register() {
        let mut types = TypeStore::default();
        let mut parameters = vec![TypeId::LONG.into(); 5];
        parameters.push(TypeId::UNSIGNED_INT128.into());
        parameters.push(TypeId::LONG.into());
        let signature =
            types.function_type(FunctionType::prototype(TypeId::UNSIGNED_INT128, parameters));
        let config = EffectiveCompilationConfig::default();
        let BoundaryPlan::Bridge(entry) = plan_boundary_type(&types, signature, &config).unwrap()
        else {
            panic!("wide fixed definition did not select the explicit bridge");
        };
        assert_eq!(entry.kind, BridgeKind::FixedEntry);
        assert_eq!(entry.result.classes, [AbiClass::Integer, AbiClass::Integer]);
        assert_eq!(entry.stack_size, 16);
        assert!(entry.parameter_pieces.iter().any(|piece| {
            piece.source_index == Some(5)
                && piece.piece.index == 0
                && piece.location == BridgeLocation::Stack { offset: 0 }
        }));
        assert!(entry.parameter_pieces.iter().any(|piece| {
            piece.source_index == Some(5)
                && piece.piece.index == 1
                && piece.location == BridgeLocation::Stack { offset: 8 }
        }));
        assert!(entry.parameter_pieces.iter().any(|piece| {
            piece.source_index == Some(6)
                && piece.location == BridgeLocation::Register(RegisterSlot::integer(5))
        }));

        let actual = [
            TypeId::LONG,
            TypeId::LONG,
            TypeId::LONG,
            TypeId::LONG,
            TypeId::LONG,
            TypeId::UNSIGNED_INT128,
            TypeId::LONG,
        ];
        let call = plan_fixed_call(&types, signature, &actual, &config).unwrap();
        assert_eq!(call.kind, BridgeKind::FixedCall);
        assert_eq!(call.parameter_pieces, entry.parameter_pieces);
        assert_eq!(call.result_pieces, entry.result_pieces);
    }
}
