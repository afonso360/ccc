//! Cranelift lowering for the typed control-flow IR.

mod data;
mod debug;
mod function;
#[cfg(test)]
mod tests;
mod unwind;

use std::collections::{HashMap, HashSet};

use ccc_ir::generic as gir;
use ccc_sema::generic::{
    Linkage as CLinkage, ObjectDefinitionPolicy, StorageDuration, SymbolBinding, SymbolVisibility,
};
use ccc_target::{
    AbiIdentity, BinaryFormat, EffectiveCompilationConfig, LongDoubleFormat, OptimizationLevel,
    RelocationModel, RuntimeHelperContract, RuntimeHelperValue,
};
use ccc_types::{
    BuiltinType, LayoutShape, QualifiedType, TypeId, TypeKind, TypeQualifiers, TypeStore,
};
use cranelift_codegen::Context;
use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_codegen::ir::immediates::{Ieee16, Ieee32, Ieee64};
use cranelift_codegen::ir::{
    self, BlockArg, InstBuilder, MemFlagsData, StackSlot, StackSlotData, StackSlotKind, TrapCode,
    UserFuncName,
};
use cranelift_codegen::isa;
use cranelift_codegen::settings::{self, Configurable as _};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_module::{
    DataDescription, DataId as ClifDataId, FuncId, Linkage, Module as _, default_libcall_names,
};
use cranelift_object::{ObjectBuilder, ObjectModule};
use object::read::{Object as _, ObjectSymbol as _};
use object::write::SymbolSection;
use object::{FileFlags, SymbolFlags, SymbolKind, SymbolScope};

use crate::{CodegenError, Options, Output};

const BACKEND_ERROR: &str = "CCC4002";
const ATOMIC_ERROR: &str = "CCC4011";

fn f64_to_f16_bits(value: f64) -> u16 {
    let raw = value.to_bits();
    let sign = ((raw >> 48) & 0x8000) as u16;
    let magnitude = raw & 0x7fff_ffff_ffff_ffff;
    let exponent = ((magnitude >> 52) & 0x7ff) as i32;
    let fraction = magnitude & 0x000f_ffff_ffff_ffff;

    let rounded = if exponent == 0x7ff {
        if fraction == 0 {
            0x7c00
        } else {
            let payload = (fraction >> 42) as u16;
            0x7c00 | payload | u16::from(payload == 0)
        }
    } else {
        let unbiased = exponent - 1023;
        if unbiased > 15 {
            0x7c00
        } else if unbiased >= -14 {
            let base = (((unbiased + 15) as u64) << 10) | (fraction >> 42);
            let remainder = fraction & ((1u64 << 42) - 1);
            let halfway = 1u64 << 41;
            let increment = remainder > halfway || remainder == halfway && base & 1 != 0;
            (base + u64::from(increment)).min(0x7c00) as u16
        } else if unbiased >= -25 {
            let significand = fraction | (1u64 << 52);
            let shift = (28 - unbiased) as u32;
            let base = significand >> shift;
            let remainder = significand & ((1u64 << shift) - 1);
            let halfway = 1u64 << (shift - 1);
            let increment = remainder > halfway || remainder == halfway && base & 1 != 0;
            (base + u64::from(increment)) as u16
        } else {
            0
        }
    };
    sign | rounded
}

/// Emits an ELF object from the typed control-flow IR.
///
/// Function ABI plans are derived from each canonical function type, so the
/// same stable diagnostics are used for declarations, definitions, direct
/// calls, and indirect calls.
pub fn emit(
    module: &gir::FullModule,
    config: &EffectiveCompilationConfig,
    options: Options<'_>,
) -> Result<Output, CodegenError> {
    let plan = ccc_abi::plan_module(module, config).map_err(abi_error)?;
    emit_with_plan(module, config, &plan, options)
}

pub fn emit_with_plan(
    module: &gir::FullModule,
    config: &EffectiveCompilationConfig,
    plan: &ccc_abi::ModuleAbiPlan,
    options: Options<'_>,
) -> Result<Output, CodegenError> {
    let verified = plan.verify_against(module, config).map_err(abi_error)?;
    emit_inner(module, config, verified, options)
}

