use std::collections::BTreeMap;

use ccc_target::EffectiveCompilationConfig;
use ccc_types::{BuiltinType, QualifiedType, TypeId, TypeKind, TypeStore};

use super::super::{
    BinaryOperation, FullFunction, FullInstructionKind, ScalarConstant, ScalarConversion,
    UnaryOperation, ValueId,
};

#[derive(Clone, Copy, Debug)]
struct IntegerRepresentation {
    bits: u32,
    signed: bool,
    boolean: bool,
}

#[derive(Clone, Copy, Debug)]
struct IntegerValue {
    raw: u128,
    representation: IntegerRepresentation,
}

/// Folds operations whose result is fully determined by CCC's target-aware C
/// integer semantics. Floating-point reassociation and machine-level algebra
/// remain backend responsibilities.
pub(super) fn fold_constants(
    types: &TypeStore,
    function: &mut FullFunction,
    config: &EffectiveCompilationConfig,
) -> bool {
    let mut changed = false;
    loop {
        let constants = function
            .blocks
            .iter()
            .flat_map(|block| &block.instructions)
            .filter_map(
                |instruction| match (instruction.result, &instruction.kind) {
                    (Some(result), FullInstructionKind::Constant(constant)) => {
                        Some((result, *constant))
                    }
                    _ => None,
                },
            )
            .collect::<BTreeMap<_, _>>();
        let value_types = &function.value_types;
        let mut folded_any = false;
        for block in &mut function.blocks {
            for instruction in &mut block.instructions {
                let Some(result) = instruction.result else {
                    continue;
                };
                let Some(result_ty) = value_types.get(result.0 as usize).copied() else {
                    continue;
                };
                let Some(constant) = fold_instruction(
                    types,
                    &instruction.kind,
                    result_ty,
                    value_types,
                    &constants,
                    config,
                ) else {
                    continue;
                };
                instruction.kind = FullInstructionKind::Constant(constant);
                folded_any = true;
            }
        }
        if !folded_any {
            break;
        }
        changed = true;
    }
    changed
}

pub(super) fn normalize_integer_constant(
    types: &TypeStore,
    constant: ScalarConstant,
    ty: TypeId,
    config: &EffectiveCompilationConfig,
) -> Option<u128> {
    let representation = integer_representation(types, ty, config)?;
    integer_value(constant, representation).map(|value| value.raw)
}

fn fold_instruction(
    types: &TypeStore,
    kind: &FullInstructionKind,
    result_ty: TypeId,
    value_types: &[TypeId],
    constants: &BTreeMap<ValueId, ScalarConstant>,
    config: &EffectiveCompilationConfig,
) -> Option<ScalarConstant> {
    match kind {
        FullInstructionKind::Unary { operator, operand } => {
            let constant = constants.get(operand).copied()?;
            let operand_ty = *value_types.get(operand.0 as usize)?;
            fold_unary(
                *operator,
                constant,
                integer_representation(types, operand_ty, config)?,
                integer_representation(types, result_ty, config)?,
            )
        }
        FullInstructionKind::Binary {
            operator,
            left,
            right,
        } => {
            let left_constant = constants.get(left).copied()?;
            let right_constant = constants.get(right).copied()?;
            let left_ty = *value_types.get(left.0 as usize)?;
            let right_ty = *value_types.get(right.0 as usize)?;
            fold_binary(
                *operator,
                integer_value(
                    left_constant,
                    integer_representation(types, left_ty, config)?,
                )?,
                integer_value(
                    right_constant,
                    integer_representation(types, right_ty, config)?,
                )?,
                integer_representation(types, result_ty, config)?,
            )
        }
        FullInstructionKind::Convert {
            kind,
            operand,
            from,
            to,
        } => {
            let constant = constants.get(operand).copied()?;
            fold_conversion(types, *kind, constant, *from, *to, config)
        }
        _ => None,
    }
}

fn fold_unary(
    operator: UnaryOperation,
    constant: ScalarConstant,
    operand: IntegerRepresentation,
    result: IntegerRepresentation,
) -> Option<ScalarConstant> {
    let value = integer_value(constant, operand)?;
    let raw = match operator {
        UnaryOperation::Plus => value.raw,
        UnaryOperation::Negate if operand.signed => {
            let value = value.as_signed();
            let negated = value.checked_neg()?;
            if !fits_signed(negated, result.bits) {
                return None;
            }
            truncate(negated as u128, result.bits)
        }
        UnaryOperation::Negate => truncate(0u128.wrapping_sub(value.raw), result.bits),
        UnaryOperation::BitwiseNot => truncate(!value.raw, result.bits),
        UnaryOperation::LogicalNot => u128::from(value.raw == 0),
    };
    Some(constant_from_raw(raw, result))
}

