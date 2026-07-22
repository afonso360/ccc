//! Cranelift lowering and ELF object emission for CCC-IR.

pub mod generic;

use std::fmt;

use ccc_session::{SourceMap, Span};
use ccc_target::{CallingConvention, EffectiveCompilationConfig};
use cranelift_codegen::ir::{self, AbiParam, ArgumentPurpose, Signature};
use cranelift_codegen::isa::CallConv;

pub use generic::emit;

#[derive(Clone, Debug)]
pub struct Output {
    pub object: Vec<u8>,
    pub clif: String,
    pub assemblies: Vec<ccc_link::bridge::GeneratedAssembly>,
    pub manifest: ccc_link::artifact::BridgeManifestV2,
}

impl Output {
    pub fn into_artifact_bundle(self) -> ccc_link::artifact::ArtifactBundle {
        ccc_link::artifact::ArtifactBundle::new(self.object, self.assemblies, self.manifest)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Options<'a> {
    pub emit_clif: bool,
    /// Emit source-level DWARF using this compilation's source map.
    ///
    /// Keeping the map in the invocation options makes it impossible to emit
    /// line information from stale or reconstructed file identities.
    pub debug_info: Option<&'a SourceMap>,
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
    let enabled = ccc_target::TargetSpec::enabled(config.target.triple.clone())?;
    if enabled.abi != config.target.abi || enabled.data_layout != config.target.data_layout {
        return Err(format!(
            "target `{}` does not match its enabled ABI and data-layout profile",
            config.target.triple
        ));
    }
    let pointer_width = config
        .target
        .pointer_width()
        .ok_or_else(|| "the configured target has an unknown pointer width".to_owned())?;
    if pointer_width != 64 {
        return Err(format!(
            "pointer width {} is incompatible with the enabled 64-bit object backends",
            pointer_width
        ));
    }
    if config.target.int_width() != Some(32)
        || config.target.data_layout.int_width != 32
        || config.target.data_layout.long_width != 64
        || config.target.data_layout.pointer_width != 64
    {
        return Err(format!(
            "target `{}` is outside the enabled LP64 profiles",
            config.target.triple
        ));
    }
    config.validate_target_profile_options()?;
    Ok(())
}

pub(crate) fn signature(plan: &ccc_abi::NativeBoundaryPlan) -> Result<Signature, String> {
    let call_conv = match plan.calling_convention {
        CallingConvention::SystemV => CallConv::SystemV,
        CallingConvention::AppleAarch64 => CallConv::AppleAarch64,
        convention => {
            return Err(format!(
                "calling convention `{convention:?}` is unsupported by this backend"
            ));
        }
    };
    let mut signature = Signature::new(call_conv);
    for parameter in &plan.clif_parameters {
        signature.params.push(abi_parameter(parameter)?);
    }
    for result in &plan.clif_results {
        signature.returns.push(abi_parameter(result)?);
    }
    Ok(signature)
}

fn abi_parameter(carrier: &ccc_abi::NativeCarrierPlan) -> Result<AbiParam, String> {
    let ty = match carrier.carrier {
        ccc_abi::AbiCarrier::I8 => ir::types::I8,
        ccc_abi::AbiCarrier::I16 => ir::types::I16,
        ccc_abi::AbiCarrier::I32 => ir::types::I32,
        ccc_abi::AbiCarrier::I64 => ir::types::I64,
        ccc_abi::AbiCarrier::I128 => ir::types::I128,
        ccc_abi::AbiCarrier::F16 => ir::types::F16,
        ccc_abi::AbiCarrier::F32 => ir::types::F32,
        ccc_abi::AbiCarrier::F64 => ir::types::F64,
        ccc_abi::AbiCarrier::V32 => ir::types::I8X4,
        ccc_abi::AbiCarrier::V64 => ir::types::I8X8,
    };
    let purpose = match carrier.purpose {
        ccc_abi::NativePurpose::Normal => ArgumentPurpose::Normal,
        ccc_abi::NativePurpose::StructArgument(size) => ArgumentPurpose::StructArgument(size),
        ccc_abi::NativePurpose::IndirectArgument | ccc_abi::NativePurpose::Padding => {
            ArgumentPurpose::Normal
        }
        ccc_abi::NativePurpose::StructReturn => ArgumentPurpose::StructReturn,
    };
    let parameter = AbiParam::special(ty, purpose);
    Ok(match carrier.extension {
        ccc_abi::IntegerExtension::None => parameter,
        ccc_abi::IntegerExtension::Signed => parameter.sext(),
        ccc_abi::IntegerExtension::Unsigned => parameter.uext(),
    })
}

pub(crate) fn opaque_native_plan(
    calling_convention: CallingConvention,
) -> ccc_abi::NativeBoundaryPlan {
    ccc_abi::NativeBoundaryPlan {
        calling_convention,
        parameters: Vec::new(),
        result: ccc_abi::NativeResultPlan::Void,
        clif_parameters: Vec::new(),
        clif_results: Vec::new(),
        variadic: false,
    }
}