fn emit_inner(
    module: &gir::FullModule,
    config: &EffectiveCompilationConfig,
    abi_plan: ccc_abi::VerifiedModuleAbiPlan<'_>,
    options: Options<'_>,
) -> Result<Output, CodegenError> {
    super::validate_target(config).map_err(error)?;
    if !config.target.abi.supports_tls_codegen()
        && module.globals.iter().any(|global| {
            global.duration == StorageDuration::Thread || global.emission.tls.is_some()
        })
    {
        return Err(CodegenError {
            code: "CCC3522",
            message: format!(
                "thread-local storage has no enabled object and link contract for target ABI `{}`",
                config.target.abi.name()
            ),
            span: None,
        });
    }
    let mut isa_builder = isa::lookup(config.target.triple.clone()).map_err(module_error)?;
    if config.target.abi == AbiIdentity::RiscvLp64d {
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
        .set("opt_level", cranelift_opt_level(config.optimization))
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
    if module_contains_returns_twice_call(module) || options.debug_info.is_some() {
        // Resuming a saved environment requires a stable native frame. Scalar
        // locals are already forced to stack storage by IR lowering; retain
        // the frame pointer as the matching machine-code side of that contract.
        flag_builder
            .set("preserve_frame_pointers", "true")
            .map_err(module_error)?;
    }
    let isa = isa_builder
        .finish(settings::Flags::new(flag_builder))
        .map_err(module_error)?;
    let object_builder =
        ObjectBuilder::new(isa, "ccc", default_libcall_names()).map_err(module_error)?;
    let mut object_module = ObjectModule::new(object_builder);
    let mut unwind = unwind::UnwindEmitter::new(object_module.isa()).map_err(error)?;

    // Revalidation at the code-generation boundary prevents a stale plan from
    // reaching declaration or bridge materialization even in release builds.
    let abi_plan = abi_plan
        .plan()
        .verify_against(module, config)
        .map_err(abi_error)?;
    let declarations = declare_module(module, config, abi_plan, &mut object_module)?;
    let mut debug = options
        .debug_info
        .map(|sources| debug::DebugEmitter::new(module, config, sources));

    let mut clif = String::new();
    for (function_index, function) in module.functions.iter().enumerate() {
        let Some(_) = function.entry else {
            continue;
        };
        let signature = declarations
            .definition_signatures
            .get(&function.id.0)
            .cloned()
            .ok_or_else(|| error(format!("function {} has no ABI signature", function.id.0)))?;
        let mut context = Context::new();
        context.func =
            ir::Function::with_name_signature(UserFuncName::user(0, function.id.0), signature);
        let frontend_config = object_module.isa().frontend_config();
        let references = function::FunctionReferences::new(&declarations, &mut object_module);
        let definition_plan = abi_plan
            .plan()
            .definitions
            .get(&function.id)
            .ok_or_else(|| error(format!("function {} has no module ABI plan", function.id.0)))?;
        let definition_plan = match &definition_plan.boundary {
            ccc_abi::BoundaryPlan::Native(plan) => function::DefinitionAbi::Native(plan),
            ccc_abi::BoundaryPlan::Bridge(plan) => function::DefinitionAbi::Variadic(plan),
        };
        let debug_layout = function::lower_function(
            module,
            function,
            config,
            abi_plan,
            definition_plan,
            &references,
            frontend_config,
            &mut context.func,
            debug.as_mut().map(|emitter| &mut emitter.locations),
        )
        .map_err(|error| error.with_span_if_none(function.span))?;
        drop(references);
        if options.emit_clif {
            clif.push_str(&format!(
                "; function {}\n{}\n",
                function.symbol_name, context.func
            ));
        }
        let id = declarations
            .definition_functions
            .get(&function.id.0)
            .copied()
            .ok_or_else(|| error(format!("function {} was not declared", function.id.0)))?;
        if let Err(errors) = cranelift_codegen::verify_function(&context.func, object_module.isa())
        {
            let details =
                cranelift_codegen::print_errors::pretty_verifier_error(&context.func, None, errors);
            return Err(error(format!(
                "Cranelift verifier rejected `{}`:\n{details}",
                function.symbol_name
            )));
        }
        object_module
            .define_function(id, &mut context)
            .map_err(module_error)?;
        if let Some(debug) = debug.as_mut() {
            debug.record_function(
                function_index,
                id,
                &context,
                &debug_layout,
                object_module.isa(),
            )?;
        }
        unwind
            .record_function(id, &context, object_module.isa())
            .map_err(error)?;
    }

    // Define code before data so Mach-O's `__text` section is created first.
    // Apple's linker derives compact-unwind records from `.eh_frame`; when a
    // data section precedes `__text`, its relocatable-link pass can associate
    // a section-relative FDE with data after reordering the sections.  Data
    // references only require declarations while functions are lowered, so
    // deferring their definitions preserves those references and gives the
    // object the conventional text-before-data section order.
    data::define_strings(module, &declarations, &mut object_module)?;
    data::define_globals(module, config, &declarations, &mut object_module)?;

    let mut product = object_module.finish();
    if config.target.abi == AbiIdentity::RiscvLp64d {
        let FileFlags::Elf { e_flags, .. } = &mut product.object.flags else {
            return Err(error(
                "RISC-V LP64D code generation did not produce an ELF object",
            ));
        };
        // The enabled RISC-V profile is rv64gc/lp64d. Cranelift records the
        // floating-point ABI but does not propagate the C extension into the
        // writable object's ELF header. Generated bridge assembly is allowed
        // to use compressed instructions, so every constituent object must
        // advertise the complete profile before relocatable linking.
        *e_flags |= object::elf::EF_RISCV_RVC | object::elf::EF_RISCV_FLOAT_ABI_DOUBLE;
    }
    if config.target.abi == AbiIdentity::DarwinArm64 {
        product
            .object
            .set_macho_build_version(darwin_build_version(config)?);

        // `object` applies Mach-O's ordinary C leading-underscore mangling to
        // every function and data declaration.  A declaration assembly label
        // is different: its string is already the exact physical symbol name,
        // as required by Apple's SDK redirects.  Relocations refer to symbol
        // IDs, so restoring the spelling after module finalization preserves
        // both defined and undefined references without guessing from the
        // label's contents.
        for function in module
            .functions
            .iter()
            .filter(|function| function.symbol_name_is_exact)
        {
            let Some(id) = declarations.functions.get(&function.id.0).copied() else {
                continue;
            };
            product.object.symbol_mut(product.function_symbol(id)).name =
                function.symbol_name.as_bytes().to_vec();
        }
        for global in module
            .globals
            .iter()
            .filter(|global| global.emission.symbol_name_is_exact)
        {
            let Some(declaration) = declarations.globals.get(&global.id.0) else {
                continue;
            };
            product
                .object
                .symbol_mut(product.data_symbol(declaration.id))
                .name = global.emission.symbol_name.as_bytes().to_vec();
        }
    }
    for common in &declarations.commons {
        let symbol = product.data_symbol(common.id);
        let symbol = product.object.symbol_mut(symbol);
        symbol.section = SymbolSection::Common;
        symbol.value = common.align;
        symbol.size = common.size;
        symbol.kind = SymbolKind::Data;
        symbol.scope = SymbolScope::Dynamic;
        symbol.weak = false;
    }
    for function in module
        .functions
        .iter()
        .filter(|function| function.linkage == CLinkage::External)
    {
        let Some(id) = declarations.functions.get(&function.id.0).copied() else {
            continue;
        };
        let symbol = product.function_symbol(id);
        let symbol = product.object.symbol_mut(symbol);
        symbol.weak = function.binding == SymbolBinding::Weak;
        if config.target.triple.binary_format == BinaryFormat::Elf {
            set_elf_symbol_visibility(symbol, function.visibility);
        } else if config.target.triple.binary_format == BinaryFormat::Macho {
            set_macho_symbol_visibility(symbol, function.visibility);
        }
    }
    for global in module
        .globals
        .iter()
        .filter(|global| global.linkage == CLinkage::External)
    {
        let Some(declaration) = declarations.globals.get(&global.id.0) else {
            continue;
        };
        let symbol = product.data_symbol(declaration.id);
        let symbol = product.object.symbol_mut(symbol);
        symbol.weak = global.emission.binding == SymbolBinding::Weak;
        if config.target.triple.binary_format == BinaryFormat::Elf {
            set_elf_symbol_visibility(symbol, global.emission.visibility);
        } else if config.target.triple.binary_format == BinaryFormat::Macho {
            set_macho_symbol_visibility(symbol, global.emission.visibility);
        }
    }
    unwind.emit(&mut product).map_err(error)?;
    if let Some(debug) = debug {
        debug.emit(&mut product, &declarations)?;
    }
    let object = product.emit().map_err(module_error)?;
    let required_runtime_helpers = required_runtime_helper_symbols(module, config);
    validate_runtime_helper_symbols(
        &object,
        module,
        config,
        required_runtime_helpers.iter().copied(),
    )?;
    let (assemblies, manifest) = generated_bridge_artifacts(
        module,
        config,
        abi_plan,
        &declarations.hidden_body_symbols,
        declarations.call_helper_symbol.as_deref(),
    )?;
    Ok(Output {
        object,
        clif,
        assemblies,
        manifest,
    })
}

const fn cranelift_opt_level(optimization: OptimizationLevel) -> &'static str {
    match optimization {
        OptimizationLevel::O0 => "none",
        OptimizationLevel::O1 | OptimizationLevel::O2 | OptimizationLevel::O3 => "speed",
        OptimizationLevel::Size | OptimizationLevel::SizeMin => "speed_and_size",
    }
}

