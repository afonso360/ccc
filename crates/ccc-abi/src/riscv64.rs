//! RISC-V LP64D fixed-boundary classification.

use ccc_target::{AbiIdentity, EffectiveCompilationConfig};
use ccc_types::{
    ArrayLength, BuiltinType, FunctionParameters, FunctionType, LayoutShape, RecordKind, TypeId,
    TypeKind, TypeStore,
};

use crate::{AbiError, model::*};

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
        return Err(variadic_transport_error("function type"));
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
        return Err(variadic_transport_error("function definition"));
    }
    Ok(BoundaryPlan::Native(plan_native_signature(
        types, &signature, config,
    )?))
}

pub(crate) fn plan_variadic_call(
    _types: &TypeStore,
    _signature: TypeId,
    _actual_types: &[TypeId],
    _variadic_boundary: usize,
    config: &EffectiveCompilationConfig,
) -> Result<BridgeBoundaryPlan, AbiError> {
    validate_target(config)?;
    Err(variadic_transport_error("variadic call"))
}

pub(crate) fn plan_unprototyped_call(
    _types: &TypeStore,
    _signature: TypeId,
    _promoted_actual_types: &[TypeId],
    config: &EffectiveCompilationConfig,
) -> Result<BridgeBoundaryPlan, AbiError> {
    validate_target(config)?;
    Err(variadic_transport_error("unprototyped call"))
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
    _types: &TypeStore,
    _ty: TypeId,
    config: &EffectiveCompilationConfig,
) -> Result<VaArgPlan, AbiError> {
    validate_target(config)?;
    Err(variadic_transport_error("va_arg"))
}

fn variadic_transport_error(boundary: &str) -> AbiError {
    AbiError::new(
        "CCC3520",
        format!("{boundary} requires the RISC-V LP64D variadic transport adapter"),
    )
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
                        piece_carrier(piece)?,
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
            for piece in &result_classified.pieces {
                carrier_indices.push(push_carrier(
                    &mut clif_results,
                    None,
                    Some(piece.index),
                    piece.offset,
                    piece.valid_bytes,
                    piece.class,
                    piece_carrier(piece)?,
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
        Some(TypeKind::Pointer(_) | TypeKind::Enum(_)) => Ok(Some(vec![FlattenedLeaf {
            offset: base,
            size: 8,
            class: AbiClass::Integer,
        }])),
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

fn piece_carrier(piece: &AbiPiece) -> Result<AbiCarrier, AbiError> {
    match (piece.class, piece.valid_bytes) {
        (AbiClass::Integer, _) => Ok(AbiCarrier::I64),
        (AbiClass::Sse, 4) => Ok(AbiCarrier::F32),
        (AbiClass::Sse, 8) => Ok(AbiCarrier::F64),
        _ => Err(AbiError::new(
            "CCC3503",
            "RISC-V aggregate has an invalid register carrier",
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
}
