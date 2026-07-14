//! Immutable scalar call plans for the enabled C ABI.

use std::fmt;

use ccc_session::Span;
use ccc_target::{CallingConvention, EffectiveCompilationConfig};
use ccc_types::{
    BuiltinType, FunctionParameters, FunctionType, QualifiedType, TypeId, TypeKind, TypeStore,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AbiScalar {
    SignedInteger { bits: u8 },
    UnsignedInteger { bits: u8 },
    Pointer { bits: u8 },
    Float32,
    Float64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FunctionPlan {
    pub calling_convention: CallingConvention,
    pub parameters: Vec<AbiScalar>,
    pub result: Option<AbiScalar>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AbiError {
    pub code: &'static str,
    pub message: String,
    pub span: Option<Span>,
}

impl AbiError {
    pub fn with_span_if_none(mut self, span: Span) -> Self {
        self.span.get_or_insert(span);
        self
    }
}

impl fmt::Display for AbiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.message.fmt(formatter)
    }
}

impl std::error::Error for AbiError {}

/// Builds a scalar ABI plan directly from a canonical function type.
///
/// This entry point does not require a CCC-IR module and is suitable for both
/// definitions and individual call sites. Aggregate and bridge-requiring
/// boundaries fail with stable capability diagnostics.
pub fn plan_function_type(
    types: &TypeStore,
    signature: TypeId,
    config: &EffectiveCompilationConfig,
) -> Result<FunctionPlan, AbiError> {
    let signature = types
        .function_signature(signature)
        .ok_or_else(|| AbiError {
            code: "CCC3505",
            message: format!(
                "type `{}` is not a function type and has no function ABI plan",
                types.display(signature)
            ),
            span: None,
        })?;
    plan_signature(types, &signature, config)
}

fn plan_signature(
    types: &TypeStore,
    signature: &FunctionType,
    config: &EffectiveCompilationConfig,
) -> Result<FunctionPlan, AbiError> {
    let calling_convention = config.target.calling_convention().ok_or_else(|| AbiError {
        code: "CCC3504",
        message: format!(
            "target `{}` does not define a C calling convention",
            config.target.triple
        ),
        span: None,
    })?;
    if signature.variadic {
        return Err(AbiError {
            code: "CCC3510",
            message: "variadic function boundaries require the target variadic bridge capability"
                .to_owned(),
            span: None,
        });
    }
    let FunctionParameters::Prototype(parameters) = &signature.parameters else {
        return Err(AbiError {
            code: "CCC3506",
            message: "a function type without a prototype has no fixed scalar ABI plan".to_owned(),
            span: None,
        });
    };
    let parameters = parameters
        .iter()
        .map(|parameter| parameter_scalar(types, *parameter, config))
        .collect::<Result<_, _>>()?;
    let result = if types.builtin_type(signature.result.ty) == Some(BuiltinType::Void) {
        None
    } else {
        Some(boundary_scalar(types, signature.result, config, "return")?)
    };
    Ok(FunctionPlan {
        calling_convention,
        parameters,
        result,
    })
}

fn parameter_scalar(
    types: &TypeStore,
    parameter: QualifiedType,
    config: &EffectiveCompilationConfig,
) -> Result<AbiScalar, AbiError> {
    if types.builtin_type(parameter.ty) == Some(BuiltinType::Void) {
        return Err(AbiError {
            code: "CCC3507",
            message: "`void` cannot appear as a function parameter type".to_owned(),
            span: None,
        });
    }
    boundary_scalar(types, parameter, config, "parameter")
}

fn boundary_scalar(
    types: &TypeStore,
    ty: QualifiedType,
    config: &EffectiveCompilationConfig,
    boundary: &str,
) -> Result<AbiScalar, AbiError> {
    match types.try_kind(ty.ty) {
        Some(TypeKind::Builtin(builtin)) => {
            builtin_scalar(types, ty.ty, *builtin, config, boundary)
        }
        Some(TypeKind::Pointer(_)) => {
            let layout = target_layout(types, ty.ty, config)?;
            Ok(AbiScalar::Pointer {
                bits: layout_bits(layout.size, "pointer")?,
            })
        }
        Some(TypeKind::Enum(id)) => {
            let definition = types.enumeration(*id).ok_or_else(|| AbiError {
                code: "CCC3502",
                message: format!("type `{}` has no target layout", types.display(ty.ty)),
                span: None,
            })?;
            let body = definition.body.as_ref().ok_or_else(|| AbiError {
                code: "CCC3502",
                message: format!("type `{}` is incomplete", types.display(ty.ty)),
                span: None,
            })?;
            boundary_scalar(
                types,
                QualifiedType::unqualified(body.underlying),
                config,
                boundary,
            )
        }
        Some(TypeKind::Array(_) | TypeKind::Record(_)) => Err(AbiError {
            code: "CCC3508",
            message: format!(
                "aggregate {boundary} type `{}` requires aggregate ABI classification",
                types.display(ty.ty)
            ),
            span: None,
        }),
        Some(TypeKind::Function(_)) => Err(AbiError {
            code: "CCC3501",
            message: format!(
                "function {boundary} type `{}` must be adjusted to a pointer before ABI planning",
                types.display(ty.ty)
            ),
            span: None,
        }),
        None => Err(AbiError {
            code: "CCC3501",
            message: format!("unknown type {} has no scalar ABI plan", ty.ty.index()),
            span: None,
        }),
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
        BuiltinType::Void => Err(AbiError {
            code: "CCC3507",
            message: "`void` has no scalar ABI representation".to_owned(),
            span: None,
        }),
        BuiltinType::LongDouble => Err(AbiError {
            code: "CCC3509",
            message: format!(
                "native `long double` {boundary} type `{}` requires the target long-double bridge capability",
                types.display(ty)
            ),
            span: None,
        }),
        BuiltinType::Float => {
            require_float_layout(types, ty, config, 32)?;
            Ok(AbiScalar::Float32)
        }
        BuiltinType::Double => {
            require_float_layout(types, ty, config, 64)?;
            Ok(AbiScalar::Float64)
        }
        BuiltinType::Bool
        | BuiltinType::UnsignedChar
        | BuiltinType::UnsignedShort
        | BuiltinType::UnsignedInt
        | BuiltinType::UnsignedLong
        | BuiltinType::UnsignedLongLong => integer_scalar(types, ty, false, config),
        BuiltinType::Char => {
            integer_scalar(types, ty, config.target.data_layout.char_is_signed, config)
        }
        BuiltinType::SignedChar
        | BuiltinType::Short
        | BuiltinType::Int
        | BuiltinType::Long
        | BuiltinType::LongLong => integer_scalar(types, ty, true, config),
    }
}

fn integer_scalar(
    types: &TypeStore,
    ty: TypeId,
    signed: bool,
    config: &EffectiveCompilationConfig,
) -> Result<AbiScalar, AbiError> {
    let layout = target_layout(types, ty, config)?;
    let bits = layout_bits(layout.size, "integer")?;
    if signed {
        Ok(AbiScalar::SignedInteger { bits })
    } else {
        Ok(AbiScalar::UnsignedInteger { bits })
    }
}

fn require_float_layout(
    types: &TypeStore,
    ty: TypeId,
    config: &EffectiveCompilationConfig,
    expected_bits: u8,
) -> Result<(), AbiError> {
    let layout = target_layout(types, ty, config)?;
    let actual = layout_bits(layout.size, "floating-point")?;
    if actual != expected_bits {
        return Err(AbiError {
            code: "CCC3502",
            message: format!(
                "type `{}` has {actual}-bit storage, expected {expected_bits}-bit ABI storage",
                types.display(ty)
            ),
            span: None,
        });
    }
    Ok(())
}

fn target_layout(
    types: &TypeStore,
    ty: TypeId,
    config: &EffectiveCompilationConfig,
) -> Result<ccc_types::TypeLayout, AbiError> {
    types.layout_of(ty, config).map_err(|error| AbiError {
        code: "CCC3502",
        message: format!("type `{}` has no target layout: {error}", types.display(ty)),
        span: None,
    })
}

fn layout_bits(size: u64, class: &str) -> Result<u8, AbiError> {
    size.checked_mul(8)
        .and_then(|bits| u8::try_from(bits).ok())
        .ok_or_else(|| AbiError {
            code: "CCC3503",
            message: format!("{class} ABI width is too large"),
            span: None,
        })
}

#[cfg(test)]
mod tests {
    use ccc_types::{ArrayLength, ArrayType, Field, RecordKind};

    use super::*;

    fn signature(types: &mut TypeStore, result: TypeId, parameters: Vec<TypeId>) -> TypeId {
        types.function_type(FunctionType::prototype(
            result,
            parameters
                .into_iter()
                .map(QualifiedType::unqualified)
                .collect(),
        ))
    }

    fn direct_plan(types: &TypeStore, signature: TypeId) -> Result<FunctionPlan, AbiError> {
        plan_function_type(types, signature, &EffectiveCompilationConfig::default())
    }

    #[test]
    fn plans_scalar_system_v_signatures() {
        let mut types = TypeStore::default();
        let signature = signature(&mut types, TypeId::INT, vec![TypeId::INT, TypeId::INT]);
        let plan = direct_plan(&types, signature).unwrap();
        assert_eq!(
            plan,
            FunctionPlan {
                calling_convention: CallingConvention::SystemV,
                parameters: vec![AbiScalar::SignedInteger { bits: 32 }; 2],
                result: Some(AbiScalar::SignedInteger { bits: 32 }),
            }
        );
    }

    #[test]
    fn plans_every_integer_width_and_signedness() {
        let mut types = TypeStore::default();
        let parameters = vec![
            TypeId::BOOL,
            TypeId::CHAR,
            TypeId::SIGNED_CHAR,
            TypeId::UNSIGNED_CHAR,
            TypeId::SHORT,
            TypeId::UNSIGNED_SHORT,
            TypeId::INT,
            TypeId::UNSIGNED_INT,
            TypeId::LONG,
            TypeId::UNSIGNED_LONG,
            TypeId::LONG_LONG,
            TypeId::UNSIGNED_LONG_LONG,
        ];
        let signature = signature(&mut types, TypeId::UNSIGNED_INT, parameters);
        let plan = direct_plan(&types, signature).unwrap();
        assert_eq!(
            plan.parameters,
            [
                AbiScalar::UnsignedInteger { bits: 8 },
                AbiScalar::SignedInteger { bits: 8 },
                AbiScalar::SignedInteger { bits: 8 },
                AbiScalar::UnsignedInteger { bits: 8 },
                AbiScalar::SignedInteger { bits: 16 },
                AbiScalar::UnsignedInteger { bits: 16 },
                AbiScalar::SignedInteger { bits: 32 },
                AbiScalar::UnsignedInteger { bits: 32 },
                AbiScalar::SignedInteger { bits: 64 },
                AbiScalar::UnsignedInteger { bits: 64 },
                AbiScalar::SignedInteger { bits: 64 },
                AbiScalar::UnsignedInteger { bits: 64 },
            ]
        );
        assert_eq!(plan.result, Some(AbiScalar::UnsignedInteger { bits: 32 }));
    }

    #[test]
    fn plans_pointers_floats_enums_and_void_results() {
        let mut types = TypeStore::default();
        let pointer = types.pointer(TypeId::INT);
        let (enum_id, enumeration) = types.declare_enum(Some("choice".to_owned()));
        types
            .complete_enum(enum_id, TypeId::UNSIGNED_INT, Vec::new())
            .unwrap();
        let signature = signature(
            &mut types,
            TypeId::VOID,
            vec![pointer, TypeId::FLOAT, TypeId::DOUBLE, enumeration],
        );
        let plan = direct_plan(&types, signature).unwrap();
        assert_eq!(
            plan.parameters,
            [
                AbiScalar::Pointer { bits: 64 },
                AbiScalar::Float32,
                AbiScalar::Float64,
                AbiScalar::UnsignedInteger { bits: 32 },
            ]
        );
        assert_eq!(plan.result, None);
    }

    #[test]
    fn rejects_aggregate_boundaries() {
        let mut types = TypeStore::default();
        let (record_id, record) = types.declare_record(RecordKind::Struct, None);
        types
            .complete_record(record_id, vec![Field::named("value", TypeId::INT)])
            .unwrap();
        let array = types.array(ArrayType {
            element: TypeId::INT.into(),
            length: ArrayLength::Constant(2),
        });
        for aggregate in [record, array] {
            let signature = signature(&mut types, TypeId::INT, vec![aggregate]);
            let error = direct_plan(&types, signature).unwrap_err();
            assert_eq!(error.code, "CCC3508");
            assert!(error.message.contains("aggregate parameter"));
        }
        let signature = signature(&mut types, record, Vec::new());
        let error = direct_plan(&types, signature).unwrap_err();
        assert_eq!(error.code, "CCC3508");
        assert!(error.message.contains("aggregate return"));
    }

    #[test]
    fn rejects_variadic_unspecified_and_long_double_boundaries() {
        let mut types = TypeStore::default();
        let variadic = types.function_type(FunctionType::variadic(TypeId::INT, Vec::new()));
        assert_eq!(direct_plan(&types, variadic).unwrap_err().code, "CCC3510");

        let unspecified = types.function_type(FunctionType::unspecified(TypeId::INT));
        assert_eq!(
            direct_plan(&types, unspecified).unwrap_err().code,
            "CCC3506"
        );

        let parameter = signature(&mut types, TypeId::INT, vec![TypeId::LONG_DOUBLE]);
        let error = direct_plan(&types, parameter).unwrap_err();
        assert_eq!(error.code, "CCC3509");
        assert!(error.message.contains("long double` parameter type"));
        let result = signature(&mut types, TypeId::LONG_DOUBLE, Vec::new());
        let error = direct_plan(&types, result).unwrap_err();
        assert_eq!(error.code, "CCC3509");
        assert!(error.message.contains("long double` return type"));
    }

    #[test]
    fn rejects_non_function_and_void_parameter_types() {
        let mut types = TypeStore::default();
        assert_eq!(
            direct_plan(&types, TypeId::INT).unwrap_err().code,
            "CCC3505"
        );
        let signature = signature(&mut types, TypeId::INT, vec![TypeId::VOID]);
        assert_eq!(direct_plan(&types, signature).unwrap_err().code, "CCC3507");
    }
}