fn module_contains_returns_twice_call(module: &gir::FullModule) -> bool {
    module.functions.iter().any(|function| {
        function.blocks.iter().any(|block| {
            block.instructions.iter().any(|instruction| {
                matches!(
                    &instruction.kind,
                    gir::FullInstructionKind::DirectCall { effects, .. }
                        | gir::FullInstructionKind::IndirectCall { effects, .. }
                        if effects.returns_twice
                )
            })
        })
    })
}

fn validate_runtime_helper_symbols<'a>(
    object_bytes: &[u8],
    module: &gir::FullModule,
    config: &EffectiveCompilationConfig,
    selected: impl IntoIterator<Item = &'a str>,
) -> Result<(), CodegenError> {
    let selected = selected.into_iter().collect::<HashSet<_>>();
    let manifest = config
        .target
        .abi
        .runtime_helper_manifest()
        .iter()
        .map(|entry| (entry.symbol, entry))
        .collect::<HashMap<_, _>>();
    let source_symbols = module
        .functions
        .iter()
        .map(|function| function.symbol_name.as_str())
        .collect::<HashSet<_>>();
    let object = object::File::parse(object_bytes).map_err(module_error)?;
    let undefined = object
        .symbols()
        .filter(|symbol| symbol.is_undefined())
        .filter_map(|symbol| symbol.name().ok())
        .collect::<HashSet<_>>();
    for symbol in &undefined {
        let looks_like_wide_helper = manifest.contains_key(symbol)
            || (symbol.starts_with("__")
                && (symbol.ends_with("ti2") || symbol.ends_with("ti3") || symbol.contains("tif")));
        if looks_like_wide_helper && !selected.contains(symbol) && !source_symbols.contains(symbol)
        {
            return Err(error(format!(
                "backend emitted undeclared wide-integer runtime helper `{symbol}`"
            )));
        }
    }
    for symbol in selected {
        let Some(contract) = manifest.get(symbol).copied() else {
            return Err(error(format!(
                "selected runtime helper `{symbol}` is absent from the target manifest"
            )));
        };
        if runtime_helper_uses_f80(contract) {
            // This reference is emitted by the generated x87 assembly unit,
            // not by the primary Cranelift object validated here. The
            // packaged object is inspected again by the final link planner.
            continue;
        }
        if !undefined.contains(symbol) && !source_symbols.contains(symbol) {
            return Err(error(format!(
                "selected runtime helper `{symbol}` is absent from the emitted object"
            )));
        }
    }
    Ok(())
}

fn darwin_build_version(
    config: &EffectiveCompilationConfig,
) -> Result<object::write::MachOBuildVersion, CodegenError> {
    // arm64 macOS first shipped with macOS 11.  Recording a real platform and
    // minimum version is mandatory: Apple's linker rejects Mach-O objects
    // whose LC_BUILD_VERSION uses PLATFORM_UNKNOWN (the value produced by a
    // target-lexicon `darwin` triple without this override).
    let deployment = config
        .normalized_deployment_target()
        .ok_or_else(|| error("Darwin build version requested for a non-Darwin target"))?;
    let (major, minor, patch) = parse_darwin_version(deployment)?;
    let mut version = object::write::MachOBuildVersion::default();
    version.platform = object::macho::PLATFORM_MACOS;
    version.minos = (u32::from(major) << 16) | (u32::from(minor) << 8) | u32::from(patch);
    // SDK zero is the conventional value for a relocatable object.  The
    // linker records the selected SDK in the final image.
    version.sdk = 0;
    Ok(version)
}

fn parse_darwin_version(version: &str) -> Result<(u16, u8, u8), CodegenError> {
    let mut components = version.split('.');
    let invalid = || {
        error(format!(
            "invalid Darwin deployment target `{version}`; expected MAJOR[.MINOR[.PATCH]]"
        ))
    };
    let major = components
        .next()
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or_else(invalid)?;
    let minor = components
        .next()
        .unwrap_or("0")
        .parse::<u8>()
        .map_err(|_| invalid())?;
    let patch = components
        .next()
        .unwrap_or("0")
        .parse::<u8>()
        .map_err(|_| invalid())?;
    if components.next().is_some() || major == 0 {
        return Err(invalid());
    }
    Ok((major, minor, patch))
}

fn generated_bridge_artifacts(
    module: &gir::FullModule,
    config: &EffectiveCompilationConfig,
    abi_plan: ccc_abi::VerifiedModuleAbiPlan<'_>,
    hidden_body_symbols: &HashMap<u32, String>,
    call_helper_symbol: Option<&str>,
) -> Result<
    (
        Vec<ccc_link::bridge::GeneratedAssembly>,
        ccc_link::artifact::BridgeManifestV2,
    ),
    CodegenError,
