//! Cranelift lowering for the typed control-flow IR.

mod data;
mod function;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};

use ccc_ir::generic as gir;
use ccc_sema::generic::{
    Linkage as CLinkage, ObjectDefinitionPolicy, StorageDuration, SymbolVisibility,
};
use ccc_session::Span;
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
use object::{SymbolKind, SymbolScope};

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
    emit_inner(module, config, options)
}

fn emit_inner(
    module: &gir::FullModule,
    config: &EffectiveCompilationConfig,
    options: Options,
) -> Result<Output, CodegenError> {
    super::validate_target(config).map_err(error)?;
    let isa_builder = isa::lookup(config.target.triple.clone()).map_err(module_error)?;
    let mut flag_builder = settings::builder();
    match config.relocation_model {
        RelocationModel::Static => flag_builder.set("is_pic", "false").map_err(module_error)?,
    }
    let isa = isa_builder
        .finish(settings::Flags::new(flag_builder))
        .map_err(module_error)?;
    let object_builder =
        ObjectBuilder::new(isa, "ccc", default_libcall_names()).map_err(module_error)?;
    let mut object_module = ObjectModule::new(object_builder);

    let declarations = declare_module(module, config, &mut object_module)?;
    data::define_strings(module, &declarations, &mut object_module)?;
    data::define_globals(module, config, &declarations, &mut object_module)?;

    let mut clif = String::new();
    for function in &module.functions {
        let Some(_) = function.entry else {
            continue;
        };
        let signature = declarations
            .signatures
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
        function::lower_function(module, function, config, &references, &mut context.func)
            .map_err(|error| error.with_span_if_none(function.span))?;
        if options.emit_clif {
            clif.push_str(&format!(
                "; function {}\n{}\n",
                function.symbol_name, context.func
            ));
        }
        let id = declarations
            .functions
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
    }

    let mut product = object_module.finish();
    for common in declarations.commons {
        let symbol = product.data_symbol(common.id);
        let symbol = product.object.symbol_mut(symbol);
        symbol.section = SymbolSection::Common;
        symbol.value = common.align;
        symbol.size = common.size;
        symbol.kind = SymbolKind::Data;
        symbol.scope = SymbolScope::Dynamic;
        symbol.weak = false;
    }
    let object = product.emit().map_err(module_error)?;
    Ok(Output { object, clif })
}

struct Declarations {
    functions: HashMap<u32, FuncId>,
    signatures: HashMap<u32, ir::Signature>,
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
    object_module: &mut ObjectModule,
) -> Result<Declarations, CodegenError> {
    let direct_calls = direct_call_signatures(module)?;
    let mut functions = HashMap::with_capacity(module.functions.len());
    let mut signatures = HashMap::with_capacity(module.functions.len());
    for function in &module.functions {
        let required = if function.entry.is_some() {
            Some((function.signature, function.span))
        } else {
            direct_calls
                .get(&function.id.0)
                .map(|call| (call.signature, call.span))
        };
        let signature = if let Some((required, required_span)) = required {
            let required_plan = ccc_abi::plan_function_type(&module.types, required, config)
                .map_err(|error| abi_error(error).with_span_if_none(required_span))?;
            let required_signature = super::signature(&required_plan)
                .map_err(error)
                .map_err(|error| error.with_span_if_none(required_span))?;
            if function.entry.is_some()
                && let Some(call) = direct_calls.get(&function.id.0)
            {
                let call_plan = ccc_abi::plan_function_type(&module.types, call.signature, config)
                    .map_err(|error| abi_error(error).with_span_if_none(call.span))?;
                let call_signature = super::signature(&call_plan)
                    .map_err(error)
                    .map_err(|error| error.with_span_if_none(call.span))?;
                if call_signature != required_signature {
                    return Err(error(format!(
                        "direct calls to `{}` use an ABI signature that differs from its definition",
                        function.symbol_name
                    )));
                }
            }
            required_signature
        } else {
            ccc_abi::plan_function_type(&module.types, function.signature, config)
                .ok()
                .and_then(|plan| super::signature(&plan).ok())
                .unwrap_or(opaque_signature(config)?)
        };
        let linkage = function_linkage(function);
        let id = object_module
            .declare_function(&function.symbol_name, linkage, &signature)
            .map_err(module_error)
            .map_err(|error| error.with_span_if_none(function.span))?;
        if functions.insert(function.id.0, id).is_some() {
            return Err(error(format!(
                "duplicate function id {} in typed IR",
                function.id.0
            ))
            .with_span_if_none(function.span));
        }
        signatures.insert(function.id.0, signature);
    }

    let mut globals = HashMap::with_capacity(module.globals.len());
    let mut commons = Vec::new();
    for global in &module.globals {
        let layout = object_layout(&module.types, global.ty, config)
            .map_err(|error| error.with_span_if_none(global.span))?;
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
        signatures,
        globals,
        strings,
        commons,
    })
}

#[derive(Clone, Copy)]
struct DirectCallUse {
    signature: TypeId,
    span: Span,
}

fn direct_call_signatures(
    module: &gir::FullModule,
) -> Result<HashMap<u32, DirectCallUse>, CodegenError> {
    let mut signatures = HashMap::new();
    for function in &module.functions {
        for instruction in function.blocks.iter().flat_map(|block| &block.instructions) {
            let gir::FullInstructionKind::DirectCall {
                function,
                signature,
                ..
            } = instruction.kind
            else {
                continue;
            };
            let call = DirectCallUse {
                signature,
                span: instruction.span,
            };
            if let Some(previous) = signatures.insert(function.0, call)
                && previous.signature != signature
            {
                return Err(error(format!(
                    "direct calls to function {} carry inconsistent canonical signatures",
                    function.0
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
    super::signature(&ccc_abi::FunctionPlan {
        calling_convention,
        parameters: Vec::new(),
        result: None,
    })
    .map_err(error)
}

fn function_linkage(function: &gir::FullFunction) -> Linkage {
    if function.entry.is_none() {
        return Linkage::Import;
    }
    match function.linkage {
        CLinkage::External => Linkage::Export,
        CLinkage::Internal | CLinkage::None => Linkage::Local,
    }
}

fn global_linkage(global: &gir::FullGlobal) -> Linkage {
    if global.emission.definition == ObjectDefinitionPolicy::Declaration {
        return Linkage::Import;
    }
    if global.linkage != CLinkage::External
        || global.emission.visibility == SymbolVisibility::Internal
    {
        return Linkage::Local;
    }
    match global.emission.visibility {
        SymbolVisibility::Hidden => Linkage::Hidden,
        SymbolVisibility::Default | SymbolVisibility::Protected => Linkage::Export,
        SymbolVisibility::Internal => Linkage::Local,
    }
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
