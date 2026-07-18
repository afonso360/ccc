//! AAPCS64 and Darwin arm64 fixed-boundary classification.

use ccc_target::{AbiIdentity, EffectiveCompilationConfig};
use ccc_types::{
    ArrayLength, BuiltinType, FunctionParameters, FunctionType, LayoutShape, RecordKind, TypeId,
    TypeKind, TypeStore,
};

use crate::{AbiError, boundary_value_alignment, model::*};

const MAX_ARGUMENT_REGISTERS: u8 = 8;
const MAX_RESULT_REGISTERS: usize = 8;

pub(crate) fn validate_target(config: &EffectiveCompilationConfig) -> Result<(), AbiError> {
    if !matches!(
        config.target.abi,
        AbiIdentity::Aapcs64Lp64 | AbiIdentity::DarwinArm64
    ) {
        return Err(AbiError::new(
            "CCC3504",
            format!(
                "target `{}` does not use an enabled arm64 ABI identity",
                config.target.triple
            ),
        ));
    }
    let layout = config.target.data_layout;
    if layout.pointer_width != 64
        || layout.int_width != 32
        || layout.long_width != 64
        || layout.double_width != 64
    {
        return Err(AbiError::new(
            "CCC3504",
            format!(
                "target `{}` does not satisfy the enabled arm64 LP64 data model",
                config.target.triple
            ),
        ));
    }
    Ok(())
}

pub(crate) fn plan_function_type(
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
    plan_native_signature(types, &signature, config)
}

pub(crate) fn plan_boundary_type(
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
        return Ok(BoundaryPlan::Bridge(plan_bridge(
            types,
            &signature,
            parameters,
            &actual,
            parameters.len(),
            BridgeKind::VariadicEntry,
            config,
        )?));
    }
    Ok(BoundaryPlan::Native(plan_native_signature(
        types, &signature, config,
    )?))
}

