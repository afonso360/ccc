//! CCC-IR lowering to Cranelift and ELF object emission.

use std::collections::{BTreeSet, HashMap};
use std::fmt;

use ccc_abi::{AbiScalar, ModulePlan};
use ccc_ir::{
    BinaryOperator, Function, FunctionId, InstructionKind, Module, Terminator, UnaryOperator,
    ValueId as IrValueId,
};
use ccc_target::{
    Architecture, BinaryFormat, CallingConvention, EffectiveCompilationConfig, RelocationModel,
};
use ccc_types::{TypeId, TypeKind};
use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::{
    self, AbiParam, BlockArg, InstBuilder, Signature, TrapCode, UserFuncName,
};
use cranelift_codegen::isa::{self, CallConv};
use cranelift_codegen::settings::Configurable as _;
use cranelift_codegen::{Context, settings};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_module::{FuncId, Linkage, Module as _, default_libcall_names};
use cranelift_object::{ObjectBuilder, ObjectModule};

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
}

impl fmt::Display for CodegenError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.message.fmt(formatter)
    }
}

impl std::error::Error for CodegenError {}

pub fn emit(
    ir_module: &Module,
    plans: &ModulePlan,
    config: &EffectiveCompilationConfig,
    options: Options,
) -> Result<Output, CodegenError> {
    emit_inner(ir_module, plans, config, options).map_err(|message| CodegenError {
        code: "CCC4001",
        message,
    })
}

fn emit_inner(
    ir_module: &Module,
    plans: &ModulePlan,
    config: &EffectiveCompilationConfig,
    options: Options,
) -> Result<Output, String> {
    validate_target(config)?;
    if plans.functions.len() != ir_module.functions.len() {
        return Err("ABI plan count does not match the IR module".to_owned());
    }

    let isa_builder =
        isa::lookup(config.target.triple.clone()).map_err(|error| error.to_string())?;
    let mut flag_builder = settings::builder();
    match config.relocation_model {
        RelocationModel::Static => flag_builder
            .set("is_pic", "false")
            .map_err(|error| error.to_string())?,
    }
    let isa = isa_builder
        .finish(settings::Flags::new(flag_builder))
        .map_err(|error| error.to_string())?;
    let builder = ObjectBuilder::new(isa, "ccc", default_libcall_names())
        .map_err(|error| error.to_string())?;
    let mut object_module = ObjectModule::new(builder);

    let signatures = plans
        .functions
        .iter()
        .map(signature)
        .collect::<Result<Vec<_>, _>>()?;
    let mut function_ids = HashMap::with_capacity(ir_module.functions.len());
    for (function, signature) in ir_module.functions.iter().zip(&signatures) {
        let linkage = if function.entry.is_some() {
            Linkage::Export
        } else {
            Linkage::Import
        };
        let id = object_module
            .declare_function(&function.name, linkage, signature)
            .map_err(|error| error.to_string())?;
        if function_ids.insert(function.id, id).is_some() {
            return Err(format!("duplicate IR function id {}", function.id.0));
        }
    }

    let mut clif = String::new();
    for (function, signature) in ir_module.functions.iter().zip(&signatures) {
        if function.entry.is_none() {
            continue;
        }
        let mut context = Context::new();
        context.func = ir::Function::with_name_signature(
            UserFuncName::user(0, function.id.0),
            signature.clone(),
        );
        let function_refs = declare_referenced_functions(
            function,
            &function_ids,
            &mut object_module,
            &mut context.func,
        )?;
        lower_function(
            ir_module,
            function,
            &function_refs,
            config,
            &mut context.func,
        )?;
        if options.emit_clif {
            clif.push_str(&format!("; function {}\n{}\n", function.name, context.func));
        }
        let function_id = function_ids
            .get(&function.id)
            .copied()
            .ok_or_else(|| format!("missing object id for `{}`", function.name))?;
        object_module
            .define_function(function_id, &mut context)
            .map_err(|error| error.to_string())?;
    }

    let object = object_module
        .finish()
        .emit()
        .map_err(|error| error.to_string())?;
    Ok(Output { object, clif })
}

