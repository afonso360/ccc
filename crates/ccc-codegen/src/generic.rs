//! Cranelift lowering for the typed control-flow IR.

mod data;
mod function;
#[cfg(test)]
mod tests;
mod unwind;

use std::collections::{HashMap, HashSet};

use ccc_ir::generic as gir;
use ccc_sema::generic::{
    Linkage as CLinkage, ObjectDefinitionPolicy, StorageDuration, SymbolBinding, SymbolVisibility,
};
use ccc_target::{EffectiveCompilationConfig, RelocationModel};
use ccc_types::{
    BuiltinType, LayoutShape, QualifiedType, TypeId, TypeKind, TypeQualifiers, TypeStore,
};
use cranelift_codegen::Context;
use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_codegen::ir::immediates::{Ieee32, Ieee64};
use cranelift_codegen::ir::{
    self, BlockArg, InstBuilder, MemFlags, StackSlot, StackSlotData, StackSlotKind, TrapCode,
    UserFuncName,
};
use cranelift_codegen::isa;
use cranelift_codegen::settings::{self, Configurable as _};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_module::{
    DataDescription, DataId as ClifDataId, FuncId, Linkage, Module as _, default_libcall_names,
};
use cranelift_object::{ObjectBuilder, ObjectModule};
use object::write::SymbolSection;
use object::{SymbolFlags, SymbolKind, SymbolScope};

use crate::{CodegenError, Options, Output};

const BACKEND_ERROR: &str = "CCC4002";
const ATOMIC_ERROR: &str = "CCC4011";

/// Emits an ELF object from the typed control-flow IR.
///
/// Function ABI plans are derived from each canonical function type, so the
/// same stable diagnostics are used for declarations, definitions, direct
/// calls, and indirect calls.
pub fn emit(
    module: &gir::FullModule,
    config: &EffectiveCompilationConfig,
    options: Options,
) -> Result<Output, CodegenError> {
    let plan = ccc_abi::plan_module(module, config).map_err(abi_error)?;
    emit_with_plan(module, config, &plan, options)
}

pub fn emit_with_plan(
    module: &gir::FullModule,
    config: &EffectiveCompilationConfig,
    plan: &ccc_abi::ModuleAbiPlan,
    options: Options,
) -> Result<Output, CodegenError> {
    let verified = plan.verify_against(module, config).map_err(abi_error)?;
    emit_inner(module, config, verified, options)
}