fn fold_binary(
    operator: BinaryOperation,
    left: IntegerValue,
    right: IntegerValue,
    result: IntegerRepresentation,
) -> Option<ScalarConstant> {
    let arithmetic = left.representation;
    let raw = match operator {
        BinaryOperation::Multiply if arithmetic.signed => {
            signed_arithmetic(left, right, result, i128::checked_mul)?
        }
        BinaryOperation::Add if arithmetic.signed => {
            signed_arithmetic(left, right, result, i128::checked_add)?
        }
        BinaryOperation::Subtract if arithmetic.signed => {
            signed_arithmetic(left, right, result, i128::checked_sub)?
        }
        BinaryOperation::Multiply => truncate(left.raw.wrapping_mul(right.raw), result.bits),
        BinaryOperation::Add => truncate(left.raw.wrapping_add(right.raw), result.bits),
        BinaryOperation::Subtract => truncate(left.raw.wrapping_sub(right.raw), result.bits),
        BinaryOperation::Divide if right.raw == 0 => return None,
        BinaryOperation::Remainder if right.raw == 0 => return None,
        BinaryOperation::Divide if arithmetic.signed => {
            let quotient = left.as_signed().checked_div(right.as_signed())?;
            if !fits_signed(quotient, result.bits) {
                return None;
            }
            truncate(quotient as u128, result.bits)
        }
        BinaryOperation::Remainder if arithmetic.signed => {
            let remainder = left.as_signed().checked_rem(right.as_signed())?;
            truncate(remainder as u128, result.bits)
        }
        BinaryOperation::Divide => truncate(left.raw / right.raw, result.bits),
        BinaryOperation::Remainder => truncate(left.raw % right.raw, result.bits),
        BinaryOperation::LeftShift => {
            let count = shift_count(right)?;
            if count >= arithmetic.bits {
                return None;
            }
            if arithmetic.signed {
                let value = left.as_signed();
                if value < 0 {
                    return None;
                }
                let maximum = signed_maximum(result.bits);
                if value > (maximum >> count) {
                    return None;
                }
                let shifted = value << count;
                if !fits_signed(shifted, result.bits) {
                    return None;
                }
                truncate(shifted as u128, result.bits)
            } else {
                truncate(left.raw.checked_shl(count)?, result.bits)
            }
        }
        BinaryOperation::RightShift => {
            let count = shift_count(right)?;
            if count >= arithmetic.bits {
                return None;
            }
            if arithmetic.signed {
                let value = left.as_signed();
                if value < 0 {
                    // Right shift of a negative signed integer is
                    // implementation-defined; leave it explicit.
                    return None;
                }
                truncate((value >> count) as u128, result.bits)
            } else {
                truncate(left.raw >> count, result.bits)
            }
        }
        BinaryOperation::Less => u128::from(if arithmetic.signed {
            left.as_signed() < right.as_signed()
        } else {
            left.raw < right.raw
        }),
        BinaryOperation::LessEqual => u128::from(if arithmetic.signed {
            left.as_signed() <= right.as_signed()
        } else {
            left.raw <= right.raw
        }),
        BinaryOperation::Greater => u128::from(if arithmetic.signed {
            left.as_signed() > right.as_signed()
        } else {
            left.raw > right.raw
        }),
        BinaryOperation::GreaterEqual => u128::from(if arithmetic.signed {
            left.as_signed() >= right.as_signed()
        } else {
            left.raw >= right.raw
        }),
        BinaryOperation::Equal => u128::from(left.raw == right.raw),
        BinaryOperation::NotEqual => u128::from(left.raw != right.raw),
        BinaryOperation::BitwiseAnd => truncate(left.raw & right.raw, result.bits),
        BinaryOperation::BitwiseXor => truncate(left.raw ^ right.raw, result.bits),
        BinaryOperation::BitwiseOr => truncate(left.raw | right.raw, result.bits),
    };
    Some(constant_from_raw(raw, result))
}

fn signed_arithmetic(
    left: IntegerValue,
    right: IntegerValue,
    result: IntegerRepresentation,
    operation: fn(i128, i128) -> Option<i128>,
) -> Option<u128> {
    let value = operation(left.as_signed(), right.as_signed())?;
    fits_signed(value, result.bits).then(|| truncate(value as u128, result.bits))
}

