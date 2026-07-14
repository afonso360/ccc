//! Cranelift lowering and ELF object emission for CCC-IR.

pub mod generic;

use std::fmt;

use ccc_abi::AbiScalar;
use ccc_session::Span;
use ccc_target::{Architecture, BinaryFormat, CallingConvention, EffectiveCompilationConfig};
use cranelift_codegen::ir::{self, AbiParam, Signature};
use cranelift_codegen::isa::CallConv;

pub use generic::emit;

#[derive(Clone, Debug)]
pub struct Output {
    pub object: Vec<u8>,
    pub clif: String,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Options {
    pub emit_clif: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CodegenError {
    pub code: &'static str,
    pub message: String,
    pub span: Option<Span>,
}

impl CodegenError {
    pub fn with_span_if_none(mut self, span: Span) -> Self {
        self.span.get_or_insert(span);
        self
    }
}

impl fmt::Display for CodegenError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.message.fmt(formatter)
    }
}

impl std::error::Error for CodegenError {}

pub(crate) fn validate_target(config: &EffectiveCompilationConfig) -> Result<(), String> {
    if config.target.triple.binary_format != BinaryFormat::Elf {
        return Err("the configured object format is unsupported".to_owned());
    }
    if config.target.triple.architecture != Architecture::X86_64 {
        return Err(format!(
            "architecture `{}` is incompatible with the x86-64 object backend",
            config.target.triple.architecture
        ));
    }
    let pointer_width = config
        .target
        .pointer_width()
        .ok_or_else(|| "the configured target has an unknown pointer width".to_owned())?;
    if pointer_width != 64 {
        return Err(format!(
            "pointer width {} is incompatible with the x86-64 object backend",
            pointer_width
        ));
    }
    Ok(())
}

pub(crate) fn signature(plan: &ccc_abi::FunctionPlan) -> Result<Signature, String> {
    let call_conv = match plan.calling_convention {
        CallingConvention::SystemV => CallConv::SystemV,
        convention => {
            return Err(format!(
                "calling convention `{convention:?}` is unsupported by this backend"
            ));
        }
    };
    let mut signature = Signature::new(call_conv);
    for parameter in &plan.parameters {
        signature.params.push(abi_parameter(*parameter)?);
    }
    if let Some(result) = plan.result {
        signature.returns.push(abi_parameter(result)?);
    }
    Ok(signature)
}

fn abi_parameter(scalar: AbiScalar) -> Result<AbiParam, String> {
    match scalar {
        AbiScalar::SignedInteger { bits } => integer_abi_parameter(bits, true),
        AbiScalar::UnsignedInteger { bits } => integer_abi_parameter(bits, false),
        AbiScalar::Pointer { bits } => Ok(AbiParam::new(integer_type(bits, "pointer")?)),
        AbiScalar::Float32 => Ok(AbiParam::new(ir::types::F32)),
        AbiScalar::Float64 => Ok(AbiParam::new(ir::types::F64)),
    }
}

fn integer_abi_parameter(bits: u8, signed: bool) -> Result<AbiParam, String> {
    let parameter = AbiParam::new(integer_type(bits, "integer")?);
    Ok(if bits < 32 {
        if signed {
            parameter.sext()
        } else {
            parameter.uext()
        }
    } else {
        parameter
    })
}

fn integer_type(bits: u8, class: &str) -> Result<ir::Type, String> {
    match bits {
        8 => Ok(ir::types::I8),
        16 => Ok(ir::types::I16),
        32 => Ok(ir::types::I32),
        64 => Ok(ir::types::I64),
        _ => Err(format!("unsupported {class} ABI width {bits}")),
    }
}