> {
    use ccc_link::artifact::{BridgeManifestV2, GeneratedSymbol, GeneratedSymbolOwner};
    use ccc_link::bridge::{
        AssemblyFunctionLinkage, BridgeEntryPlan, F80RuntimeHelperPlan, GeneratedSymbolKind,
        InlineAsmSupportPlan, TlsAccessModel, TlsAccessorPlan, TlsSymbolVisibility,
        render_f80_support, render_inline_asm_support, render_target_call_helper,
        render_target_fixed_entry, render_target_tls_accessor, render_target_variadic_entry,
    };

    let mut assemblies = Vec::new();
    let mut symbols = Vec::new();
    if let Some(helper) = call_helper_symbol {
        let assembly =
            render_target_call_helper(helper, config.target.abi).map_err(module_error)?;
        symbols.push(GeneratedSymbol::internal(
            helper,
            GeneratedSymbolKind::CallHelper,
            GeneratedSymbolOwner::AssemblyUnit(assembly.stem().to_owned()),
        ));
        assemblies.push(assembly);
    }
    if let Some(support) = &abi_plan.plan().artifacts.f80_support {
        let required = required_runtime_helper_symbols(module, config);
        let assembly = render_f80_support(
            &support.helper_symbol,
            F80RuntimeHelperPlan {
                from_i128: required.contains("__floattixf"),
                from_u128: required.contains("__floatuntixf"),
                to_i128: required.contains("__fixxfti"),
                to_u128: required.contains("__fixunsxfti"),
            },
        )
        .map_err(module_error)?;
        symbols.push(GeneratedSymbol::internal(
            &support.helper_symbol,
            GeneratedSymbolKind::Support,
            GeneratedSymbolOwner::AssemblyUnit(assembly.stem().to_owned()),
        ));
        assemblies.push(assembly);
    }
    if let Some(support) = &abi_plan.plan().artifacts.inline_asm_support {
        let assembly = render_inline_asm_support(&InlineAsmSupportPlan {
            cpuid_symbol: support.cpuid_symbol.clone(),
            rdtsc_symbol: support.rdtsc_symbol.clone(),
        })
        .map_err(module_error)?;
        let owner = GeneratedSymbolOwner::AssemblyUnit(assembly.stem().to_owned());
        for helper in support
            .cpuid_symbol
            .iter()
            .chain(support.rdtsc_symbol.iter())
        {
            symbols.push(GeneratedSymbol::internal(
                helper,
                GeneratedSymbolKind::Support,
                owner.clone(),
            ));
        }
        assemblies.push(assembly);
    }
    for (function, artifact) in &abi_plan.plan().artifacts.bridge_entries {
        let definition = abi_plan
            .plan()
            .definitions
            .get(function)
            .ok_or_else(|| error("bridge entry artifact has no definition plan"))?;
        let ccc_abi::BoundaryPlan::Bridge(plan) = &definition.boundary else {
            return Err(error(
                "bridge entry artifact references a native definition plan",
            ));
        };
        let hidden_body = hidden_body_symbols.get(&function.0).ok_or_else(|| {
            error(format!(
                "bridged function {} has no hidden body symbol",
                function.0
            ))
        })?;
        if hidden_body != &artifact.body_symbol {
            return Err(error(
                "declared bridge body symbol differs from the module ABI plan",
            ));
        }
        let public_symbol = &artifact.public_symbol;
        let linkage = if artifact.source_linkage != ccc_abi::SourceLinkage::External {
            AssemblyFunctionLinkage::Internal
        } else {
            match artifact.source_visibility {
                ccc_abi::SourceVisibility::Default => AssemblyFunctionLinkage::ExternalDefault,
                ccc_abi::SourceVisibility::Hidden => AssemblyFunctionLinkage::ExternalHidden,
                ccc_abi::SourceVisibility::Protected => AssemblyFunctionLinkage::ExternalProtected,
                ccc_abi::SourceVisibility::Internal => AssemblyFunctionLinkage::ExternalInternal,
            }
        };
        let gp_results = plan
            .result_pieces
            .iter()
            .filter(|piece| piece.piece.class == ccc_abi::AbiClass::Integer)
            .count() as u8;
        let xmm_results = plan
            .result_pieces
            .iter()
            .filter(|piece| {
                matches!(
                    piece.piece.class,
                    ccc_abi::AbiClass::Sse | ccc_abi::AbiClass::SseUp
                )
            })
            .count() as u8;
        let x87_result = plan
            .result_pieces
            .iter()
            .any(|piece| piece.piece.class == ccc_abi::AbiClass::X87);
        let entry_plan = BridgeEntryPlan {
            public_symbol: public_symbol.clone(),
            public_symbol_is_exact: artifact.public_symbol_is_exact,
            hidden_body_symbol: hidden_body.clone(),
            linkage,
            weak: artifact.source_binding == ccc_abi::SourceBinding::Weak,
            fixed_gp_used: plan.gp_used,
            fixed_sse_used: plan.xmm_used,
            overflow_arg_offset: plan.overflow_arg_offset,
            gp_results,
            xmm_results,
            x87_result,
            hidden_return: plan.hidden_return,
            logical_line: 1,
        };
        let assembly = match plan.kind {
            ccc_abi::BridgeKind::FixedEntry => {
                render_target_fixed_entry(&entry_plan, config.target.abi)
            }
            ccc_abi::BridgeKind::VariadicEntry => {
                render_target_variadic_entry(&entry_plan, config.target.abi)
            }
            ccc_abi::BridgeKind::FixedCall
            | ccc_abi::BridgeKind::VariadicCall
            | ccc_abi::BridgeKind::UnprototypedCall => Err(ccc_link::LinkError {
                code: "CCC5008",
                message: "a definition artifact has a call-side bridge kind".to_owned(),
            }),
        }
        .map_err(module_error)?;
        if artifact.kind != plan.kind {
            return Err(error(
                "bridge entry artifact kind differs from its definition plan",
            ));
        }
        let (entry_kind, body_kind) = match plan.kind {
            ccc_abi::BridgeKind::FixedEntry => (
                GeneratedSymbolKind::FixedEntry,
                GeneratedSymbolKind::FixedBody,
            ),
            ccc_abi::BridgeKind::VariadicEntry => (
                GeneratedSymbolKind::VariadicEntry,
                GeneratedSymbolKind::VariadicBody,
            ),
            _ => unreachable!(),
        };
        let mut entry_symbol = match linkage {
            AssemblyFunctionLinkage::ExternalDefault => GeneratedSymbol::public(
                public_symbol,
                entry_kind,
                GeneratedSymbolOwner::AssemblyUnit(assembly.stem().to_owned()),
            ),
            AssemblyFunctionLinkage::Internal => GeneratedSymbol::source_internal(
                public_symbol,
                entry_kind,
                GeneratedSymbolOwner::AssemblyUnit(assembly.stem().to_owned()),
            ),
            AssemblyFunctionLinkage::ExternalHidden => GeneratedSymbol::source_hidden(
                public_symbol,
                entry_kind,
                GeneratedSymbolOwner::AssemblyUnit(assembly.stem().to_owned()),
            ),
            AssemblyFunctionLinkage::ExternalProtected => GeneratedSymbol::source_protected(
                public_symbol,
                entry_kind,
                GeneratedSymbolOwner::AssemblyUnit(assembly.stem().to_owned()),
            ),
            AssemblyFunctionLinkage::ExternalInternal => GeneratedSymbol::source_elf_internal(
                public_symbol,
                entry_kind,
                GeneratedSymbolOwner::AssemblyUnit(assembly.stem().to_owned()),
            ),
        };
        if artifact.source_binding == ccc_abi::SourceBinding::Weak {
            entry_symbol = entry_symbol.with_weak_binding();
        }
        if artifact.public_symbol_is_exact {
            entry_symbol = entry_symbol.with_exact_object_name();
        }
        symbols.push(entry_symbol);
        symbols.push(GeneratedSymbol::internal(
            hidden_body,
            body_kind,
            GeneratedSymbolOwner::PrimaryObject,
        ));
        assemblies.push(assembly);
    }
    for (object, artifact) in &abi_plan.plan().artifacts.tls_accessors {
        let global = module
            .globals
            .get(object.0 as usize)
            .filter(|global| global.id == *object)
            .ok_or_else(|| error(format!("TLS accessor references unknown data {}", object.0)))?;
        if global.emission.symbol_name != artifact.object_symbol {
            return Err(error(
                "TLS accessor source symbol differs from the module ABI plan",
            ));
        }
        if global.emission.symbol_name_is_exact != artifact.object_symbol_is_exact {
            return Err(error(
                "TLS accessor source symbol exactness differs from the module ABI plan",
            ));
        }
        let model = match artifact.model {
            ccc_sema::generic::TlsModel::GeneralDynamic => TlsAccessModel::GeneralDynamic,
            ccc_sema::generic::TlsModel::LocalDynamic => TlsAccessModel::LocalDynamic,
            ccc_sema::generic::TlsModel::InitialExec => TlsAccessModel::InitialExec,
            ccc_sema::generic::TlsModel::LocalExec => TlsAccessModel::LocalExec,
        };
        let object_visibility = if matches!(
            artifact.source_linkage,
            ccc_abi::SourceLinkage::None | ccc_abi::SourceLinkage::Internal
        ) {
            TlsSymbolVisibility::Hidden
        } else {
            match artifact.source_visibility {
                ccc_abi::SourceVisibility::Default => TlsSymbolVisibility::Default,
                ccc_abi::SourceVisibility::Hidden => TlsSymbolVisibility::Hidden,
                ccc_abi::SourceVisibility::Protected => TlsSymbolVisibility::Protected,
                ccc_abi::SourceVisibility::Internal => TlsSymbolVisibility::Internal,
            }
        };
        let assembly = render_target_tls_accessor(&TlsAccessorPlan {
            abi: config.target.abi,
            helper_symbol: artifact.helper_symbol.clone(),
            object_symbol: artifact.object_symbol.clone(),
            object_symbol_is_exact: artifact.object_symbol_is_exact,
            model,
            object_visibility,
            logical_line: 1,
        })
        .map_err(module_error)?;
        symbols.push(GeneratedSymbol::internal(
            &artifact.helper_symbol,
            GeneratedSymbolKind::TlsAccessor,
            GeneratedSymbolOwner::AssemblyUnit(assembly.stem().to_owned()),
        ));
        if artifact.source_defined
            && matches!(
                artifact.source_linkage,
                ccc_abi::SourceLinkage::None | ccc_abi::SourceLinkage::Internal
            )
        {
            let mut symbol = GeneratedSymbol::source_internal(
                &artifact.object_symbol,
                GeneratedSymbolKind::TlsObject,
                GeneratedSymbolOwner::PrimaryObject,
            );
            if artifact.object_symbol_is_exact {
                symbol = symbol.with_exact_object_name();
            }
            symbols.push(symbol);
        }
        assemblies.push(assembly);
    }
    let manifest = BridgeManifestV2::new(abi_plan.plan().translation_unit_digest.0, symbols);
    let actual_localization = manifest.localization_symbols();
    let expected_localization = &abi_plan
        .plan()
        .artifacts
        .packaging
        .exact_localization_symbols;
    if actual_localization
        != expected_localization
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
    {
        return Err(error(
            "generated bridge manifest differs from the ABI plan localization allowlist",
        ));
    }
    if assemblies.len() != abi_plan.plan().artifacts.packaging.generated_assembly_units as usize {
        return Err(error(
            "generated assembly count differs from the module ABI packaging plan",
        ));
    }
    Ok((assemblies, manifest))
}