fn emit_inner(
    module: &gir::FullModule,
    config: &EffectiveCompilationConfig,
    abi_plan: ccc_abi::VerifiedModuleAbiPlan<'_>,
    options: Options,
) -> Result<Output, CodegenError> {
    super::validate_target(config).map_err(error)?;
    let isa_builder = isa::lookup(config.target.triple.clone()).map_err(module_error)?;
    let mut flag_builder = settings::builder();
    match config.relocation_model {
        RelocationModel::Static => flag_builder.set("is_pic", "false").map_err(module_error)?,
    }
    flag_builder
        .set("enable_llvm_abi_extensions", "false")
        .map_err(module_error)?;
    flag_builder
        .set("enable_multi_ret_implicit_sret", "false")
        .map_err(module_error)?;
    flag_builder
        .set("unwind_info", "true")
        .map_err(module_error)?;
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
    data::define_strings(module, &declarations, &mut object_module)?;
    data::define_globals(module, config, &declarations, &mut object_module)?;

    let mut clif = String::new();
    for function in &module.functions {
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
        let references = function::declare_function_references(
            &declarations,
            &mut object_module,
            &mut context.func,
        );
        let frontend_config = object_module.isa().frontend_config();
        let definition_plan = abi_plan
            .plan()
            .definitions
            .get(&function.id)
            .ok_or_else(|| error(format!("function {} has no module ABI plan", function.id.0)))?;
        let definition_plan = match &definition_plan.boundary {
            ccc_abi::BoundaryPlan::Native(plan) => function::DefinitionAbi::Native(plan),
            ccc_abi::BoundaryPlan::Bridge(plan) => function::DefinitionAbi::Variadic(plan),
        };
        function::lower_function(
            module,
            function,
            config,
            abi_plan,
            definition_plan,
            &references,
            frontend_config,
            &mut context.func,
        )
        .map_err(|error| error.with_span_if_none(function.span))?;
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
        unwind
            .record_function(id, &context, object_module.isa())
            .map_err(error)?;
    }

    let mut product = object_module.finish();
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
        set_elf_symbol_visibility(symbol, function.visibility);
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
        set_elf_symbol_visibility(symbol, global.emission.visibility);
    }
    unwind.emit(&mut product).map_err(error)?;
    let object = product.emit().map_err(module_error)?;
    let (assemblies, manifest) = generated_bridge_artifacts(
        module,
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

fn generated_bridge_artifacts(
    _module: &gir::FullModule,
    abi_plan: ccc_abi::VerifiedModuleAbiPlan<'_>,
    hidden_body_symbols: &HashMap<u32, String>,
    call_helper_symbol: Option<&str>,
) -> Result<
    (
        Vec<ccc_link::bridge::GeneratedAssembly>,
        ccc_link::artifact::BridgeManifestV1,
    ),
    CodegenError,
> {
    use ccc_link::artifact::{BridgeManifestV1, GeneratedSymbol, GeneratedSymbolOwner};
    use ccc_link::bridge::{
        AssemblyFunctionLinkage, GeneratedSymbolKind, VariadicEntryPlan,
        render_generic_call_helper, render_variadic_entry,
    };

    let mut assemblies = Vec::new();
    let mut symbols = Vec::new();
    if let Some(helper) = call_helper_symbol {
        let assembly = render_generic_call_helper(helper).map_err(module_error)?;
        symbols.push(GeneratedSymbol::internal(
            helper,
            GeneratedSymbolKind::CallHelper,
            GeneratedSymbolOwner::AssemblyUnit(assembly.stem().to_owned()),
        ));
        assemblies.push(assembly);
    }
    for (function, artifact) in &abi_plan.plan().artifacts.variadic_entries {
        let definition = abi_plan
            .plan()
            .definitions
            .get(function)
            .ok_or_else(|| error("variadic entry artifact has no definition plan"))?;
        let ccc_abi::BoundaryPlan::Bridge(plan) = &definition.boundary else {
            return Err(error(
                "variadic entry artifact references a native definition plan",
            ));
        };
        let hidden_body = hidden_body_symbols.get(&function.0).ok_or_else(|| {
            error(format!(
                "variadic function {} has no hidden body symbol",
                function.0
            ))
        })?;
        if hidden_body != &artifact.body_symbol {
            return Err(error(
                "declared variadic body symbol differs from the module ABI plan",
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
        let xmm_results = plan.result_pieces.len() as u8 - gp_results;
        let assembly = render_variadic_entry(&VariadicEntryPlan {
            public_symbol: public_symbol.clone(),
            hidden_body_symbol: hidden_body.clone(),
            linkage,
            fixed_gp_used: plan.gp_used,
            fixed_sse_used: plan.xmm_used,
            overflow_arg_offset: plan.overflow_arg_offset,
            gp_results,
            xmm_results,
            hidden_return: plan.hidden_return,
            logical_line: 1,
        })
        .map_err(module_error)?;
        let entry_symbol = match linkage {
            AssemblyFunctionLinkage::ExternalDefault => GeneratedSymbol::public(
                public_symbol,
                GeneratedSymbolKind::VariadicEntry,
                GeneratedSymbolOwner::AssemblyUnit(assembly.stem().to_owned()),
            ),
            AssemblyFunctionLinkage::Internal => GeneratedSymbol::source_internal(
                public_symbol,
                GeneratedSymbolKind::VariadicEntry,
                GeneratedSymbolOwner::AssemblyUnit(assembly.stem().to_owned()),
            ),
            AssemblyFunctionLinkage::ExternalHidden => GeneratedSymbol::source_hidden(
                public_symbol,
                GeneratedSymbolKind::VariadicEntry,
                GeneratedSymbolOwner::AssemblyUnit(assembly.stem().to_owned()),
            ),
            AssemblyFunctionLinkage::ExternalProtected => GeneratedSymbol::source_protected(
                public_symbol,
                GeneratedSymbolKind::VariadicEntry,
                GeneratedSymbolOwner::AssemblyUnit(assembly.stem().to_owned()),
            ),
            AssemblyFunctionLinkage::ExternalInternal => GeneratedSymbol::source_elf_internal(
                public_symbol,
                GeneratedSymbolKind::VariadicEntry,
                GeneratedSymbolOwner::AssemblyUnit(assembly.stem().to_owned()),
            ),
        };
        symbols.push(entry_symbol);
        symbols.push(GeneratedSymbol::internal(
            hidden_body,
            GeneratedSymbolKind::VariadicBody,
            GeneratedSymbolOwner::PrimaryObject,
        ));
        assemblies.push(assembly);
    }
    let manifest = BridgeManifestV1::new(abi_plan.plan().translation_unit_digest.0, symbols);
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
    globals: HashMap<u32, DataDeclaration>,
    strings: HashMap<u32, ClifDataId>,
    commons: Vec<CommonDefinition>,
}

#[derive(Clone, Copy)]
struct DataDeclaration {
    id: ClifDataId,
    tls: bool,
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
                        .variadic_entries
                        .get(&function.id)
                        .ok_or_else(|| error("variadic definition has no artifact plan"))?
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

    let mut globals = HashMap::with_capacity(module.globals.len());
    let mut commons = Vec::new();
    for global in &module.globals {
        let tls = global.duration == StorageDuration::Thread || global.emission.tls.is_some();
        let is_external_common = global.emission.definition
            == ObjectDefinitionPolicy::TentativeCommon
            && global.linkage == CLinkage::External;
        let linkage = if is_external_common {
            // Cranelift has no common-symbol linkage. Keep the symbol
            // undefined through module finalization, then rewrite its ELF
            // symbol entry to SHN_COMMON with the requested size/alignment.
            Linkage::Import
        } else {
            global_linkage(global)
        };
        let writable = !global.ty.qualifiers.contains(TypeQualifiers::CONST);
        let id = object_module
            .declare_data(&global.emission.symbol_name, linkage, writable, tls)
            .map_err(module_error)
            .map_err(|error| error.with_span_if_none(global.span))?;
        if globals
            .insert(global.id.0, DataDeclaration { id, tls })
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
        globals,
        strings,
        commons,
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