fn validate_target(config: &EffectiveCompilationConfig) -> Result<(), String> {
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

fn signature(plan: &ccc_abi::FunctionPlan) -> Result<Signature, String> {
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
    signature.returns.push(abi_parameter(plan.result)?);
    Ok(signature)
}

fn abi_parameter(scalar: AbiScalar) -> Result<AbiParam, String> {
    let AbiScalar::SignedInteger { bits } = scalar;
    let ty = match bits {
        8 => ir::types::I8,
        16 => ir::types::I16,
        32 => ir::types::I32,
        64 => ir::types::I64,
        _ => return Err(format!("unsupported scalar ABI width {bits}")),
    };
    Ok(AbiParam::new(ty))
}

fn declare_referenced_functions(
    function: &Function,
    function_ids: &HashMap<FunctionId, FuncId>,
    object_module: &mut ObjectModule,
    clif_function: &mut ir::Function,
) -> Result<HashMap<FunctionId, ir::FuncRef>, String> {
    let referenced = function
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .filter_map(|instruction| match instruction.kind {
            InstructionKind::Call { function, .. } => Some(function.0),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    let mut refs = HashMap::with_capacity(referenced.len());
    for raw_function in referenced {
        let function = FunctionId(raw_function);
        let object_id = function_ids
            .get(&function)
            .copied()
            .ok_or_else(|| format!("invalid function id {}", function.0))?;
        refs.insert(
            function,
            object_module.declare_func_in_func(object_id, clif_function),
        );
    }
    Ok(refs)
}

fn lower_function(
    module: &Module,
    function: &Function,
    function_refs: &HashMap<FunctionId, ir::FuncRef>,
    config: &EffectiveCompilationConfig,
    clif_function: &mut ir::Function,
) -> Result<(), String> {
    let mut builder_context = FunctionBuilderContext::new();
    let mut builder = FunctionBuilder::new(clif_function, &mut builder_context);
    let blocks = function
        .blocks
        .iter()
        .map(|_| builder.create_block())
        .collect::<Vec<_>>();
    let entry = function.entry.ok_or_else(|| {
        format!(
            "definition `{}` does not have an entry block",
            function.name
        )
    })?;
    for block in &function.blocks {
        let clif_block = block_value(&blocks, block.id.0, "block")?;
        if block.id == entry {
            builder.append_block_params_for_function_params(clif_block);
            if builder.block_params(clif_block).len() != block.parameters.len() {
                return Err(format!(
                    "entry parameter mismatch while lowering `{}`",
                    function.name
                ));
            }
        } else {
            for parameter in &block.parameters {
                builder.append_block_param(
                    clif_block,
                    clif_type(module, value_type(function, *parameter)?, config)?,
                );
            }
        }
    }

    let mut values = vec![None; function.value_count as usize];
    for block in &function.blocks {
        let clif_block = block_value(&blocks, block.id.0, "block")?;
        for (ir_value, clif_value) in block
            .parameters
            .iter()
            .zip(builder.block_params(clif_block).iter().copied())
        {
            set_value(&mut values, *ir_value, clif_value)?;
        }
    }

    for block in &function.blocks {
        let clif_block = block_value(&blocks, block.id.0, "block")?;
        builder.switch_to_block(clif_block);
        for instruction in &block.instructions {
            let result_type = clif_type(module, instruction.ty, config)?;
            let result =
                match &instruction.kind {
                    InstructionKind::Integer(value) => {
                        builder.ins().iconst(result_type, i64::from(*value))
                    }
                    InstructionKind::Unary { operator, operand } => {
                        let operand = value(&values, *operand)?;
                        match operator {
                            UnaryOperator::Negate => builder.ins().ineg(operand),
                            UnaryOperator::LogicalNot => {
                                let boolean = builder.ins().icmp_imm(IntCC::Equal, operand, 0);
                                builder.ins().uextend(result_type, boolean)
                            }
                        }
                    }
                    InstructionKind::Binary {
                        operator,
                        left,
                        right,
                    } => {
                        let left = value(&values, *left)?;
                        let right = value(&values, *right)?;
                        lower_binary(&mut builder, *operator, left, right, result_type)
                    }
                    InstructionKind::Call {
                        function,
                        arguments,
                    } => {
                        let arguments = arguments
                            .iter()
                            .map(|argument| value(&values, *argument))
                            .collect::<Result<Vec<_>, _>>()?;
                        let function_ref = function_refs
                            .get(function)
                            .copied()
                            .ok_or_else(|| format!("function {} was not declared", function.0))?;
                        let call = builder.ins().call(function_ref, &arguments);
                        builder.inst_results(call).first().copied().ok_or_else(|| {
                            format!("call to function {} has no result", function.0)
                        })?
                    }
                };
            set_value(&mut values, instruction.result, result)?;
        }

        match block
            .terminator
            .as_ref()
            .ok_or_else(|| format!("block{} has no terminator", block.id.0))?
        {
            Terminator::Branch { target, arguments } => {
                let arguments = arguments
                    .iter()
                    .map(|argument| value(&values, *argument))
                    .collect::<Result<Vec<_>, _>>()?;
                let arguments = arguments
                    .into_iter()
                    .map(BlockArg::from)
                    .collect::<Vec<_>>();
                builder
                    .ins()
                    .jump(block_value(&blocks, target.0, "branch target")?, &arguments);
            }
            Terminator::ConditionalBranch {
                condition,
                then_block,
                else_block,
            } => {
                builder.ins().brif(
                    value(&values, *condition)?,
                    block_value(&blocks, then_block.0, "conditional branch target")?,
                    &[],
                    block_value(&blocks, else_block.0, "conditional branch target")?,
                    &[],
                );
            }
            Terminator::Return(result) => {
                builder.ins().return_(&[value(&values, *result)?]);
            }
            Terminator::Unreachable => {
                builder.ins().trap(TrapCode::unwrap_user(1));
            }
        }
    }
    builder.seal_all_blocks();
    builder.finalize();
    Ok(())
}

fn lower_binary(
    builder: &mut FunctionBuilder<'_>,
    operator: BinaryOperator,
    left: ir::Value,
    right: ir::Value,
    result_type: ir::Type,
) -> ir::Value {
    match operator {
        BinaryOperator::Add => builder.ins().iadd(left, right),
        BinaryOperator::Subtract => builder.ins().isub(left, right),
        BinaryOperator::Multiply => builder.ins().imul(left, right),
        BinaryOperator::Divide => builder.ins().sdiv(left, right),
        BinaryOperator::Remainder => builder.ins().srem(left, right),
        BinaryOperator::Less => {
            comparison(builder, IntCC::SignedLessThan, left, right, result_type)
        }
        BinaryOperator::LessEqual => comparison(
            builder,
            IntCC::SignedLessThanOrEqual,
            left,
            right,
            result_type,
        ),
        BinaryOperator::Greater => {
            comparison(builder, IntCC::SignedGreaterThan, left, right, result_type)
        }
        BinaryOperator::GreaterEqual => comparison(
            builder,
            IntCC::SignedGreaterThanOrEqual,
            left,
            right,
            result_type,
        ),
        BinaryOperator::Equal => comparison(builder, IntCC::Equal, left, right, result_type),
        BinaryOperator::NotEqual => comparison(builder, IntCC::NotEqual, left, right, result_type),
    }
}

fn comparison(
    builder: &mut FunctionBuilder<'_>,
    condition: IntCC,
    left: ir::Value,
    right: ir::Value,
    result_type: ir::Type,
) -> ir::Value {
    let boolean = builder.ins().icmp(condition, left, right);
    builder.ins().uextend(result_type, boolean)
}

fn clif_type(
    module: &Module,
    ty: TypeId,
    config: &EffectiveCompilationConfig,
) -> Result<ir::Type, String> {
    if !matches!(module.types.kind(ty), TypeKind::Int) {
        return Err(format!(
            "type `{}` cannot be lowered to a Cranelift scalar",
            module.types.display(ty)
        ));
    }
    let layout = module
        .types
        .layout(ty, config)
        .ok_or_else(|| format!("type `{}` has no target layout", module.types.display(ty)))?;
    match layout.size * 8 {
        8 => Ok(ir::types::I8),
        16 => Ok(ir::types::I16),
        32 => Ok(ir::types::I32),
        64 => Ok(ir::types::I64),
        bits => Err(format!("unsupported Cranelift integer width {bits}")),
    }
}

fn value_type(function: &Function, value: IrValueId) -> Result<TypeId, String> {
    function
        .value_types
        .get(value.0 as usize)
        .copied()
        .ok_or_else(|| format!("IR value v{} has no type", value.0))
}

fn value(values: &[Option<ir::Value>], id: IrValueId) -> Result<ir::Value, String> {
    values
        .get(id.0 as usize)
        .copied()
        .flatten()
        .ok_or_else(|| format!("IR value v{} is unavailable during lowering", id.0))
}

fn set_value(
    values: &mut [Option<ir::Value>],
    id: IrValueId,
    value: ir::Value,
) -> Result<(), String> {
    let slot = values
        .get_mut(id.0 as usize)
        .ok_or_else(|| format!("IR result v{} is out of range", id.0))?;
    *slot = Some(value);
    Ok(())
}

fn block_value<T: Copy>(values: &[T], id: u32, kind: &str) -> Result<T, String> {
    values
        .get(id as usize)
        .copied()
        .ok_or_else(|| format!("invalid {kind} block{id}"))
}

#[cfg(test)]
mod tests {
    use ccc_abi::plan;
    use ccc_ir::lower;
    use ccc_pp::lex;
    use ccc_sema::analyze;
    use ccc_session::SourceMap;
    use ccc_syntax::{convert_pp_tokens, parse};
    use object::{Object as _, ObjectSymbol as _};

    use super::*;

    #[test]
    fn emits_an_x86_64_object_and_clif() {
        let mut sources = SourceMap::new();
        let file = sources.add_file(
            "test.c",
            "int add(int a, int b) { return a + b; }\n\
             int main(void) { return add(40, 2); }",
        );
        let tokens = convert_pp_tokens(lex(file, sources.source(file).unwrap()).unwrap());
        let ir = lower(&analyze(&parse(&tokens).unwrap()).unwrap()).unwrap();
        let config = EffectiveCompilationConfig::default();
        let plans = plan(&ir, &config).unwrap();
        let output = emit(&ir, &plans, &config, Options { emit_clif: true }).unwrap();
        let object = object::File::parse(output.object.as_slice()).unwrap();
        assert_eq!(object.architecture(), object::Architecture::X86_64);
        assert!(object.symbols().any(|symbol| symbol.name() == Ok("main")));
        assert!(output.clif.contains("function main"));
        assert!(output.clif.contains("call"));
    }

    #[test]
    fn skips_clif_formatting_unless_requested() {
        let mut sources = SourceMap::new();
        let file = sources.add_file("test.c", "int main(void) { return 0; }");
        let tokens = convert_pp_tokens(lex(file, sources.source(file).unwrap()).unwrap());
        let ir = lower(&analyze(&parse(&tokens).unwrap()).unwrap()).unwrap();
        let config = EffectiveCompilationConfig::default();
        let plans = plan(&ir, &config).unwrap();
        assert!(
            emit(&ir, &plans, &config, Options::default())
                .unwrap()
                .clif
                .is_empty()
        );
    }

    #[test]
    fn rejects_a_target_incompatible_with_the_object_backend() {
        let mut sources = SourceMap::new();
        let file = sources.add_file("test.c", "int main(void) { return 0; }");
        let tokens = convert_pp_tokens(lex(file, sources.source(file).unwrap()).unwrap());
        let ir = lower(&analyze(&parse(&tokens).unwrap()).unwrap()).unwrap();
        let mut config = EffectiveCompilationConfig::default();
        config.target.triple.binary_format = BinaryFormat::Coff;
        let plans = plan(&ir, &config).unwrap();
        let error = emit(&ir, &plans, &config, Options::default()).unwrap_err();
        assert_eq!(error.code, "CCC4001");
        assert!(error.message.contains("object format"));
    }
}