struct Declarations {
    functions: HashMap<u32, FuncId>,
    definition_functions: HashMap<u32, FuncId>,
    definition_signatures: HashMap<u32, ir::Signature>,
    hidden_body_symbols: HashMap<u32, String>,
    call_helper: Option<FuncId>,
    call_helper_symbol: Option<String>,
    runtime_realloc: Option<FuncId>,
    runtime_free: Option<FuncId>,
    runtime_helpers: HashMap<&'static str, FuncId>,
    f80_support: Option<FuncId>,
    inline_cpuid_support: Option<FuncId>,
    inline_rdtsc_support: Option<FuncId>,
    globals: HashMap<u32, DataDeclaration>,
    strings: HashMap<u32, ClifDataId>,
    commons: Vec<CommonDefinition>,
}

#[derive(Clone, Copy)]
struct DataDeclaration {
    id: ClifDataId,
    tls: bool,
    tls_accessor: Option<FuncId>,
}

struct CommonDefinition {
    id: ClifDataId,
    size: u64,
    align: u64,
}

fn declare_module(
    module: &gir::FullModule,
    config: &EffectiveCompilationConfig,
    abi_plan: ccc_abi::VerifiedModuleAbiPlan<'_>,
    object_module: &mut ObjectModule,
) -> Result<Declarations, CodegenError> {
    let direct_calls = direct_call_signatures(module)?;
    let mut functions = HashMap::with_capacity(module.functions.len());
    let mut definition_functions = HashMap::new();
    let mut definition_signatures = HashMap::new();
    let mut hidden_body_symbols = HashMap::new();
    for function in &module.functions {
        let definition_plan = abi_plan.plan().definitions.get(&function.id);
        let definition_boundary = definition_plan.map(|plan| &plan.boundary);
        let call_use = direct_calls.get(&function.id.0);
        let public_signature = if let Some(boundary) = definition_boundary {
            match boundary {
                ccc_abi::BoundaryPlan::Native(plan) => super::signature(plan)
                    .map_err(error)
                    .map_err(|error| error.with_span_if_none(function.span))?,
                ccc_abi::BoundaryPlan::Bridge(_) => opaque_signature(config)?,
            }
        } else if let Some(call) = call_use {
            match abi_plan.plan().calls.get(&(call.caller, call.instruction)) {
                Some(call) if matches!(call.boundary, ccc_abi::BoundaryPlan::Native(_)) => {
                    let ccc_abi::BoundaryPlan::Native(plan) = &call.boundary else {
                        unreachable!()
                    };
                    super::signature(plan)
                        .map_err(error)
                        .map_err(|error| error.with_span_if_none(call.source_location))?
                }
                Some(call) if matches!(call.boundary, ccc_abi::BoundaryPlan::Bridge(_)) => {
                    opaque_signature(config)?
                }
                Some(_) => unreachable!(),
                None => return Err(error("direct call has no module ABI plan")),
            }
        } else {
            ccc_abi::plan_function_type(&module.types, function.signature, config)
                .ok()
                .and_then(|plan| super::signature(&plan).ok())
                .unwrap_or(opaque_signature(config)?)
        };
        if let (Some(ccc_abi::BoundaryPlan::Native(definition)), Some(call)) =
            (definition_boundary, call_use)
            && let Some(call_plan) = abi_plan.plan().calls.get(&(call.caller, call.instruction))
            && let ccc_abi::BoundaryPlan::Native(call_plan) = &call_plan.boundary
            && super::signature(definition).map_err(error)?
                != super::signature(call_plan).map_err(error)?
        {
            return Err(error(format!(
                "direct calls to `{}` use an ABI signature that differs from its definition",
                function.symbol_name
            )));
        }
        let public_linkage =
            if matches!(definition_boundary, Some(ccc_abi::BoundaryPlan::Bridge(_))) {
                Linkage::Import
            } else {
                function_linkage(function)
            };
        let public_id = object_module
            .declare_function(&function.symbol_name, public_linkage, &public_signature)
            .map_err(module_error)
            .map_err(|error| error.with_span_if_none(function.span))?;
        if functions.insert(function.id.0, public_id).is_some() {
            return Err(error(format!(
                "duplicate function id {} in typed IR",
                function.id.0
            ))
            .with_span_if_none(function.span));
        }

        if let Some(boundary) = definition_boundary {
            match boundary {
                ccc_abi::BoundaryPlan::Native(_) => {
                    definition_functions.insert(function.id.0, public_id);
                    definition_signatures.insert(function.id.0, public_signature);
                }
                ccc_abi::BoundaryPlan::Bridge(_) => {
                    let hidden_symbol = abi_plan
                        .plan()
                        .artifacts
                        .bridge_entries
                        .get(&function.id)
                        .ok_or_else(|| error("bridged definition has no artifact plan"))?
                        .body_symbol
                        .clone();
                    let hidden_signature = variadic_body_signature(config)?;
                    let hidden_id = object_module
                        .declare_function(&hidden_symbol, Linkage::Hidden, &hidden_signature)
                        .map_err(module_error)
                        .map_err(|error| error.with_span_if_none(function.span))?;
                    definition_functions.insert(function.id.0, hidden_id);
                    definition_signatures.insert(function.id.0, hidden_signature);
                    hidden_body_symbols.insert(function.id.0, hidden_symbol);
                }
            }
        }
    }

    let (call_helper, call_helper_symbol) =
        if let Some(call_bridge) = &abi_plan.plan().artifacts.call_bridge {
            let symbol = call_bridge.helper_symbol.clone();
            let signature = variadic_body_signature(config)?;
            let id = object_module
                .declare_function(&symbol, Linkage::Import, &signature)
                .map_err(module_error)?;
            (Some(id), Some(symbol))
        } else {
            (None, None)
        };

    let (runtime_realloc, runtime_free) = if module_uses_runtime_sized_storage(module) {
        let mut realloc_signature = object_module.make_signature();
        realloc_signature
            .params
            .push(ir::AbiParam::new(ir::types::I64));
        realloc_signature
            .params
            .push(ir::AbiParam::new(ir::types::I64));
        realloc_signature
            .returns
            .push(ir::AbiParam::new(ir::types::I64));
        let realloc = object_module
            .declare_function("realloc", Linkage::Import, &realloc_signature)
            .map_err(module_error)?;

        let mut free_signature = object_module.make_signature();
        free_signature
            .params
            .push(ir::AbiParam::new(ir::types::I64));
        let free = object_module
            .declare_function("free", Linkage::Import, &free_signature)
            .map_err(module_error)?;
        (Some(realloc), Some(free))
    } else {
        (None, None)
    };

    let required_runtime_helpers = required_runtime_helper_symbols(module, config);
    let mut runtime_helpers = HashMap::new();
    for contract in config
        .target
        .abi
        .runtime_helper_manifest()
        .iter()
        .filter(|contract| {
            required_runtime_helpers.contains(contract.symbol) && !runtime_helper_uses_f80(contract)
        })
    {
        let signature = runtime_helper_signature(object_module, contract)?;
        let id = object_module
            .declare_function(contract.symbol, Linkage::Import, &signature)
            .map_err(module_error)?;
        if runtime_helpers.insert(contract.symbol, id).is_some() {
            return Err(error(format!(
                "duplicate runtime-helper manifest symbol `{}`",
                contract.symbol
            )));
        }
    }

    let f80_support = if let Some(support) = &abi_plan.plan().artifacts.f80_support {
        let signature = variadic_body_signature(config)?;
        let id = object_module
            .declare_function(&support.helper_symbol, Linkage::Import, &signature)
            .map_err(module_error)?;
        Some(id)
    } else {
        None
    };

    let (inline_cpuid_support, inline_rdtsc_support) =
        if let Some(support) = &abi_plan.plan().artifacts.inline_asm_support {
            let cpuid = if let Some(symbol) = &support.cpuid_symbol {
                let mut signature = object_module.make_signature();
                signature.params.extend([
                    ir::AbiParam::new(ir::types::I32),
                    ir::AbiParam::new(ir::types::I32),
                    ir::AbiParam::new(ir::types::I64),
                    ir::AbiParam::new(ir::types::I64),
                    ir::AbiParam::new(ir::types::I64),
                    ir::AbiParam::new(ir::types::I64),
                ]);
                Some(
                    object_module
                        .declare_function(symbol, Linkage::Import, &signature)
                        .map_err(module_error)?,
                )
            } else {
                None
            };
            let rdtsc = if let Some(symbol) = &support.rdtsc_symbol {
                let mut signature = object_module.make_signature();
                signature.params.extend([
                    ir::AbiParam::new(ir::types::I64),
                    ir::AbiParam::new(ir::types::I64),
                ]);
                Some(
                    object_module
                        .declare_function(symbol, Linkage::Import, &signature)
                        .map_err(module_error)?,
                )
            } else {
                None
            };
            (cpuid, rdtsc)
        } else {
            (None, None)
        };

    let tls_accessor_signature = tls_accessor_signature(config)?;
    let mut tls_accessors = HashMap::with_capacity(abi_plan.plan().artifacts.tls_accessors.len());
    for (object, accessor) in &abi_plan.plan().artifacts.tls_accessors {
        let id = object_module
            .declare_function(
                &accessor.helper_symbol,
                Linkage::Import,
                &tls_accessor_signature,
            )
            .map_err(module_error)?;
        if tls_accessors.insert(object.0, id).is_some() {
            return Err(error(format!(
                "duplicate TLS accessor for data {} in module ABI plan",
                object.0
            )));
        }
    }

    let mut globals = HashMap::with_capacity(module.globals.len());
    let mut commons = Vec::new();
    for global in &module.globals {
        let tls = global.duration == StorageDuration::Thread || global.emission.tls.is_some();
        let is_external_common = !tls
            && global.emission.definition == ObjectDefinitionPolicy::TentativeCommon
            && global.linkage == CLinkage::External;
        let is_external_common =
            is_external_common && config.target.triple.binary_format == BinaryFormat::Elf;
        let linkage = if is_external_common {
            // Cranelift has no common-symbol linkage. Keep the symbol
            // undefined through module finalization, then rewrite its ELF
            // symbol entry to SHN_COMMON with the requested size/alignment.
            Linkage::Import
        } else if tls
            && global.linkage != CLinkage::External
            && global.emission.definition != ObjectDefinitionPolicy::Declaration
        {
            // The generated accessor lives in a separate temporary object.
            // Give an internal TLS definition hidden link visibility until
            // the verified partial link resolves it, then the manifest
            // restores local binding before publication.
            Linkage::Hidden
        } else {
            global_linkage(global)
        };
        let writable = !global.ty.qualifiers.contains(TypeQualifiers::CONST);
        let id = object_module
            .declare_data(&global.emission.symbol_name, linkage, writable, tls)
            .map_err(module_error)
            .map_err(|error| error.with_span_if_none(global.span))?;
        let tls_accessor = tls_accessors.get(&global.id.0).copied();
        if tls != tls_accessor.is_some() {
            return Err(error(format!(
                "data {} and its TLS accessor plan disagree",
                global.id.0
            ))
            .with_span_if_none(global.span));
        }
        if globals
            .insert(
                global.id.0,
                DataDeclaration {
                    id,
                    tls,
                    tls_accessor,
                },
            )
            .is_some()
        {
            return Err(
                error(format!("duplicate data id {} in typed IR", global.id.0))
                    .with_span_if_none(global.span),
            );
        }
        if is_external_common {
            let layout = object_layout(&module.types, global.ty, config)
                .map_err(|error| error.with_span_if_none(global.span))?;
            commons.push(CommonDefinition {
                id,
                size: layout.size,
                align: data::requested_alignment(global.emission.requested_alignment, layout.align)
                    .map_err(|error| error.with_span_if_none(global.span))?,
            });
        }
    }

    let mut strings = HashMap::with_capacity(module.strings.len());
    for string in &module.strings {
        let name = format!("__ccc_string_{}", string.id.0);
        let id = object_module
            .declare_data(&name, Linkage::Local, false, false)
            .map_err(module_error)?;
        if strings.insert(string.id.0, id).is_some() {
            return Err(error(format!(
                "duplicate string id {} in typed IR",
                string.id.0
            )));
        }
    }
    Ok(Declarations {
        functions,
        definition_functions,
        definition_signatures,
        hidden_body_symbols,
        call_helper,
        call_helper_symbol,
        runtime_realloc,
        runtime_free,
        runtime_helpers,
        f80_support,
        inline_cpuid_support,
        inline_rdtsc_support,
        globals,
        strings,
        commons,
    })
}