fn fold_conversion(
    types: &TypeStore,
    kind: ScalarConversion,
    constant: ScalarConstant,
    from: QualifiedType,
    to: QualifiedType,
    config: &EffectiveCompilationConfig,
) -> Option<ScalarConstant> {
    match kind {
        ScalarConversion::IntegerPromotion
        | ScalarConversion::IntegerConversion
        | ScalarConversion::QualificationAdjustment => {
            let source = integer_value(constant, integer_representation(types, from.ty, config)?)?;
            let destination = integer_representation(types, to.ty, config)?;
            convert_integer(source, destination)
        }
        ScalarConversion::ToBoolean => {
            constant_truth(constant).map(|truth| ScalarConstant::Signed(i128::from(truth)))
        }
        ScalarConversion::PointerConversion if matches!(constant, ScalarConstant::NullPointer) => {
            Some(ScalarConstant::NullPointer)
        }
        ScalarConversion::ArrayToPointer
        | ScalarConversion::FunctionToPointer
        | ScalarConversion::FloatingConversion
        | ScalarConversion::IntegerToFloating
        | ScalarConversion::FloatingToInteger
        | ScalarConversion::PointerConversion
        | ScalarConversion::ToVoid => None,
    }
}

fn convert_integer(
    source: IntegerValue,
    destination: IntegerRepresentation,
) -> Option<ScalarConstant> {
    let raw = if destination.boolean {
        u128::from(source.raw != 0)
    } else if destination.signed {
        let value = if source.representation.signed {
            source.as_signed()
        } else {
            i128::try_from(source.raw).ok()?
        };
        if !fits_signed(value, destination.bits) {
            // Conversion to a signed type outside its range is
            // implementation-defined. Preserve it for the backend contract.
            return None;
        }
        truncate(value as u128, destination.bits)
    } else if source.representation.signed {
        truncate(source.as_signed() as u128, destination.bits)
    } else {
        truncate(source.raw, destination.bits)
    };
    Some(constant_from_raw(raw, destination))
}

fn integer_value(
    constant: ScalarConstant,
    representation: IntegerRepresentation,
) -> Option<IntegerValue> {
    let raw = match constant {
        ScalarConstant::Signed(value) => truncate(value as u128, representation.bits),
        ScalarConstant::Unsigned(value) => truncate(value, representation.bits),
        ScalarConstant::Floating(_)
        | ScalarConstant::LongDouble(_)
        | ScalarConstant::NullPointer => return None,
    };
    Some(IntegerValue {
        raw,
        representation,
    })
}

impl IntegerValue {
    fn as_signed(self) -> i128 {
        sign_extend(self.raw, self.representation.bits)
    }
}

fn shift_count(value: IntegerValue) -> Option<u32> {
    if value.representation.signed {
        u32::try_from(value.as_signed()).ok()
    } else {
        u32::try_from(value.raw).ok()
    }
}

fn integer_representation(
    types: &TypeStore,
    ty: TypeId,
    config: &EffectiveCompilationConfig,
) -> Option<IntegerRepresentation> {
    match types.try_kind(ty)? {
        TypeKind::Builtin(kind) => builtin_integer_representation(*kind, config),
        TypeKind::Enum(id) => {
            let underlying = types.enumeration(*id)?.body.as_ref()?.underlying;
            integer_representation(types, underlying, config)
        }
        TypeKind::AlignmentAdjusted(adjusted) => {
            integer_representation(types, adjusted.underlying, config)
        }
        TypeKind::Pointer(_) | TypeKind::Array(_) | TypeKind::Function(_) | TypeKind::Record(_) => {
            None
        }
    }
}

