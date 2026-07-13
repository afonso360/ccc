//! Immutable scalar call plans for the enabled C ABI.

use std::fmt;

use ccc_ir::Module;
use ccc_target::{CallingConvention, EffectiveCompilationConfig};
use ccc_types::{TypeId, TypeKind};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AbiScalar {
    SignedInteger { bits: u8 },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FunctionPlan {
    pub calling_convention: CallingConvention,
    pub parameters: Vec<AbiScalar>,
    pub result: AbiScalar,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModulePlan {
    pub functions: Vec<FunctionPlan>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AbiError {
    pub code: &'static str,
    pub message: String,
}

impl fmt::Display for AbiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.message.fmt(formatter)
    }
}

impl std::error::Error for AbiError {}

pub fn plan(module: &Module, config: &EffectiveCompilationConfig) -> Result<ModulePlan, AbiError> {
    let calling_convention = config.target.calling_convention().ok_or_else(|| AbiError {
        code: "CCC3504",
        message: format!(
            "target `{}` does not define a C calling convention",
            config.target.triple
        ),
    })?;
    let functions = module
        .functions
        .iter()
        .map(|function| {
            Ok(FunctionPlan {
                calling_convention,
                parameters: function
                    .parameter_types
                    .iter()
                    .map(|ty| scalar(module, *ty, config))
                    .collect::<Result<_, _>>()?,
                result: scalar(module, function.result_type, config)?,
            })
        })
        .collect::<Result<_, _>>()?;
    Ok(ModulePlan { functions })
}

fn scalar(
    module: &Module,
    ty: TypeId,
    config: &EffectiveCompilationConfig,
) -> Result<AbiScalar, AbiError> {
    if !matches!(module.types.kind(ty), TypeKind::Int) {
        return Err(AbiError {
            code: "CCC3501",
            message: format!("type `{}` has no scalar ABI plan", module.types.display(ty)),
        });
    }
    let layout = module.types.layout(ty, config).ok_or_else(|| AbiError {
        code: "CCC3502",
        message: format!("type `{}` has no target layout", module.types.display(ty)),
    })?;
    let bits = u8::try_from(layout.size * 8).map_err(|_| AbiError {
        code: "CCC3503",
        message: "integer ABI width is too large".to_owned(),
    })?;
    Ok(AbiScalar::SignedInteger { bits })
}

#[cfg(test)]
mod tests {
    use ccc_ir::lower;
    use ccc_pp::lex;
    use ccc_sema::analyze;
    use ccc_session::SourceMap;
    use ccc_syntax::{convert_pp_tokens, parse};

    use super::*;

    #[test]
    fn plans_scalar_system_v_signatures() {
        let mut sources = SourceMap::new();
        let file = sources.add_file("test.c", "int add(int a, int b) { return a + b; }");
        let tokens = convert_pp_tokens(lex(file, sources.source(file).unwrap()).unwrap());
        let module = lower(&analyze(&parse(&tokens).unwrap()).unwrap()).unwrap();
        let plans = plan(&module, &EffectiveCompilationConfig::default()).unwrap();
        assert_eq!(
            plans.functions[0],
            FunctionPlan {
                calling_convention: CallingConvention::SystemV,
                parameters: vec![AbiScalar::SignedInteger { bits: 32 }; 2],
                result: AbiScalar::SignedInteger { bits: 32 },
            }
        );
    }
}