fn runtime_helper_signature(
    object_module: &ObjectModule,
    contract: &RuntimeHelperContract,
) -> Result<ir::Signature, CodegenError> {
    let mut signature = object_module.make_signature();
    for parameter in contract.parameters {
        signature.params.push(runtime_helper_abi_param(*parameter)?);
    }
    signature
        .returns
        .push(runtime_helper_abi_param(contract.result)?);
    Ok(signature)
}

fn runtime_helper_abi_param(value: RuntimeHelperValue) -> Result<ir::AbiParam, CodegenError> {
    let ty = match value {
        RuntimeHelperValue::SignedInt128 | RuntimeHelperValue::UnsignedInt128 => ir::types::I128,
        RuntimeHelperValue::Float32 => ir::types::F32,
        RuntimeHelperValue::Float64 => ir::types::F64,
        RuntimeHelperValue::Float80 => {
            return Err(error(
                "an x87 runtime helper cannot be declared through Cranelift",
            ));
        }
    };
    Ok(ir::AbiParam::new(ty))
}

fn runtime_helper_uses_f80(contract: &RuntimeHelperContract) -> bool {
    contract.result == RuntimeHelperValue::Float80
        || contract.parameters.contains(&RuntimeHelperValue::Float80)
}

fn required_runtime_helper_symbols(
    module: &gir::FullModule,
    config: &EffectiveCompilationConfig,
) -> HashSet<&'static str> {
    let mut required = HashSet::new();
    for function in &module.functions {
        for instruction in function.blocks.iter().flat_map(|block| &block.instructions) {
            match instruction.kind {
                gir::FullInstructionKind::Binary { operator, left, .. }
                    if matches!(
                        operator,
                        gir::BinaryOperation::Divide | gir::BinaryOperation::Remainder
                    ) =>
                {
                    let Some(ty) = function.value_types.get(left.0 as usize).copied() else {
                        continue;
                    };
                    let symbol = match (module.types.builtin_type(ty), operator) {
                        (Some(BuiltinType::Int128), gir::BinaryOperation::Divide) => "__divti3",
                        (Some(BuiltinType::UnsignedInt128), gir::BinaryOperation::Divide) => {
                            "__udivti3"
                        }
                        (Some(BuiltinType::Int128), gir::BinaryOperation::Remainder) => "__modti3",
                        (Some(BuiltinType::UnsignedInt128), gir::BinaryOperation::Remainder) => {
                            "__umodti3"
                        }
                        _ => continue,
                    };
                    required.insert(symbol);
                }
                gir::FullInstructionKind::Convert { kind, from, to, .. }
                    if matches!(
                        kind,
                        gir::ScalarConversion::IntegerToFloating
                            | gir::ScalarConversion::FloatingToInteger
                    ) =>
                {
                    let symbol = match (
                        kind,
                        module.types.builtin_type(from.ty),
                        module.types.builtin_type(to.ty),
                    ) {
                        (
                            gir::ScalarConversion::IntegerToFloating,
                            Some(BuiltinType::Int128),
                            Some(BuiltinType::Float),
                        ) => "__floattisf",
                        (
                            gir::ScalarConversion::IntegerToFloating,
                            Some(BuiltinType::Int128),
                            Some(BuiltinType::Double),
                        ) => "__floattidf",
                        (
                            gir::ScalarConversion::IntegerToFloating,
                            Some(BuiltinType::Int128),
                            Some(BuiltinType::LongDouble),
                        ) => match config.target.data_layout.long_double_format {
                            LongDoubleFormat::Binary64 => "__floattidf",
                            LongDoubleFormat::X87Extended => "__floattixf",
                            LongDoubleFormat::IeeeBinary128 => continue,
                        },
                        (
                            gir::ScalarConversion::IntegerToFloating,
                            Some(BuiltinType::UnsignedInt128),
                            Some(BuiltinType::Float),
                        ) => "__floatuntisf",
                        (
                            gir::ScalarConversion::IntegerToFloating,
                            Some(BuiltinType::UnsignedInt128),
                            Some(BuiltinType::Double),
                        ) => "__floatuntidf",
                        (
                            gir::ScalarConversion::IntegerToFloating,
                            Some(BuiltinType::UnsignedInt128),
                            Some(BuiltinType::LongDouble),
                        ) => match config.target.data_layout.long_double_format {
                            LongDoubleFormat::Binary64 => "__floatuntidf",
                            LongDoubleFormat::X87Extended => "__floatuntixf",
                            LongDoubleFormat::IeeeBinary128 => continue,
                        },
                        (
                            gir::ScalarConversion::FloatingToInteger,
                            Some(BuiltinType::Float),
                            Some(BuiltinType::Int128),
                        ) => "__fixsfti",
                        (
                            gir::ScalarConversion::FloatingToInteger,
                            Some(BuiltinType::Double),
                            Some(BuiltinType::Int128),
                        ) => "__fixdfti",
                        (
                            gir::ScalarConversion::FloatingToInteger,
                            Some(BuiltinType::LongDouble),
                            Some(BuiltinType::Int128),
                        ) => match config.target.data_layout.long_double_format {
                            LongDoubleFormat::Binary64 => "__fixdfti",
                            LongDoubleFormat::X87Extended => "__fixxfti",
                            LongDoubleFormat::IeeeBinary128 => continue,
                        },
                        (
                            gir::ScalarConversion::FloatingToInteger,
                            Some(BuiltinType::Float),
                            Some(BuiltinType::UnsignedInt128),
                        ) => "__fixunssfti",
                        (
                            gir::ScalarConversion::FloatingToInteger,
                            Some(BuiltinType::Double),
                            Some(BuiltinType::UnsignedInt128),
                        ) => "__fixunsdfti",
                        (
                            gir::ScalarConversion::FloatingToInteger,
                            Some(BuiltinType::LongDouble),
                            Some(BuiltinType::UnsignedInt128),
                        ) => match config.target.data_layout.long_double_format {
                            LongDoubleFormat::Binary64 => "__fixunsdfti",
                            LongDoubleFormat::X87Extended => "__fixunsxfti",
                            LongDoubleFormat::IeeeBinary128 => continue,
                        },
                        _ => continue,
                    };
                    required.insert(symbol);
                }
                _ => {}
            }
        }
    }
    required
}