fn builtin_integer_representation(
    kind: BuiltinType,
    config: &EffectiveCompilationConfig,
) -> Option<IntegerRepresentation> {
    let layout = config.target.data_layout;
    let (bits, signed, boolean) = match kind {
        BuiltinType::Bool => (layout.bool_width, false, true),
        BuiltinType::Char => (layout.char_width, layout.char_is_signed, false),
        BuiltinType::SignedChar => (layout.char_width, true, false),
        BuiltinType::UnsignedChar => (layout.char_width, false, false),
        BuiltinType::Short => (layout.short_width, true, false),
        BuiltinType::UnsignedShort => (layout.short_width, false, false),
        BuiltinType::Int => (layout.int_width, true, false),
        BuiltinType::UnsignedInt => (layout.int_width, false, false),
        BuiltinType::Long => (layout.long_width, true, false),
        BuiltinType::UnsignedLong => (layout.long_width, false, false),
        BuiltinType::LongLong => (layout.long_long_width, true, false),
        BuiltinType::UnsignedLongLong => (layout.long_long_width, false, false),
        BuiltinType::Int128 => (128, true, false),
        BuiltinType::UnsignedInt128 => (128, false, false),
        BuiltinType::Void
        | BuiltinType::Float16
        | BuiltinType::Float
        | BuiltinType::Double
        | BuiltinType::LongDouble => return None,
    };
    Some(IntegerRepresentation {
        bits: u32::from(bits),
        signed,
        boolean,
    })
}

fn constant_from_raw(raw: u128, representation: IntegerRepresentation) -> ScalarConstant {
    let raw = truncate(raw, representation.bits);
    if representation.signed {
        ScalarConstant::Signed(sign_extend(raw, representation.bits))
    } else {
        ScalarConstant::Unsigned(raw)
    }
}

fn constant_truth(constant: ScalarConstant) -> Option<bool> {
    match constant {
        ScalarConstant::Signed(value) => Some(value != 0),
        ScalarConstant::Unsigned(value) => Some(value != 0),
        ScalarConstant::Floating(value) => Some(value != 0.0),
        ScalarConstant::NullPointer => Some(false),
        ScalarConstant::LongDouble(_) => None,
    }
}

fn truncate(value: u128, bits: u32) -> u128 {
    value & bit_mask(bits)
}

fn bit_mask(bits: u32) -> u128 {
    if bits >= 128 {
        u128::MAX
    } else {
        (1u128 << bits) - 1
    }
}

fn sign_extend(value: u128, bits: u32) -> i128 {
    if bits >= 128 {
        return value as i128;
    }
    let sign = 1u128 << (bits - 1);
    if value & sign == 0 {
        value as i128
    } else {
        (value | !bit_mask(bits)) as i128
    }
}

fn fits_signed(value: i128, bits: u32) -> bool {
    if bits >= 128 {
        return true;
    }
    let maximum = (1i128 << (bits - 1)) - 1;
    let minimum = -(1i128 << (bits - 1));
    (minimum..=maximum).contains(&value)
}

fn signed_maximum(bits: u32) -> i128 {
    if bits >= 128 {
        i128::MAX
    } else {
        (1i128 << (bits - 1)) - 1
    }
}

#[cfg(test)]
mod tests {
    use ccc_target::EffectiveCompilationConfig;

    use super::*;

    const U8: IntegerRepresentation = IntegerRepresentation {
        bits: 8,
        signed: false,
        boolean: false,
    };
    const I8: IntegerRepresentation = IntegerRepresentation {
        bits: 8,
        signed: true,
        boolean: false,
    };
    const I128: IntegerRepresentation = IntegerRepresentation {
        bits: 128,
        signed: true,
        boolean: false,
    };

    fn value(raw: u128, representation: IntegerRepresentation) -> IntegerValue {
        IntegerValue {
            raw,
            representation,
        }
    }

    #[test]
    fn unsigned_arithmetic_wraps_at_the_c_type_width() {
        assert_eq!(
            fold_binary(BinaryOperation::Add, value(255, U8), value(1, U8), U8,),
            Some(ScalarConstant::Unsigned(0))
        );
    }

    #[test]
    fn undefined_or_implementation_defined_integer_results_remain_explicit() {
        assert_eq!(
            fold_binary(BinaryOperation::Add, value(127, I8), value(1, I8), I8,),
            None
        );
        assert_eq!(
            fold_binary(
                BinaryOperation::RightShift,
                value(255, I8),
                value(1, I8),
                I8,
            ),
            None
        );
        assert_eq!(convert_integer(value(255, U8), I8), None);
        assert_eq!(
            fold_binary(
                BinaryOperation::LeftShift,
                value(1, I128),
                value(127, I128),
                I128,
            ),
            None
        );
    }

    #[test]
    fn integer_widths_come_from_the_effective_target() {
        let config = EffectiveCompilationConfig::aarch64_unknown_linux_gnu();
        let types = TypeStore::default();
        let representation = integer_representation(&types, TypeId::LONG, &config).unwrap();
        assert_eq!(
            representation.bits,
            u32::from(config.target.data_layout.long_width)
        );
        assert!(representation.signed);
    }
}
