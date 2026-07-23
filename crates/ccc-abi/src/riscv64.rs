//! RISC-V LP64D fixed-boundary classification.

use ccc_target::{AbiIdentity, EffectiveCompilationConfig};
use ccc_types::{
    ArrayLength, BuiltinType, FunctionParameters, FunctionType, LayoutShape, RecordKind, TypeId,
    TypeKind, TypeStore,
};

use crate::{AbiError, boundary_value_alignment, model::*};

const ARGUMENT_REGISTERS: u8 = 8;

pub(crate) fn validate_target(config: &EffectiveCompilationConfig) -> Result<(), AbiError> {
    if config.target.abi != AbiIdentity::RiscvLp64d {
        return Err(AbiError::new(
            "CCC3504",
            format!(
                "target `{}` does not use the RISC-V LP64D ABI identity",
                config.target.triple
            ),
        ));
    }
    let layout = config.target.data_layout;
    if layout.pointer_width != 64
        || layout.int_width != 32
        || layout.long_width != 64
        || layout.float_width != 32
        || layout.double_width != 64
    {
        return Err(AbiError::new(
            "CCC3504",
            format!(
                "target `{}` does not satisfy the RISC-V LP64D data model",
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
    let payload_size = if indirect { 8 } else { classified.size };
    Ok(VaArgPlan {
        result_size: classified.size,
        result_align: classified.align,
        overflow_size: align_up(payload_size, 8)?,
        overflow_align: if indirect {
            8
        } else {
            // GCC's RISC-V ABI treats a scalar typedef's alignment attribute
            // as object layout rather than variadic transport alignment.
            boundary_value_alignment(types, &classified, config)?.clamp(8, 16)
        },
        gp_slots: if indirect {
            1
        } else {
            payload_size.div_ceil(8) as u8
        },
        sse_slots: 0,
        classified,
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
    if result_classified.passing == PassingMode::Memory {
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

    let mut gp_used = u8::from(result_classified.passing == PassingMode::Memory);
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
                let mut class = scalar_class(scalar);
                let mut carrier = scalar_carrier(scalar);
                if class == AbiClass::Sse && fp_used >= ARGUMENT_REGISTERS {
                    class = AbiClass::Integer;
                    carrier = match scalar {
                        AbiScalar::Float16 => AbiCarrier::I16,
                        AbiScalar::Float32 => AbiCarrier::I32,
                        AbiScalar::Float64 => AbiCarrier::I64,
                        _ => unreachable!(),
                    };
                }
                if class == AbiClass::Sse {
                    fp_used += 1;
                } else {
                    gp_used = gp_used.saturating_add(1).min(ARGUMENT_REGISTERS);
                }
                carrier_indices.push(push_carrier(
                    &mut clif_parameters,
                    Some(source_index),
                    None,
                    0,
                    scalar_size(scalar),
                    class,
                    carrier,
                    scalar_extension(scalar),
                    NativePurpose::Normal,
                )?);
            }
            PassingMode::Registers if uses_hardware_float(&classified) => {
                let gp_needed = classified
                    .pieces
                    .iter()
                    .filter(|piece| piece.class == AbiClass::Integer)
                    .count() as u8;
                let fp_needed = classified
                    .pieces
                    .iter()
                    .filter(|piece| piece.class == AbiClass::Sse)
                    .count() as u8;
                if gp_used.saturating_add(gp_needed) <= ARGUMENT_REGISTERS
                    && fp_used.saturating_add(fp_needed) <= ARGUMENT_REGISTERS
                {
                    gp_used += gp_needed;
                    fp_used += fp_needed;
                } else {
                    classified = integer_aggregate_classification(&classified);
                    gp_used = gp_used
                        .saturating_add(classified.pieces.len() as u8)
                        .min(ARGUMENT_REGISTERS);
                }
                for piece in &classified.pieces {
                    carrier_indices.push(push_carrier(
                        &mut clif_parameters,
                        Some(source_index),
                        Some(piece.index),
                        piece.offset,
                        piece.valid_bytes,
                        piece.class,
                        flattened_piece_carrier(piece)?,
                        IntegerExtension::None,
                        NativePurpose::Normal,
                    )?);
                }
            }
            PassingMode::Registers => {
                gp_used = gp_used
                    .saturating_add(classified.pieces.len() as u8)
                    .min(ARGUMENT_REGISTERS);
                for piece in &classified.pieces {
                    carrier_indices.push(push_carrier(
                        &mut clif_parameters,
                        Some(source_index),
                        Some(piece.index),
                        piece.offset,
                        piece.valid_bytes,
                        AbiClass::Integer,
                        AbiCarrier::I64,
                        IntegerExtension::None,
                        NativePurpose::Normal,
                    )?);
                }
            }
            PassingMode::Memory => {
                gp_used = gp_used.saturating_add(1).min(ARGUMENT_REGISTERS);
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
            let mut carrier_indices = Vec::new();
            let flattened = uses_hardware_float(&result_classified);
            for piece in &result_classified.pieces {
                carrier_indices.push(push_carrier(
                    &mut clif_results,
                    None,
                    Some(piece.index),
                    piece.offset,
                    piece.valid_bytes,
                    piece.class,
                    if flattened {
                        flattened_piece_carrier(piece)?
                    } else {
                        piece_carrier(piece)?
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
    let mut gp_used = u8::from(hidden_return);
    let mut fp_used = 0u8;
    let mut stack_size = 0u64;
    let mut variadic_stack_started = false;
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
            &mut gp_used,
            &mut fp_used,
            &mut stack_size,
            &mut variadic_stack_started,
            &mut parameter_pieces,
        )?;
        parameters.push(classified);
        if source_index + 1 == variadic_boundary {
            fixed_stack_end = Some(stack_size);
        }
    }
    let fixed_stack_end = fixed_stack_end.unwrap_or(stack_size);
    let stack_size = u32::try_from(align_up(stack_size, 16)?)
        .map_err(|_| AbiError::new("CCC3503", "RISC-V bridge stack payload is too large"))?;
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
        abi_identity: AbiIdentity::RiscvLp64d,
        calling_convention: config.target.abi.calling_convention(),
        kind,
        parameters,
        parameter_pieces,
        result_pieces: bridge_result_pieces(&result, result_extension),
        result,
        hidden_return,
        overflow_arg_offset: u32::try_from(fixed_stack_end).map_err(|_| {
            AbiError::new("CCC3503", "RISC-V variadic overflow offset is too large")
        })?,
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
    gp_used: &mut u8,
    fp_used: &mut u8,
    stack_size: &mut u64,
    variadic_stack_started: &mut bool,
    pieces: &mut Vec<BridgePiecePlan>,
) -> Result<(), AbiError> {
    if classified.passing == PassingMode::Memory {
        let pointer_piece = AbiPiece {
            index: 0,
            offset: 0,
            valid_bytes: 8,
            class: AbiClass::Integer,
        };
        let location =
            allocate_integer_slot(1, 8, unnamed, gp_used, stack_size, variadic_stack_started)?
                .into_iter()
                .next()
                .expect("one pointer slot");
        pieces.push(BridgePiecePlan {
            source_index: Some(source_index),
            piece: pointer_piece,
            extension: IntegerExtension::None,
            indirect: true,
            location,
        });
        return Ok(());
    }

    if !unnamed
        && classified.passing == PassingMode::Scalar
        && classified.pieces[0].class == AbiClass::Sse
        && *fp_used < ARGUMENT_REGISTERS
    {
        pieces.push(BridgePiecePlan {
            source_index: Some(source_index),
            piece: classified.pieces[0].clone(),
            extension,
            indirect: false,
            location: BridgeLocation::Register(RegisterSlot::float(*fp_used)),
        });
        *fp_used += 1;
        return Ok(());
    }

    if !unnamed && uses_hardware_float(classified) {
        let gp_needed = classified
            .pieces
            .iter()
            .filter(|piece| piece.class == AbiClass::Integer)
            .count() as u8;
        let fp_needed = classified
            .pieces
            .iter()
            .filter(|piece| piece.class == AbiClass::Sse)
            .count() as u8;
        if gp_used.saturating_add(gp_needed) <= ARGUMENT_REGISTERS
            && fp_used.saturating_add(fp_needed) <= ARGUMENT_REGISTERS
        {
            for piece in &classified.pieces {
                let register = if piece.class == AbiClass::Sse {
                    let register = RegisterSlot::float(*fp_used);
                    *fp_used += 1;
                    register
                } else {
                    let register = RegisterSlot::integer(*gp_used);
                    *gp_used += 1;
                    register
                };
                pieces.push(BridgePiecePlan {
                    source_index: Some(source_index),
                    piece: piece.clone(),
                    extension,
                    indirect: false,
                    location: BridgeLocation::Register(register),
                });
            }
            return Ok(());
        }
    }

    let integer = if classified.passing == PassingMode::Scalar {
        classified.clone()
    } else {
        integer_aggregate_classification(classified)
    };
    let slot_count = integer.size.div_ceil(8).max(1) as u8;
    let slot_alignment = if classified.passing == PassingMode::Scalar {
        boundary_alignment
    } else {
        integer.align
    };
    let locations = allocate_integer_slot(
        slot_count,
        slot_alignment,
        unnamed,
        gp_used,
        stack_size,
        variadic_stack_started,
    )?;
    for (index, location) in locations.into_iter().enumerate() {
        let piece = if integer.passing == PassingMode::Scalar {
            integer.pieces[0].clone()
        } else {
            integer.pieces[index].clone()
        };
        pieces.push(BridgePiecePlan {
            source_index: Some(source_index),
            piece,
            extension,
            indirect: false,
            location,
        });
    }
    Ok(())
}

fn allocate_integer_slot(
    slots: u8,
    align: u64,
    unnamed: bool,
    gp_used: &mut u8,
    stack_size: &mut u64,
    variadic_stack_started: &mut bool,
) -> Result<Vec<BridgeLocation>, AbiError> {
    if unnamed && align >= 16 && *gp_used < ARGUMENT_REGISTERS && !gp_used.is_multiple_of(2) {
        *gp_used += 1;
    }
    let pair_must_fit = unnamed && slots == 2 && align >= 16;
    let registers_available = ARGUMENT_REGISTERS.saturating_sub(*gp_used);
    let use_stack = (unnamed && *variadic_stack_started)
        || (pair_must_fit && registers_available < slots)
        || registers_available == 0;
    if use_stack {
        if unnamed {
            *variadic_stack_started = true;
        }
        *stack_size = align_up(*stack_size, align.clamp(8, 16))?;
        let base = *stack_size;
        *stack_size = stack_size
            .checked_add(u64::from(slots) * 8)
            .ok_or_else(|| AbiError::new("CCC3503", "RISC-V bridge stack size overflow"))?;
        return (0..slots)
            .map(|index| {
                Ok(BridgeLocation::Stack {
                    offset: u32::try_from(base + u64::from(index) * 8).map_err(|_| {
                        AbiError::new("CCC3503", "RISC-V bridge stack offset overflow")
                    })?,
                })
            })
            .collect();
    }

    let register_slots = slots.min(registers_available);
    let mut locations = Vec::with_capacity(slots as usize);
    for _ in 0..register_slots {
        locations.push(BridgeLocation::Register(RegisterSlot::integer(*gp_used)));
        *gp_used += 1;
    }
    if register_slots < slots {
        *stack_size = align_up(*stack_size, 8)?;
        let base = *stack_size;
        for index in register_slots..slots {
            locations.push(BridgeLocation::Stack {
                offset: u32::try_from(base + u64::from(index - register_slots) * 8)
                    .map_err(|_| AbiError::new("CCC3503", "RISC-V bridge stack offset overflow"))?,
            });
        }
        *stack_size = stack_size
            .checked_add(u64::from(slots - register_slots) * 8)
            .ok_or_else(|| AbiError::new("CCC3503", "RISC-V bridge stack size overflow"))?;
        if unnamed {
            *variadic_stack_started = true;
        }
    }
    Ok(locations)
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
        .ok_or_else(|| AbiError::new("CCC3503", "RISC-V ABI size overflow"))
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
    if layout.size > 16 {
        return Ok(memory_classification(ty, layout.size, layout.align));
    }
    if let Some(leaves) = flatten_aggregate(types, ty, config, 0)?
        && (1..=2).contains(&leaves.len())
        && leaves.iter().any(|leaf| leaf.class == AbiClass::Sse)
    {
        let pieces = leaves
            .iter()
            .enumerate()
            .map(|(index, leaf)| AbiPiece {
                index: index as u8,
                offset: leaf.offset,
                valid_bytes: leaf.size,
                class: leaf.class,
            })
            .collect::<Vec<_>>();
        return Ok(ClassifiedType {
            ty,
            size: layout.size,
            align: layout.align,
            classes: pieces.iter().map(|piece| piece.class).collect(),
            pieces,
            passing: PassingMode::Registers,
        });
    }
    Ok(integer_classification(ty, layout.size, layout.align))
}

#[derive(Clone, Copy)]
struct FlattenedLeaf {
    offset: u64,
    size: u8,
    class: AbiClass,
}

fn flatten_aggregate(
    types: &TypeStore,
    ty: TypeId,
    config: &EffectiveCompilationConfig,
    base: u64,
) -> Result<Option<Vec<FlattenedLeaf>>, AbiError> {
    match types.try_kind(ty) {
        Some(TypeKind::Builtin(BuiltinType::Float16)) => Ok(Some(vec![FlattenedLeaf {
            offset: base,
            size: 2,
            class: AbiClass::Sse,
        }])),
        Some(TypeKind::Builtin(BuiltinType::Float)) => Ok(Some(vec![FlattenedLeaf {
            offset: base,
            size: 4,
            class: AbiClass::Sse,
        }])),
        Some(TypeKind::Builtin(BuiltinType::Double)) => Ok(Some(vec![FlattenedLeaf {
            offset: base,
            size: 8,
            class: AbiClass::Sse,
        }])),
        Some(TypeKind::Builtin(builtin)) if builtin.is_integer() => {
            let layout = types.layout_of(ty, config).map_err(|error| {
                AbiError::new("CCC3502", format!("integer has no ABI layout: {error}"))
            })?;
            Ok(Some(vec![FlattenedLeaf {
                offset: base,
                size: layout.size as u8,
                class: AbiClass::Integer,
            }]))
        }
        Some(TypeKind::AlignmentAdjusted(_)) => {
            let layout = types.layout_of(ty, config).map_err(|error| {
                AbiError::new("CCC3502", format!("integer has no ABI layout: {error}"))
            })?;
            Ok(Some(vec![FlattenedLeaf {
                offset: base,
                size: layout.size as u8,
                class: AbiClass::Integer,
            }]))
        }
        Some(TypeKind::Pointer(_)) => Ok(Some(vec![FlattenedLeaf {
            offset: base,
            size: 8,
            class: AbiClass::Integer,
        }])),
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
            flatten_aggregate(types, underlying, config, base)
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
            let mut leaves = Vec::new();
            for index in 0..length {
                let Some(element) =
                    flatten_aggregate(types, array.element.ty, config, base + index * stride)?
                else {
                    return Ok(None);
                };
                leaves.extend(element);
            }
            Ok((leaves.len() <= 2).then_some(leaves))
        }
        Some(TypeKind::Record(id)) => {
            let definition = types
                .record(*id)
                .ok_or_else(|| AbiError::new("CCC3502", format!("record {} is unknown", id.0)))?;
            if definition.kind != RecordKind::Struct {
                return Ok(None);
            }
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
            let mut leaves = Vec::new();
            for field_layout in &record_layout.fields {
                if field_layout.bitfield.is_some() {
                    return Ok(None);
                }
                let field = &fields[field_layout.index];
                let Some(field_leaves) =
                    flatten_aggregate(types, field.ty.ty, config, base + field_layout.offset)?
                else {
                    return Ok(None);
                };
                leaves.extend(field_leaves);
                if leaves.len() > 2 {
                    return Ok(None);
                }
            }
            Ok(Some(leaves))
        }
        _ => Ok(None),
    }
}

fn integer_classification(ty: TypeId, size: u64, align: u64) -> ClassifiedType {
    let pieces = (0..size.div_ceil(8))
        .map(|index| AbiPiece {
            index: index as u8,
            offset: index * 8,
            valid_bytes: size.saturating_sub(index * 8).min(8) as u8,
            class: AbiClass::Integer,
        })
        .collect::<Vec<_>>();
    ClassifiedType {
        ty,
        size,
        align,
        classes: vec![AbiClass::Integer; pieces.len()],
        pieces,
        passing: PassingMode::Registers,
    }
}

fn integer_aggregate_classification(classified: &ClassifiedType) -> ClassifiedType {
    integer_classification(classified.ty, classified.size, classified.align)
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

fn uses_hardware_float(classified: &ClassifiedType) -> bool {
    classified
        .pieces
        .iter()
        .any(|piece| piece.class == AbiClass::Sse)
}

fn boundary_scalar(
    types: &TypeStore,
    ty: TypeId,
    config: &EffectiveCompilationConfig,
    boundary: &str,
) -> Result<AbiScalar, AbiError> {
    match types.try_kind(ty) {
        Some(TypeKind::Builtin(BuiltinType::Float16)) => Ok(AbiScalar::Float16),
        Some(TypeKind::Builtin(BuiltinType::Float)) => Ok(AbiScalar::Float32),
        Some(TypeKind::Builtin(BuiltinType::Double)) => Ok(AbiScalar::Float64),
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
        AbiScalar::Float16 | AbiScalar::Float32 | AbiScalar::Float64 => AbiClass::Sse,
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
        AbiScalar::Float16 => AbiCarrier::F16,
        AbiScalar::Float32 => AbiCarrier::F32,
        AbiScalar::Float64 => AbiCarrier::F64,
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

fn scalar_extension(scalar: AbiScalar) -> IntegerExtension {
    match scalar {
        AbiScalar::SignedInteger { bits } if bits < 32 => IntegerExtension::Signed,
        AbiScalar::UnsignedInteger { bits } if bits < 32 => IntegerExtension::Unsigned,
        _ => IntegerExtension::None,
    }
}

fn piece_carrier(piece: &AbiPiece) -> Result<AbiCarrier, AbiError> {
    match (piece.class, piece.valid_bytes) {
        (AbiClass::Integer, _) => Ok(AbiCarrier::I64),
        (AbiClass::Sse, 2) => Ok(AbiCarrier::F16),
        (AbiClass::Sse, 4) => Ok(AbiCarrier::F32),
        (AbiClass::Sse, 8) => Ok(AbiCarrier::F64),
        _ => Err(AbiError::new(
            "CCC3503",
            "RISC-V aggregate has an invalid register carrier",
        )),
    }
}

fn flattened_piece_carrier(piece: &AbiPiece) -> Result<AbiCarrier, AbiError> {
    match (piece.class, piece.valid_bytes) {
        (AbiClass::Integer, 1) => Ok(AbiCarrier::I8),
        (AbiClass::Integer, 2) => Ok(AbiCarrier::I16),
        (AbiClass::Integer, 4) => Ok(AbiCarrier::I32),
        (AbiClass::Integer, 8) => Ok(AbiCarrier::I64),
        (AbiClass::Sse, 2) => Ok(AbiCarrier::F16),
        (AbiClass::Sse, 4) => Ok(AbiCarrier::F32),
        (AbiClass::Sse, 8) => Ok(AbiCarrier::F64),
        _ => Err(AbiError::new(
            "CCC3503",
            "RISC-V flattened aggregate has an invalid register carrier",
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
    fn lp64d_uses_float_mixed_integer_and_indirect_forms() {
        let config = EffectiveCompilationConfig::riscv64_unknown_linux_gnu();
        let mut types = TypeStore::default();
        let floats = record(
            &mut types,
            vec![
                Field::named("first", TypeId::DOUBLE),
                Field::named("second", TypeId::DOUBLE),
            ],
        );
        let mixed = record(
            &mut types,
            vec![
                Field::named("first", TypeId::DOUBLE),
                Field::named("second", TypeId::LONG),
            ],
        );
        let large = record(
            &mut types,
            vec![
                Field::named("first", TypeId::LONG),
                Field::named("second", TypeId::LONG),
                Field::named("third", TypeId::LONG),
            ],
        );
        for (ty, carriers) in [
            (floats, vec![AbiCarrier::F64, AbiCarrier::F64]),
            (mixed, vec![AbiCarrier::F64, AbiCarrier::I64]),
        ] {
            let signature = types.function_type(FunctionType::prototype(
                QualifiedType::unqualified(ty),
                vec![QualifiedType::unqualified(ty)],
            ));
            let plan = plan_function_type(&types, signature, &config).unwrap();
            assert_eq!(
                plan.clif_parameters
                    .iter()
                    .map(|carrier| carrier.carrier)
                    .collect::<Vec<_>>(),
                carriers
            );
        }
        let signature = types.function_type(FunctionType::prototype(
            QualifiedType::unqualified(large),
            vec![QualifiedType::unqualified(large)],
        ));
        let plan = plan_function_type(&types, signature, &config).unwrap();
        assert_eq!(
            plan.clif_parameters[1].purpose,
            NativePurpose::IndirectArgument
        );
        assert!(matches!(plan.result, NativeResultPlan::Indirect { .. }));
    }

    #[test]
    fn mixed_fp_aggregate_integer_leaves_keep_their_true_width() {
        let config = EffectiveCompilationConfig::riscv64_unknown_linux_gnu();
        let mut types = TypeStore::default();
        let (enum_id, enumeration) = types.declare_enum(None);
        types
            .complete_enum(enum_id, TypeId::INT, Vec::new())
            .unwrap();
        for (first, second, carriers) in [
            (
                TypeId::FLOAT,
                TypeId::INT,
                [AbiCarrier::F32, AbiCarrier::I32],
            ),
            (
                TypeId::INT,
                TypeId::FLOAT,
                [AbiCarrier::I32, AbiCarrier::F32],
            ),
            (
                TypeId::DOUBLE,
                enumeration,
                [AbiCarrier::F64, AbiCarrier::I32],
            ),
            (
                enumeration,
                TypeId::FLOAT,
                [AbiCarrier::I32, AbiCarrier::F32],
            ),
        ] {
            let mixed = record(
                &mut types,
                vec![Field::named("first", first), Field::named("second", second)],
            );
            let signature = types.function_type(FunctionType::prototype(
                QualifiedType::unqualified(mixed),
                vec![QualifiedType::unqualified(mixed)],
            ));
            let plan = plan_function_type(&types, signature, &config).unwrap();
            assert_eq!(
                plan.clif_parameters
                    .iter()
                    .map(|carrier| carrier.carrier)
                    .collect::<Vec<_>>(),
                carriers
            );
            assert_eq!(
                plan.clif_results
                    .iter()
                    .map(|carrier| carrier.carrier)
                    .collect::<Vec<_>>(),
                carriers
            );
        }
    }

    #[test]
    fn named_aligned_pair_does_not_use_the_variadic_even_register_rule() {
        let config = EffectiveCompilationConfig::riscv64_unknown_linux_gnu();
        let mut types = TypeStore::default();
        let aligned = record(
            &mut types,
            vec![
                Field::named("first", TypeId::LONG).with_requested_alignment(Some(16)),
                Field::named("second", TypeId::LONG),
            ],
        );
        let signature = types.function_type(FunctionType::variadic(
            QualifiedType::unqualified(TypeId::LONG),
            vec![
                QualifiedType::unqualified(TypeId::INT),
                QualifiedType::unqualified(aligned),
            ],
        ));
        let plan =
            plan_variadic_call(&types, signature, &[TypeId::INT, aligned], 2, &config).unwrap();
        let locations = plan
            .parameter_pieces
            .iter()
            .filter(|piece| piece.source_index == Some(1))
            .map(|piece| piece.location)
            .collect::<Vec<_>>();
        assert_eq!(
            locations,
            [
                BridgeLocation::Register(RegisterSlot::integer(1)),
                BridgeLocation::Register(RegisterSlot::integer(2)),
            ]
        );
    }

    #[test]
    fn linux_binary128_scalar_and_aggregate_boundaries_are_exact_errors() {
        let config = EffectiveCompilationConfig::riscv64_unknown_linux_gnu();
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
}