fn module_uses_runtime_sized_storage(module: &gir::FullModule) -> bool {
    module.functions.iter().any(|function| {
        function.blocks.iter().any(|block| {
            block.instructions.iter().any(|instruction| {
                matches!(
                    instruction.kind,
                    gir::FullInstructionKind::RuntimeSizedAllocate { .. }
                )
            })
        })
    })
}

#[derive(Clone, Copy)]
struct DirectCallUse {
    signature: TypeId,
    caller: ccc_sema::generic::FullFunctionId,
    instruction: gir::InstructionId,
}

fn direct_call_signatures(
    module: &gir::FullModule,
) -> Result<HashMap<u32, DirectCallUse>, CodegenError> {
    let mut signatures = HashMap::new();
    for caller in &module.functions {
        for instruction in caller.blocks.iter().flat_map(|block| &block.instructions) {
            let gir::FullInstructionKind::DirectCall {
                function: callee,
                signature,
                ..
            } = instruction.kind
            else {
                continue;
            };
            let call = DirectCallUse {
                signature,
                caller: caller.id,
                instruction: instruction.id,
            };
            if let Some(previous) = signatures.insert(callee.0, call)
                && previous.signature != signature
            {
                return Err(error(format!(
                    "direct calls to function {} carry inconsistent canonical signatures",
                    callee.0
                ))
                .with_span_if_none(instruction.span));
            }
        }
    }
    for function in signatures.keys() {
        if !module
            .functions
            .iter()
            .any(|candidate| candidate.id.0 == *function)
        {
            return Err(error(format!(
                "direct call references undeclared function {function}"
            )));
        }
    }
    Ok(signatures)
}