pub(crate) fn plan_variadic_call(
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

pub(crate) fn plan_unprototyped_call(
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

pub(crate) fn classify_type(
    types: &TypeStore,
    ty: TypeId,
    config: &EffectiveCompilationConfig,
) -> Result<ClassifiedType, AbiError> {
    validate_target(config)?;
    classify(types, ty, config, "boundary")
}

pub(crate) fn plan_va_arg(
    types: &TypeStore,
    ty: TypeId,
    config: &EffectiveCompilationConfig,
) -> Result<VaArgPlan, AbiError> {
    validate_target(config)?;
    let classified = classify(types, ty, config, "va_arg")?;
    if classified.passing == PassingMode::Void {
        return Err(AbiError::new("CCC3514", "`va_arg` cannot request `void`"));
    }
    let indirect = classified.passing == PassingMode::Memory;
    let gp_slots = if indirect {
        1
    } else {
        classified
            .pieces
            .iter()
            .filter(|piece| piece.class == AbiClass::Integer)
            .count() as u8
    };
    let sse_slots = if indirect || config.target.abi == AbiIdentity::DarwinArm64 {
        0
    } else {
        classified
            .pieces
            .iter()
            .filter(|piece| piece.class == AbiClass::Sse)
            .count() as u8
    };
    let overflow_align = if indirect {
        8
    } else {
        boundary_value_alignment(types, &classified, config)?.clamp(8, 16)
    };
    let payload_size = if indirect { 8 } else { classified.size };
    let overflow_size = align_up(payload_size, 8)?;
    Ok(VaArgPlan {
        result_size: classified.size,
        result_align: classified.align,
        overflow_size,
        overflow_align,
        classified,
        gp_slots,
        sse_slots,
        indirect,
    })
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
    let mut clif_parameters = Vec::new();
    let indirect_result = result_classified.passing == PassingMode::Memory;
    if indirect_result {
        push_carrier(
            &mut clif_parameters,
            None,
            None,
            0,
            8,
            AbiClass::Integer,
            AbiCarrier::I64,
            IntegerExtension::None,
            NativePurpose::StructReturn,
        )?;
    }

    let mut gp_used = 0u8;
    let mut fp_used = 0u8;
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
                if class == AbiClass::Integer {
                    gp_used = gp_used.saturating_add(1).min(MAX_ARGUMENT_REGISTERS);
                } else {
                    fp_used = fp_used.saturating_add(1).min(MAX_ARGUMENT_REGISTERS);
                }
                carrier_indices.push(push_carrier(
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
            PassingMode::Registers if is_homogeneous(&classified) => {
                let pieces = classified.pieces.len() as u8;
                if fp_used.saturating_add(pieces) > MAX_ARGUMENT_REGISTERS {
                    while fp_used < MAX_ARGUMENT_REGISTERS {
                        push_carrier(
                            &mut clif_parameters,
                            None,
                            None,
                            0,
                            8,
                            AbiClass::Sse,
                            AbiCarrier::F64,
                            IntegerExtension::None,
                            NativePurpose::Padding,
                        )?;
                        fp_used += 1;
                    }
                    classified = homogeneous_stack_classification(&classified, config.target.abi);
                    for piece in &classified.pieces {
                        carrier_indices.push(push_carrier(
                            &mut clif_parameters,
                            Some(source_index),
                            Some(piece.index),
                            piece.offset,
                            piece.valid_bytes,
                            piece.class,
                            if config.target.abi == AbiIdentity::DarwinArm64 {
                                float_piece_carrier(piece)?
                            } else {
                                vector_piece_carrier(piece)?
                            },
                            IntegerExtension::None,
                            NativePurpose::Normal,
                        )?);
                    }
                } else {
                    fp_used += pieces;
                    for piece in &classified.pieces {
                        carrier_indices.push(push_carrier(
                            &mut clif_parameters,
                            Some(source_index),
                            Some(piece.index),
                            piece.offset,
                            piece.valid_bytes,
                            piece.class,
                            float_piece_carrier(piece)?,
                            IntegerExtension::None,
                            NativePurpose::Normal,
                        )?);
                    }
                }
            }
            PassingMode::Registers => {
                if config.target.abi == AbiIdentity::Aapcs64Lp64
                    && classified.align >= 16
                    && gp_used < MAX_ARGUMENT_REGISTERS
                    && !gp_used.is_multiple_of(2)
                {
                    push_carrier(
                        &mut clif_parameters,
                        None,
                        None,
                        0,
                        8,
                        AbiClass::Integer,
                        AbiCarrier::I64,
                        IntegerExtension::None,
                        NativePurpose::Padding,
                    )?;
                    gp_used += 1;
                }
                let pieces = classified.pieces.len() as u8;
                if gp_used.saturating_add(pieces) > MAX_ARGUMENT_REGISTERS {
                    while gp_used < MAX_ARGUMENT_REGISTERS {
                        push_carrier(
                            &mut clif_parameters,
                            None,
                            None,
                            0,
                            8,
                            AbiClass::Integer,
                            AbiCarrier::I64,
                            IntegerExtension::None,
                            NativePurpose::Padding,
                        )?;
                        gp_used += 1;
                    }
                    if classified.align > 8 {
                        classified = aligned_stack_classification(&classified)?;
                    }
                } else {
                    gp_used += pieces;
                }
                for piece in &classified.pieces {
                    carrier_indices.push(push_carrier(
                        &mut clif_parameters,
                        Some(source_index),
                        Some(piece.index),
                        piece.offset,
                        piece.valid_bytes,
                        AbiClass::Integer,
                        if piece.valid_bytes == 16 {
                            AbiCarrier::I128
                        } else {
                            AbiCarrier::I64
                        },
                        IntegerExtension::None,
                        NativePurpose::Normal,
                    )?);
                }
            }
            PassingMode::Memory => {
                gp_used = gp_used.saturating_add(1).min(MAX_ARGUMENT_REGISTERS);
                carrier_indices.push(push_carrier(
                    &mut clif_parameters,
                    Some(source_index),
                    None,
                    0,
                    8,
                    AbiClass::Memory,
                    AbiCarrier::I64,
                    IntegerExtension::None,
                    NativePurpose::IndirectArgument,
                )?);
            }
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
            let carrier_index = push_carrier(
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
            if result_classified.pieces.len() > MAX_RESULT_REGISTERS {
                return Err(AbiError::new(
                    "CCC3503",
                    "arm64 aggregate result exceeds the register result limit",
                ));
            }
            let homogeneous = is_homogeneous(&result_classified);
            let mut carrier_indices = Vec::new();
            for piece in &result_classified.pieces {
                carrier_indices.push(push_carrier(
                    &mut clif_results,
                    None,
                    Some(piece.index),
                    piece.offset,
                    piece.valid_bytes,
                    piece.class,
                    if homogeneous {
                        float_piece_carrier(piece)?
                    } else {
                        AbiCarrier::I64
                    },
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
        calling_convention: config.target.abi.calling_convention(),
        parameters: planned_parameters,
        result,
        clif_parameters,
        clif_results,
        variadic: false,
    })
}

#[allow(clippy::too_many_arguments)]
fn push_carrier(
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
        .map_err(|_| AbiError::new("CCC3503", "ABI carrier count overflow"))?;
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
    let mut gp_used = 0u8;
    let mut fp_used = 0u8;
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
        let extension = if classified.passing == PassingMode::Scalar {
            scalar_extension(boundary_scalar(types, ty, config, "parameter")?)
        } else {
            IntegerExtension::None
        };
        allocate_bridge_argument(
            &classified,
            boundary_value_alignment(types, &classified, config)?,
            source_index as u32,
            extension,
            source_index >= variadic_boundary,
            config.target.abi,
            &mut gp_used,
            &mut fp_used,
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
        .map_err(|_| AbiError::new("CCC3503", "arm64 bridge stack payload is too large"))?;
    let result_extension = if result.passing == PassingMode::Scalar {
        scalar_extension(boundary_scalar(
            types,
            signature.result.ty,
            config,
            "return",
        )?)
    } else {
        IntegerExtension::None
    };
    Ok(BridgeBoundaryPlan {
        abi_identity: config.target.abi,
        calling_convention: config.target.abi.calling_convention(),
        kind,
        parameters,
        parameter_pieces,
        result_pieces: bridge_result_pieces(&result, result_extension),
        result,
        hidden_return,
        overflow_arg_offset: u32::try_from(fixed_stack_end)
            .map_err(|_| AbiError::new("CCC3503", "arm64 variadic overflow offset is too large"))?,
        stack_size,
        gp_used,
        xmm_used: fp_used,
        variadic_sse_count: 0,
    })
}

#[allow(clippy::too_many_arguments)]
fn allocate_bridge_argument(
    classified: &ClassifiedType,
    boundary_alignment: u64,
    source_index: u32,
    extension: IntegerExtension,
    unnamed: bool,
    abi: AbiIdentity,
    gp_used: &mut u8,
    fp_used: &mut u8,
    stack_size: &mut u64,
    pieces: &mut Vec<BridgePiecePlan>,
) -> Result<(), AbiError> {
    let darwin_stack_only = abi == AbiIdentity::DarwinArm64 && unnamed;
    if classified.passing == PassingMode::Memory {
        let pointer_piece = AbiPiece {
            index: 0,
            offset: 0,
            valid_bytes: 8,
            class: AbiClass::Integer,
        };
        let location = if !darwin_stack_only && *gp_used < MAX_ARGUMENT_REGISTERS {
            let location = BridgeLocation::Register(RegisterSlot::integer(*gp_used));
            *gp_used += 1;
            location
        } else {
            stack_location(stack_size, 8, 8)?
        };
        pieces.push(BridgePiecePlan {
            source_index: Some(source_index),
            piece: pointer_piece,
            extension: IntegerExtension::None,
            indirect: true,
            location,
        });
        return Ok(());
    }

    if darwin_stack_only {
        return allocate_whole_on_stack(
            classified,
            boundary_alignment,
            source_index,
            extension,
            abi,
            true,
            stack_size,
            pieces,
        );
    }

    if is_homogeneous(classified)
        || classified.passing == PassingMode::Scalar && classified.pieces[0].class == AbiClass::Sse
    {
        let needed = classified.pieces.len() as u8;
        if fp_used.saturating_add(needed) <= MAX_ARGUMENT_REGISTERS {
            for piece in &classified.pieces {
                pieces.push(BridgePiecePlan {
                    source_index: Some(source_index),
                    piece: piece.clone(),
                    extension,
                    indirect: false,
                    location: BridgeLocation::Register(RegisterSlot::float(*fp_used)),
                });
                *fp_used += 1;
            }
            return Ok(());
        }
        *fp_used = MAX_ARGUMENT_REGISTERS;
        return allocate_whole_on_stack(
            classified,
            boundary_alignment,
            source_index,
            extension,
            abi,
            false,
            stack_size,
            pieces,
        );
    }

    if abi == AbiIdentity::Aapcs64Lp64
        && boundary_alignment >= 16
        && *gp_used < MAX_ARGUMENT_REGISTERS
        && !gp_used.is_multiple_of(2)
    {
        *gp_used += 1;
    }
    if (*gp_used as usize + classified.pieces.len()) <= usize::from(MAX_ARGUMENT_REGISTERS) {
        for piece in &classified.pieces {
            pieces.push(BridgePiecePlan {
                source_index: Some(source_index),
                piece: piece.clone(),
                extension,
                indirect: false,
                location: BridgeLocation::Register(RegisterSlot::integer(*gp_used)),
            });
            *gp_used += 1;
        }
        return Ok(());
    }

    // AAPCS64 C.13-C.15 exhaust NGRN and place the complete composite at the
    // naturally aligned NSAA. AAPCS32's register/stack split does not apply.
    *gp_used = MAX_ARGUMENT_REGISTERS;
    allocate_whole_on_stack(
        classified,
        boundary_alignment,
        source_index,
        extension,
        abi,
        false,
        stack_size,
        pieces,
    )
}

#[allow(clippy::too_many_arguments)]
fn allocate_whole_on_stack(
    classified: &ClassifiedType,
    boundary_alignment: u64,
    source_index: u32,
    extension: IntegerExtension,
    abi: AbiIdentity,
    darwin_unnamed: bool,
    stack_size: &mut u64,
    pieces: &mut Vec<BridgePiecePlan>,
) -> Result<(), AbiError> {
    let alignment = if abi == AbiIdentity::DarwinArm64 && darwin_unnamed {
        boundary_alignment.clamp(8, 16)
    } else {
        boundary_alignment.clamp(1, 16)
    };
    *stack_size = align_up(*stack_size, alignment)?;
    let base = *stack_size;
    for piece in &classified.pieces {
        pieces.push(BridgePiecePlan {
            source_index: Some(source_index),
            piece: piece.clone(),
            extension,
            indirect: false,
            location: BridgeLocation::Stack {
                offset: u32::try_from(base + piece.offset)
                    .map_err(|_| AbiError::new("CCC3503", "arm64 bridge stack offset overflow"))?,
            },
        });
    }
    *stack_size = if abi == AbiIdentity::DarwinArm64 {
        if darwin_unnamed {
            align_up(base + classified.size, 8)?
        } else {
            align_up(base + classified.size, alignment)?
        }
    } else {
        align_up(base + classified.size, 8)?
    };
    Ok(())
}

fn stack_location(stack_size: &mut u64, size: u64, align: u64) -> Result<BridgeLocation, AbiError> {
    *stack_size = align_up(*stack_size, align)?;
    let offset = u32::try_from(*stack_size)
        .map_err(|_| AbiError::new("CCC3503", "arm64 bridge stack offset overflow"))?;
    *stack_size = align_up(*stack_size + size, 8)?;
    Ok(BridgeLocation::Stack { offset })
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
    let mut gp = 0u8;
    let mut fp = 0u8;
    classified
        .pieces
        .iter()
        .map(|piece| {
            let register = if piece.class == AbiClass::Sse {
                let register = RegisterSlot::float(fp);
                fp += 1;
                register
            } else {
                let register = RegisterSlot::integer(gp);
                gp += 1;
                register
            };
            BridgePiecePlan {
                source_index: None,
                piece: piece.clone(),
                extension,
                indirect: false,
                location: BridgeLocation::Register(register),
            }
        })
        .collect()
}

fn align_up(value: u64, align: u64) -> Result<u64, AbiError> {
    debug_assert!(align != 0 && align.is_power_of_two());
    value
        .checked_add(align - 1)
        .map(|value| value & !(align - 1))
        .ok_or_else(|| AbiError::new("CCC3503", "arm64 ABI size overflow"))
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
    reject_binary128_recursive(types, ty, config, boundary)?;
    let layout = types.layout_of(ty, config).map_err(|error| {
        AbiError::new(
            "CCC3502",
            format!("type `{}` has no ABI layout: {error}", types.display(ty)),
        )
    })?;
    if !is_aggregate(types, ty) {
        let scalar = boundary_scalar(types, ty, config, boundary)?;
        let class = scalar_class(scalar);
        return Ok(ClassifiedType {
            ty,
            size: layout.size,
            align: layout.align,
            classes: vec![class],
            pieces: vec![AbiPiece {
                index: 0,
                offset: 0,
                valid_bytes: scalar_size(scalar),
                class,
            }],
            passing: PassingMode::Scalar,
        });
    }
    if let Some(members) = homogeneous_members(types, ty, config, 0)?
        && (1..=4).contains(&members.len())
        && layout.size == u64::from(members[0].size) * members.len() as u64
        && members
            .iter()
            .enumerate()
            .all(|(index, member)| member.offset == u64::from(member.size) * index as u64)
    {
        let class = AbiClass::Sse;
        let pieces = members
            .iter()
            .enumerate()
            .map(|(index, member)| AbiPiece {
                index: index as u8,
                offset: member.offset,
                valid_bytes: member.size,
                class,
            })
            .collect::<Vec<_>>();
        return Ok(ClassifiedType {
            ty,
            size: layout.size,
            align: layout.align,
            classes: vec![class; pieces.len()],
            pieces,
            passing: PassingMode::Registers,
        });
    }
    if layout.size > 16 {
        return Ok(memory_classification(ty, layout.size, layout.align));
    }
    let pieces = (0..layout.size.div_ceil(8))
        .map(|index| AbiPiece {
            index: index as u8,
            offset: index * 8,
            valid_bytes: layout.size.saturating_sub(index * 8).min(8) as u8,
            class: AbiClass::Integer,
        })
        .collect::<Vec<_>>();
    Ok(ClassifiedType {
        ty,
        size: layout.size,
        align: layout.align,
        classes: vec![AbiClass::Integer; pieces.len()],
        pieces,
        passing: PassingMode::Registers,
    })
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct HomogeneousMember {
    offset: u64,
    size: u8,
    builtin: BuiltinType,
}

fn homogeneous_members(
    types: &TypeStore,
    ty: TypeId,
    config: &EffectiveCompilationConfig,
    base: u64,
) -> Result<Option<Vec<HomogeneousMember>>, AbiError> {
    let result = match types.try_kind(ty) {
        Some(TypeKind::Builtin(
            builtin @ (BuiltinType::Float | BuiltinType::Double | BuiltinType::LongDouble),
        )) if *builtin != BuiltinType::LongDouble
            || config.target.data_layout.long_double_width == 64 =>
        {
            let normalized = if *builtin == BuiltinType::LongDouble {
                BuiltinType::Double
            } else {
                *builtin
            };
            let size = if normalized == BuiltinType::Float {
                4
            } else {
                8
            };
            Some(vec![HomogeneousMember {
                offset: base,
                size,
                builtin: normalized,
            }])
        }
        Some(TypeKind::Array(array)) => {
            let ArrayLength::Constant(length) = array.length else {
                return Ok(None);
            };
            let layout = types.layout_of(ty, config).map_err(|error| {
                AbiError::new("CCC3502", format!("array has no ABI layout: {error}"))
            })?;
            let LayoutShape::Array { stride, .. } = layout.shape else {
                return Ok(None);
            };
            let mut members = Vec::new();
            for index in 0..length {
                let Some(element) =
                    homogeneous_members(types, array.element.ty, config, base + index * stride)?
                else {
                    return Ok(None);
                };
                members.extend(element);
            }
            Some(members)
        }
        Some(TypeKind::Record(id)) => {
            let definition = types
                .record(*id)
                .ok_or_else(|| AbiError::new("CCC3502", format!("record {} is unknown", id.0)))?;
            let fields = definition.fields.as_ref().ok_or_else(|| {
                AbiError::new(
                    "CCC3502",
                    format!("type `{}` is incomplete", types.display(ty)),
                )
            })?;
            let layout = types.layout_of(ty, config).map_err(|error| {
                AbiError::new("CCC3502", format!("record has no ABI layout: {error}"))
            })?;
            let LayoutShape::Record(record_layout) = layout.shape else {
                return Ok(None);
            };
            if definition.kind == RecordKind::Union {
                let mut canonical: Option<Vec<HomogeneousMember>> = None;
                for field_layout in &record_layout.fields {
                    if field_layout.bitfield.is_some() {
                        return Ok(None);
                    }
                    let field = &fields[field_layout.index];
                    let Some(field_members) =
                        homogeneous_members(types, field.ty.ty, config, base)?
                    else {
                        return Ok(None);
                    };
                    if let Some(members) = &canonical {
                        let shared = members.len().min(field_members.len());
                        if members[..shared] != field_members[..shared] {
                            return Ok(None);
                        }
                        if field_members.len() > members.len() {
                            canonical = Some(field_members);
                        }
                    } else {
                        canonical = Some(field_members);
                    }
                }
                return Ok(canonical);
            }
            let mut members = Vec::new();
            for field_layout in &record_layout.fields {
                if field_layout.bitfield.is_some() {
                    return Ok(None);
                }
                let field = &fields[field_layout.index];
                let Some(field_members) =
                    homogeneous_members(types, field.ty.ty, config, base + field_layout.offset)?
                else {
                    return Ok(None);
                };
                members.extend(field_members);
            }
            Some(members)
        }
        _ => None,
    };
    let Some(members) = result else {
        return Ok(None);
    };
    if members.is_empty()
        || members.len() > 4
        || members
            .iter()
            .any(|member| member.builtin != members[0].builtin)
    {
        Ok(None)
    } else {
        Ok(Some(members))
    }
}

fn is_homogeneous(classified: &ClassifiedType) -> bool {
    classified.passing == PassingMode::Registers
        && !classified.pieces.is_empty()
        && classified
            .pieces
            .iter()
            .all(|piece| piece.class == AbiClass::Sse)
}

fn memory_classification(ty: TypeId, size: u64, align: u64) -> ClassifiedType {
    ClassifiedType {
        ty,
        size,
        align,
        classes: vec![AbiClass::Memory],
        pieces: Vec::new(),
        passing: PassingMode::Memory,
    }
}

/// Once the FP register bank is exhausted, a homogeneous aggregate is one
/// contiguous stack object. Vector carriers preserve that byte layout while
/// keeping the synthetic signature in the FP register class, so later integer
/// arguments can still use otherwise available x-registers.
fn homogeneous_stack_classification(
    classified: &ClassifiedType,
    abi: AbiIdentity,
) -> ClassifiedType {
    if abi == AbiIdentity::DarwinArm64 {
        return classified.clone();
    }
    let mut offset = 0;
    let mut pieces = Vec::new();
    while offset < classified.size {
        let remaining = classified.size - offset;
        let bytes = if remaining >= 8 { 8 } else { 4 };
        pieces.push(AbiPiece {
            index: pieces.len() as u8,
            offset,
            valid_bytes: bytes as u8,
            class: AbiClass::Sse,
        });
        offset += bytes;
    }
    let classes = vec![AbiClass::Sse; pieces.len()];
    ClassifiedType {
        pieces,
        classes,
        ..classified.clone()
    }
}

/// Cranelift's arm64 ABI implementation gives I128 stack carriers the exact
/// 16-byte alignment required by AAPCS C.14 and Darwin's natural stack
/// alignment. The source object has already been rounded to 16 bytes.
fn aligned_stack_classification(classified: &ClassifiedType) -> Result<ClassifiedType, AbiError> {
    if classified.size != 16 {
        return Err(AbiError::new(
            "CCC3521",
            "arm64 over-aligned register aggregate does not have a 16-byte stack representation",
        ));
    }
    Ok(ClassifiedType {
        classes: vec![AbiClass::Integer],
        pieces: vec![AbiPiece {
            index: 0,
            offset: 0,
            valid_bytes: 16,
            class: AbiClass::Integer,
        }],
        ..classified.clone()
    })
}

fn boundary_scalar(
    types: &TypeStore,
    ty: TypeId,
    config: &EffectiveCompilationConfig,
    boundary: &str,
) -> Result<AbiScalar, AbiError> {
    match types.try_kind(ty) {
        Some(TypeKind::Builtin(BuiltinType::Float)) => Ok(AbiScalar::Float32),
        Some(TypeKind::Builtin(BuiltinType::Double)) => Ok(AbiScalar::Float64),
        Some(TypeKind::Builtin(BuiltinType::LongDouble))
            if config.target.data_layout.long_double_width == 64 =>
        {
            Ok(AbiScalar::Float64)
        }
        Some(TypeKind::Builtin(BuiltinType::LongDouble)) => Err(AbiError::new(
            "CCC3509",
            format!(
                "binary128 `long double` {boundary} type `{}` has no enabled transport",
                types.display(ty)
            ),
        )),
        Some(TypeKind::Builtin(BuiltinType::Void)) => Err(AbiError::new(
            "CCC3507",
            "`void` has no scalar ABI representation",
        )),
        Some(TypeKind::Builtin(builtin)) if builtin.is_integer() => {
            let layout = types.layout_of(ty, config).map_err(|error| {
                AbiError::new("CCC3502", format!("integer has no ABI layout: {error}"))
            })?;
            let bits = (layout.size * 8) as u8;
            let signed = match builtin {
                BuiltinType::Char => config.target.data_layout.char_is_signed,
                BuiltinType::SignedChar
                | BuiltinType::Short
                | BuiltinType::Int
                | BuiltinType::Long
                | BuiltinType::LongLong => true,
                _ => false,
            };
            Ok(if signed {
                AbiScalar::SignedInteger { bits }
            } else {
                AbiScalar::UnsignedInteger { bits }
            })
        }
        Some(TypeKind::AlignmentAdjusted(_)) => {
            let builtin = types.builtin_type(ty).ok_or_else(|| {
                AbiError::new(
                    "CCC3508",
                    format!(
                        "type `{}` has no scalar ABI representation",
                        types.display(ty)
                    ),
                )
            })?;
            let layout = types.layout_of(ty, config).map_err(|error| {
                AbiError::new("CCC3502", format!("integer has no ABI layout: {error}"))
            })?;
            let bits = (layout.size * 8) as u8;
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

fn scalar_class(scalar: AbiScalar) -> AbiClass {
    match scalar {
        AbiScalar::Float32 | AbiScalar::Float64 => AbiClass::Sse,
        _ => AbiClass::Integer,
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
            _ => unreachable!("unsupported scalar width"),
        },
        AbiScalar::Float32 => AbiCarrier::F32,
        AbiScalar::Float64 => AbiCarrier::F64,
    }
}

fn scalar_size(scalar: AbiScalar) -> u8 {
    match scalar {
        AbiScalar::SignedInteger { bits }
        | AbiScalar::UnsignedInteger { bits }
        | AbiScalar::Pointer { bits } => bits / 8,
        AbiScalar::Float32 => 4,
        AbiScalar::Float64 => 8,
    }
}

fn scalar_extension(scalar: AbiScalar) -> IntegerExtension {
    match scalar {
        AbiScalar::SignedInteger { bits } if bits < 32 => IntegerExtension::Signed,
        AbiScalar::UnsignedInteger { bits } if bits < 32 => IntegerExtension::Unsigned,
        _ => IntegerExtension::None,
    }
}

fn float_piece_carrier(piece: &AbiPiece) -> Result<AbiCarrier, AbiError> {
    match piece.valid_bytes {
        4 => Ok(AbiCarrier::F32),
        8 => Ok(AbiCarrier::F64),
        _ => Err(AbiError::new(
            "CCC3503",
            "homogeneous arm64 aggregate has an invalid element width",
        )),
    }
}

fn vector_piece_carrier(piece: &AbiPiece) -> Result<AbiCarrier, AbiError> {
    match piece.valid_bytes {
        4 => Ok(AbiCarrier::V32),
        8 => Ok(AbiCarrier::V64),
        _ => Err(AbiError::new(
            "CCC3503",
            "arm64 stack homogeneous aggregate has an invalid vector carrier",
        )),
    }
}

fn reject_binary128_recursive(
    types: &TypeStore,
    ty: TypeId,
    config: &EffectiveCompilationConfig,
    boundary: &str,
) -> Result<(), AbiError> {
    if types.builtin_type(ty) == Some(BuiltinType::LongDouble)
        && config.target.data_layout.long_double_width == 128
    {
        return Err(AbiError::new(
            "CCC3509",
            format!(
                "binary128 `long double` {boundary} type `{}` has no enabled transport",
                types.display(ty)
            ),
        ));
    }
    match types.try_kind(ty) {
        Some(TypeKind::Array(array)) => {
            reject_binary128_recursive(types, array.element.ty, config, boundary)
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
                reject_binary128_recursive(types, field.ty.ty, config, boundary)?;
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

fn is_aggregate(types: &TypeStore, ty: TypeId) -> bool {
    matches!(
        types.try_kind(ty),
        Some(TypeKind::Array(_) | TypeKind::Record(_))
    )
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

#[cfg(test)]
mod tests {
    use ccc_types::{Field, QualifiedType};

    use super::*;

    fn record(types: &mut TypeStore, fields: Vec<Field>) -> TypeId {
        let (id, ty) = types.declare_record(RecordKind::Struct, None);
        types.complete_record(id, fields).unwrap();
        ty
    }

    #[test]
    fn aapcs64_classifies_hfa_integer_and_indirect_aggregates() {
        let config = EffectiveCompilationConfig::aarch64_unknown_linux_gnu();
        let mut types = TypeStore::default();
        let h = record(
            &mut types,
            vec![
                Field::named("a", TypeId::DOUBLE),
                Field::named("b", TypeId::DOUBLE),
                Field::named("c", TypeId::DOUBLE),
            ],
        );
        let i = record(
            &mut types,
            vec![
                Field::named("a", TypeId::LONG),
                Field::named("b", TypeId::LONG),
            ],
        );
        let l = record(
            &mut types,
            vec![
                Field::named("a", TypeId::LONG),
                Field::named("b", TypeId::LONG),
                Field::named("c", TypeId::LONG),
            ],
        );
        for (ty, carriers, expected) in [(h, 3, AbiCarrier::F64), (i, 2, AbiCarrier::I64)] {
            let signature = types.function_type(FunctionType::prototype(
                QualifiedType::unqualified(ty),
                vec![QualifiedType::unqualified(ty)],
            ));
            let plan = plan_function_type(&types, signature, &config).unwrap();
            assert_eq!(plan.clif_parameters.len(), carriers);
            assert!(
                plan.clif_parameters
                    .iter()
                    .all(|piece| piece.carrier == expected)
            );
        }
        let signature = types.function_type(FunctionType::prototype(
            QualifiedType::unqualified(l),
            vec![QualifiedType::unqualified(l)],
        ));
        let plan = plan_function_type(&types, signature, &config).unwrap();
        assert_eq!(
            plan.clif_parameters[1].purpose,
            NativePurpose::IndirectArgument
        );
        assert!(matches!(plan.result, NativeResultPlan::Indirect { .. }));
    }

    #[test]
    fn arm64_models_whole_stack_aggregate_transport_after_register_exhaustion() {
        let mut types = TypeStore::default();
        let integer_pair = record(
            &mut types,
            vec![
                Field::named("first", TypeId::LONG),
                Field::named("second", TypeId::LONG),
            ],
        );
        let float_pair = record(
            &mut types,
            vec![
                Field::named("first", TypeId::DOUBLE),
                Field::named("second", TypeId::DOUBLE),
            ],
        );
        for config in [
            EffectiveCompilationConfig::aarch64_unknown_linux_gnu(),
            EffectiveCompilationConfig::aarch64_apple_darwin(),
        ] {
            let mut integer_parameters = vec![QualifiedType::unqualified(TypeId::LONG); 7];
            integer_parameters.push(QualifiedType::unqualified(integer_pair));
            let integer_signature = types.function_type(FunctionType::prototype(
                QualifiedType::unqualified(TypeId::INT),
                integer_parameters,
            ));
            let plan = plan_function_type(&types, integer_signature, &config).unwrap();
            assert_eq!(
                plan.parameters[7].classified.passing,
                PassingMode::Registers
            );
            assert_eq!(plan.clif_parameters[7].purpose, NativePurpose::Padding);
            assert_eq!(plan.parameters[7].carrier_indices, [8, 9]);

            let mut float_parameters = vec![QualifiedType::unqualified(TypeId::DOUBLE); 7];
            float_parameters.push(QualifiedType::unqualified(float_pair));
            let float_signature = types.function_type(FunctionType::prototype(
                QualifiedType::unqualified(TypeId::INT),
                float_parameters,
            ));
            let plan = plan_function_type(&types, float_signature, &config).unwrap();
            assert_eq!(plan.clif_parameters[7].purpose, NativePurpose::Padding);
            assert_eq!(plan.parameters[7].carrier_indices, [8, 9]);
            if config.target.abi == AbiIdentity::Aapcs64Lp64 {
                assert!(plan.parameters[7].carrier_indices.iter().all(|index| {
                    matches!(
                        plan.clif_parameters[*index as usize].carrier,
                        AbiCarrier::V64
                    )
                }));
            } else {
                assert!(plan.parameters[7].carrier_indices.iter().all(|index| {
                    matches!(
                        plan.clif_parameters[*index as usize].carrier,
                        AbiCarrier::F64
                    )
                }));
            }
        }
    }

    #[test]
    fn darwin_does_not_apply_aapcs_even_register_padding() {
        let mut types = TypeStore::default();
        let aligned_pair = record(
            &mut types,
            vec![
                Field::named("first", TypeId::LONG).with_requested_alignment(Some(16)),
                Field::named("second", TypeId::LONG),
            ],
        );
        let parameters = vec![
            QualifiedType::unqualified(TypeId::LONG),
            QualifiedType::unqualified(aligned_pair),
        ];
        let linux_signature = types.function_type(FunctionType::prototype(
            QualifiedType::unqualified(TypeId::INT),
            parameters.clone(),
        ));
        let linux = plan_function_type(
            &types,
            linux_signature,
            &EffectiveCompilationConfig::aarch64_unknown_linux_gnu(),
        )
        .unwrap();
        assert_eq!(linux.clif_parameters[1].purpose, NativePurpose::Padding);
        assert_eq!(linux.parameters[1].carrier_indices, [2, 3]);

        let darwin_signature = types.function_type(FunctionType::prototype(
            QualifiedType::unqualified(TypeId::INT),
            parameters,
        ));
        let darwin = plan_function_type(
            &types,
            darwin_signature,
            &EffectiveCompilationConfig::aarch64_apple_darwin(),
        )
        .unwrap();
        assert_eq!(darwin.parameters[1].carrier_indices, [1, 2]);
        assert!(
            darwin
                .clif_parameters
                .iter()
                .all(|carrier| carrier.purpose != NativePurpose::Padding)
        );
    }

    #[test]
    fn union_hfas_deduplicate_uniquely_addressable_members() {
        let mut types = TypeStore::default();
        let doubles = types.array(ccc_types::ArrayType {
            element: QualifiedType::unqualified(TypeId::DOUBLE),
            length: ArrayLength::Constant(2),
        });
        let (id, union) = types.declare_record(RecordKind::Union, None);
        types
            .complete_record(
                id,
                vec![
                    Field::named("first", TypeId::DOUBLE),
                    Field::named("second", doubles),
                ],
            )
            .unwrap();
        let signature = types.function_type(FunctionType::prototype(
            QualifiedType::unqualified(TypeId::INT),
            vec![QualifiedType::unqualified(union)],
        ));
        let plan = plan_function_type(
            &types,
            signature,
            &EffectiveCompilationConfig::aarch64_unknown_linux_gnu(),
        )
        .unwrap();
        assert_eq!(
            plan.parameters[0].classified.classes,
            [AbiClass::Sse, AbiClass::Sse]
        );
        assert_eq!(plan.parameters[0].classified.pieces.len(), 2);
        assert!(
            plan.clif_parameters
                .iter()
                .all(|carrier| carrier.carrier == AbiCarrier::F64)
        );
    }

    #[test]
    fn aapcs64_variadic_bridge_places_an_exhausted_composite_wholly_on_stack() {
        let config = EffectiveCompilationConfig::aarch64_unknown_linux_gnu();
        let mut types = TypeStore::default();
        let pair = record(
            &mut types,
            vec![
                Field::named("first", TypeId::LONG),
                Field::named("second", TypeId::LONG),
            ],
        );
        let signature = types.function_type(FunctionType::variadic(
            QualifiedType::unqualified(TypeId::LONG),
            vec![QualifiedType::unqualified(TypeId::INT)],
        ));
        let mut actual = vec![TypeId::INT];
        actual.extend([TypeId::LONG; 7]);
        actual.push(pair);
        let plan = plan_variadic_call(&types, signature, &actual, 1, &config).unwrap();
        let aggregate = plan
            .parameter_pieces
            .iter()
            .filter(|piece| piece.source_index == Some(8))
            .collect::<Vec<_>>();
        assert_eq!(aggregate.len(), 2);
        assert!(
            aggregate
                .iter()
                .all(|piece| matches!(piece.location, BridgeLocation::Stack { .. }))
        );
    }

    #[test]
    fn darwin_variadic_stack_uses_eight_byte_scalar_slots() {
        // Apple Clang 21 targeting arm64-apple-macos11 stores unnamed `int`,
        // `int`, `double` arguments at sp+0, sp+8, and sp+16. Fixed overflow
        // `int` arguments remain compact at sp+0 and sp+4, so this rounding is
        // deliberately confined to the unnamed variadic portion.
        let config = EffectiveCompilationConfig::aarch64_apple_darwin();
        let mut types = TypeStore::default();
        let signature = types.function_type(FunctionType::variadic(
            QualifiedType::unqualified(TypeId::LONG),
            vec![QualifiedType::unqualified(TypeId::INT)],
        ));
        let actual = [TypeId::INT, TypeId::INT, TypeId::INT, TypeId::DOUBLE];
        let plan = plan_variadic_call(&types, signature, &actual, 1, &config).unwrap();
        let offsets = plan
            .parameter_pieces
            .iter()
            .filter_map(|piece| match piece.location {
                BridgeLocation::Stack { offset } => Some(offset),
                BridgeLocation::Register(_) => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(offsets, [0, 8, 16]);
        assert_eq!(
            plan_va_arg(&types, TypeId::INT, &config)
                .unwrap()
                .overflow_size,
            8
        );
        assert_eq!(
            plan_va_arg(&types, TypeId::INT, &config)
                .unwrap()
                .overflow_align,
            8
        );

        let mut fixed_parameters = vec![QualifiedType::unqualified(TypeId::LONG); 8];
        fixed_parameters.extend([QualifiedType::unqualified(TypeId::INT); 2]);
        let fixed_signature = types.function_type(FunctionType::variadic(
            QualifiedType::unqualified(TypeId::LONG),
            fixed_parameters,
        ));
        let mut mixed_actual = vec![TypeId::LONG; 8];
        mixed_actual.extend([
            TypeId::INT,
            TypeId::INT,
            TypeId::INT,
            TypeId::INT,
            TypeId::DOUBLE,
        ]);
        let fixed =
            plan_variadic_call(&types, fixed_signature, &mixed_actual, 10, &config).unwrap();
        let mixed_offsets = fixed
            .parameter_pieces
            .iter()
            .filter_map(|piece| match piece.location {
                BridgeLocation::Stack { offset } => Some((piece.source_index.unwrap(), offset)),
                BridgeLocation::Register(_) => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(mixed_offsets, [(8, 0), (9, 4), (10, 8), (11, 16), (12, 24)]);
    }

    #[test]
    fn indirect_va_arg_uses_the_pointer_slots_alignment() {
        let config = EffectiveCompilationConfig::aarch64_unknown_linux_gnu();
        let mut types = TypeStore::default();
        let large = record(
            &mut types,
            vec![
                Field::named("first", TypeId::LONG).with_requested_alignment(Some(16)),
                Field::named("second", TypeId::LONG),
                Field::named("third", TypeId::LONG),
            ],
        );
        let va_arg = plan_va_arg(&types, large, &config).unwrap();
        assert!(va_arg.indirect);
        assert_eq!(va_arg.overflow_align, 8);
        assert_eq!(va_arg.overflow_size, 8);

        let signature = types.function_type(FunctionType::variadic(
            QualifiedType::unqualified(TypeId::LONG),
            vec![QualifiedType::unqualified(TypeId::INT)],
        ));
        let mut actual = vec![TypeId::INT];
        actual.extend([TypeId::LONG; 8]);
        actual.push(large);
        let call = plan_variadic_call(&types, signature, &actual, 1, &config).unwrap();
        let pointer = call
            .parameter_pieces
            .iter()
            .find(|piece| piece.source_index == Some(9))
            .unwrap();
        assert!(pointer.indirect);
        assert_eq!(pointer.location, BridgeLocation::Stack { offset: 8 });
    }

    #[test]
    fn linux_binary128_scalar_and_aggregate_boundaries_are_exact_errors() {
        let config = EffectiveCompilationConfig::aarch64_unknown_linux_gnu();
        let mut types = TypeStore::default();
        let wrapper = record(
            &mut types,
            vec![
                Field::named("tag", TypeId::LONG),
                Field::named("value", TypeId::LONG_DOUBLE),
            ],
        );
        for ty in [TypeId::LONG_DOUBLE, wrapper] {
            let signature = types.function_type(FunctionType::prototype(
                QualifiedType::unqualified(ty),
                vec![QualifiedType::unqualified(ty)],
            ));
            let error = plan_function_type(&types, signature, &config).unwrap_err();
            assert_eq!(error.code, "CCC3509");
            assert!(error.message.contains("binary128"));
        }
        let error = plan_va_arg(&types, wrapper, &config).unwrap_err();
        assert_eq!(error.code, "CCC3509");
        assert!(error.message.contains("binary128"));
    }

    #[test]
    fn darwin_long_double_boundaries_use_binary64_carriers() {
        let config = EffectiveCompilationConfig::aarch64_apple_darwin();
        let mut types = TypeStore::default();
        let signature = types.function_type(FunctionType::prototype(
            QualifiedType::unqualified(TypeId::LONG_DOUBLE),
            vec![QualifiedType::unqualified(TypeId::LONG_DOUBLE)],
        ));
        let plan = plan_function_type(&types, signature, &config).unwrap();
        assert_eq!(plan.clif_parameters[0].carrier, AbiCarrier::F64);
        assert_eq!(plan.clif_results[0].carrier, AbiCarrier::F64);
        let va_arg = plan_va_arg(&types, TypeId::LONG_DOUBLE, &config).unwrap();
        assert_eq!(va_arg.classified.pieces[0].class, AbiClass::Sse);
    }
}
