//! Narrow compatibility boundary for the upstream Cranelift API.
//!
//! Backend lifecycle, settings, and small API-shape changes belong here.
//! Apart from the explicit compatibility shims below, instruction selection
//! remains in `function`: hiding CLIF semantics would obscure lowering and
//! risk duplicating Cranelift's own work.

use ccc_target::{EffectiveCompilationConfig, OptimizationLevel, RelocationModel};
use cranelift_codegen::Context;
use cranelift_codegen::ir::{self, InstBuilder, MemFlagsData, UserFuncName};
use cranelift_codegen::isa;
use cranelift_codegen::settings::{self, Configurable as _};
use cranelift_frontend::FunctionBuilder;
use cranelift_module::{DataDescription, Module as _, default_libcall_names};
use cranelift_object::{ObjectBuilder, ObjectModule};

use super::{CodegenError, module_error};

pub(super) type FrontendConfig = isa::TargetFrontendConfig;

pub(super) fn object_module(
    config: &EffectiveCompilationConfig,
    preserve_frame_pointers: bool,
) -> Result<ObjectModule, CodegenError> {
    let mut isa_builder = isa::lookup(config.target.triple.clone()).map_err(module_error)?;
    if config.target.abi == ccc_target::AbiIdentity::RiscvLp64d {
        for extension in [
            "has_m",
            "has_a",
            "has_f",
            "has_d",
            "has_zicsr",
            "has_zifencei",
        ] {
            isa_builder.enable(extension).map_err(module_error)?;
        }
    }

    let mut flag_builder = settings::builder();
    flag_builder
        .set("opt_level", optimization_level(config.optimization))
        .map_err(module_error)?;
    match config.relocation_model {
        RelocationModel::Static => flag_builder.set("is_pic", "false").map_err(module_error)?,
        RelocationModel::Pic | RelocationModel::Pie => {
            flag_builder.set("is_pic", "true").map_err(module_error)?
        }
    }
    flag_builder
        .set(
            "enable_llvm_abi_extensions",
            if config.target.abi.supports_int128_values() {
                "true"
            } else {
                "false"
            },
        )
        .map_err(module_error)?;
    flag_builder
        .set("enable_multi_ret_implicit_sret", "false")
        .map_err(module_error)?;
    flag_builder
        .set("unwind_info", "true")
        .map_err(module_error)?;
    if preserve_frame_pointers {
        flag_builder
            .set("preserve_frame_pointers", "true")
            .map_err(module_error)?;
    }

    let isa = isa_builder
        .finish(settings::Flags::new(flag_builder))
        .map_err(module_error)?;
    let builder = ObjectBuilder::new(isa, "ccc", default_libcall_names()).map_err(module_error)?;
    Ok(ObjectModule::new(builder))
}

pub(super) fn function_context(function: u32, signature: ir::Signature) -> Context {
    let mut context = Context::new();
    context.func = ir::Function::with_name_signature(UserFuncName::user(0, function), signature);
    context
}

pub(super) fn frontend_config(module: &ObjectModule) -> FrontendConfig {
    module.isa().frontend_config()
}

pub(super) fn finalize_frontend(builder: FunctionBuilder<'_>, frontend_config: FrontendConfig) {
    builder.finalize(frontend_config);
}

pub(super) const fn empty_memory_flags() -> MemFlagsData {
    MemFlagsData::new()
}

pub(super) fn materialize_symbol(
    builder: &mut FunctionBuilder<'_>,
    ty: ir::Type,
    symbol: ir::GlobalValue,
) -> ir::Value {
    builder.ins().symbol_value(ty, symbol)
}

pub(super) fn set_custom_data_section(description: &mut DataDescription, section: &str) {
    description.set_custom_section(section);
}

pub(super) const fn optimization_level(optimization: OptimizationLevel) -> &'static str {
    match optimization {
        OptimizationLevel::O0 => "none",
        OptimizationLevel::O1 | OptimizationLevel::O2 | OptimizationLevel::O3 => "speed",
        OptimizationLevel::Size | OptimizationLevel::SizeMin => "speed_and_size",
    }
}