fn opaque_signature(config: &EffectiveCompilationConfig) -> Result<ir::Signature, CodegenError> {
    let calling_convention = config
        .target
        .calling_convention()
        .ok_or_else(|| error("target has no C calling convention for function declarations"))?;
    super::signature(&crate::opaque_native_plan(calling_convention)).map_err(error)
}

fn variadic_body_signature(
    config: &EffectiveCompilationConfig,
) -> Result<ir::Signature, CodegenError> {
    let calling_convention = config
        .target
        .calling_convention()
        .ok_or_else(|| error("target has no C calling convention for variadic bodies"))?;
    let mut plan = crate::opaque_native_plan(calling_convention);
    plan.clif_parameters.push(ccc_abi::NativeCarrierPlan {
        abi_param_index: 0,
        source_index: None,
        piece_index: None,
        source_offset: 0,
        valid_bytes: 8,
        class: ccc_abi::AbiClass::Integer,
        carrier: ccc_abi::AbiCarrier::I64,
        extension: ccc_abi::IntegerExtension::None,
        purpose: ccc_abi::NativePurpose::Normal,
    });
    super::signature(&plan).map_err(error)
}

fn tls_accessor_signature(
    config: &EffectiveCompilationConfig,
) -> Result<ir::Signature, CodegenError> {
    let mut signature = opaque_signature(config)?;
    signature.returns.push(ir::AbiParam::new(ir::types::I64));
    Ok(signature)
}

fn function_linkage(function: &gir::FullFunction) -> Linkage {
    if function.entry.is_none() {
        return Linkage::Import;
    }
    if function.linkage != CLinkage::External {
        return Linkage::Local;
    }
    match function.visibility {
        SymbolVisibility::Hidden => Linkage::Hidden,
        SymbolVisibility::Default | SymbolVisibility::Protected | SymbolVisibility::Internal => {
            Linkage::Export
        }
    }
}

fn global_linkage(global: &gir::FullGlobal) -> Linkage {
    if global.emission.definition == ObjectDefinitionPolicy::Declaration {
        return Linkage::Import;
    }
    if global.linkage != CLinkage::External {
        return Linkage::Local;
    }
    match global.emission.visibility {
        SymbolVisibility::Hidden => Linkage::Hidden,
        SymbolVisibility::Default | SymbolVisibility::Protected | SymbolVisibility::Internal => {
            Linkage::Export
        }
    }
}

fn set_elf_symbol_visibility(symbol: &mut object::write::Symbol, visibility: SymbolVisibility) {
    let binding = if symbol.weak {
        object::elf::STB_WEAK
    } else {
        object::elf::STB_GLOBAL
    };
    let symbol_type = match symbol.kind {
        SymbolKind::Text => object::elf::STT_FUNC,
        SymbolKind::Data => object::elf::STT_OBJECT,
        SymbolKind::Tls => object::elf::STT_TLS,
        _ => object::elf::STT_NOTYPE,
    };
    let st_other = match visibility {
        SymbolVisibility::Default => object::elf::STV_DEFAULT,
        SymbolVisibility::Hidden => object::elf::STV_HIDDEN,
        SymbolVisibility::Protected => object::elf::STV_PROTECTED,
        SymbolVisibility::Internal => object::elf::STV_INTERNAL,
    };
    symbol.flags = SymbolFlags::Elf {
        st_info: (binding << 4) | symbol_type,
        st_other,
    };
}

fn set_macho_symbol_visibility(symbol: &mut object::write::Symbol, visibility: SymbolVisibility) {
    symbol.scope = match visibility {
        SymbolVisibility::Default | SymbolVisibility::Protected => SymbolScope::Dynamic,
        SymbolVisibility::Hidden | SymbolVisibility::Internal => SymbolScope::Linkage,
    };
}

fn object_layout(
    types: &TypeStore,
    ty: QualifiedType,
    config: &EffectiveCompilationConfig,
) -> Result<ccc_types::TypeLayout, CodegenError> {
    types.layout_of(ty.ty, config).map_err(|layout| {
        error(format!(
            "type `{}` has no object layout: {layout}",
            types.display_qualified(ty)
        ))
    })
}

fn abi_error(error: ccc_abi::AbiError) -> CodegenError {
    CodegenError {
        code: error.code,
        message: error.message,
        span: error.span,
    }
}

fn module_error(error: impl std::fmt::Display) -> CodegenError {
    self::error(error.to_string())
}

fn error(message: impl Into<String>) -> CodegenError {
    CodegenError {
        code: BACKEND_ERROR,
        message: message.into(),
        span: None,
    }
}
