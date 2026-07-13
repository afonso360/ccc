//! Typed, ABI-independent control-flow IR and lowering from the typed AST.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt;

pub use ccc_sema::FunctionId;
use ccc_sema::{
    BinaryOperator as AstBinaryOperator, LocalId, TypedBlockItem, TypedExpression,
    TypedExpressionKind, TypedFunction, TypedStatement, TypedStatementKind, TypedTranslationUnit,
    UnaryOperator as AstUnaryOperator,
};
use ccc_session::Span;
use ccc_types::{TypeId, TypeKind, TypeStore};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct BlockId(pub u32);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ValueId(pub u32);

#[derive(Clone, Debug)]
pub struct Module {
    pub types: TypeStore,
    pub functions: Vec<Function>,
}

#[derive(Clone, Debug)]
pub struct Function {
    pub id: FunctionId,
    pub name: String,
    pub parameter_types: Vec<TypeId>,
    pub result_type: TypeId,
    pub parameters: Vec<ValueId>,
    pub blocks: Vec<Block>,
    pub entry: Option<BlockId>,
    pub value_count: u32,
    pub value_types: Vec<TypeId>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct Block {
    pub id: BlockId,
    pub parameters: Vec<ValueId>,
    pub instructions: Vec<Instruction>,
    pub terminator: Option<Terminator>,
}

#[derive(Clone, Debug)]
pub struct Instruction {
    pub result: ValueId,
    pub ty: TypeId,
    pub kind: InstructionKind,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum InstructionKind {
    Integer(i32),
    Unary {
        operator: UnaryOperator,
        operand: ValueId,
    },
    Binary {
        operator: BinaryOperator,
        left: ValueId,
        right: ValueId,
    },
    Call {
        function: FunctionId,
        arguments: Vec<ValueId>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnaryOperator {
    Negate,
    LogicalNot,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BinaryOperator {
    Add,
    Subtract,
    Multiply,
    Divide,
    Remainder,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    Equal,
    NotEqual,
}

#[derive(Clone, Debug)]
pub enum Terminator {
    Branch {
        target: BlockId,
        arguments: Vec<ValueId>,
    },
    ConditionalBranch {
        condition: ValueId,
        then_block: BlockId,
        else_block: BlockId,
    },
    Return(ValueId),
    Unreachable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IrError {
    pub code: &'static str,
    pub message: String,
}

#[derive(Clone, Copy, Debug)]
enum Definition {
    BlockParameter(BlockId),
    Instruction(BlockId, usize),
}

impl fmt::Display for IrError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.message.fmt(formatter)
    }
}

impl std::error::Error for IrError {}

pub fn lower(unit: &TypedTranslationUnit) -> Result<Module, IrError> {
    lower_inner(unit).map_err(|message| IrError {
        code: "CCC3001",
        message,
    })
}

fn lower_inner(unit: &TypedTranslationUnit) -> Result<Module, String> {
    let functions = unit
        .functions
        .iter()
        .map(|function| FunctionBuilder::lower(function, &unit.types))
        .collect::<Result<Vec<_>, _>>()?;
    let module = Module {
        types: unit.types.clone(),
        functions,
    };
    verify_inner(&module)?;
    Ok(module)
}

struct FunctionBuilder {
    function: Function,
    current: Option<BlockId>,
    environment: BTreeMap<LocalId, ValueId>,
}

struct MergePath {
    block: BlockId,
    environment: BTreeMap<LocalId, ValueId>,
    results: Vec<ValueId>,
}

impl FunctionBuilder {
    fn lower(function: &TypedFunction, types: &TypeStore) -> Result<Function, String> {
        let TypeKind::Function { result, parameters } = types.kind(function.signature) else {
            return Err(format!("`{}` does not have a function type", function.name));
        };
        let result_type = *result;
        let parameter_types = parameters.clone();
        if function.body.is_none() {
            return Ok(Function {
                id: function.id,
                name: function.name.clone(),
                parameter_types,
                result_type,
                parameters: Vec::new(),
                blocks: Vec::new(),
                entry: None,
                value_count: 0,
                value_types: Vec::new(),
                span: function.span,
            });
        }

        let mut builder = Self {
            function: Function {
                id: function.id,
                name: function.name.clone(),
                parameter_types,
                result_type,
                parameters: Vec::new(),
                blocks: Vec::new(),
                entry: None,
                value_count: 0,
                value_types: Vec::new(),
                span: function.span,
            },
            current: None,
            environment: BTreeMap::new(),
        };
        let entry = builder.new_block();
        builder.function.entry = Some(entry);
        builder.current = Some(entry);
        for (parameter, ty) in function
            .parameters
            .iter()
            .zip(builder.function.parameter_types.clone())
        {
            let value = builder.add_block_parameter(entry, ty);
            builder.function.parameters.push(value);
            builder.environment.insert(parameter.local, value);
        }
        builder.statement(function.body.as_ref().expect("body was checked"))?;
        if builder.current.is_some() {
            return Err(format!(
                "typed function `{}` has a reachable fall-through path",
                function.name
            ));
        }
        Ok(builder.function)
    }

    fn statement(&mut self, statement: &TypedStatement) -> Result<(), String> {
        if self.current.is_none() {
            return Ok(());
        }
        match &statement.kind {
            TypedStatementKind::Compound(items) => {
                let locals_before = self.environment.keys().copied().collect::<BTreeSet<_>>();
                for item in items {
                    if self.current.is_none() {
                        break;
                    }
                    match item {
                        TypedBlockItem::Declaration(declaration) => {
                            if let Some(initializer) = &declaration.initializer {
                                if expression_reads_local(initializer, declaration.local) {
                                    let initial = self.instruction(
                                        InstructionKind::Integer(0),
                                        initializer.ty,
                                        declaration.span,
                                    )?;
                                    self.environment.insert(declaration.local, initial);
                                }
                                let value = self.expression(initializer)?;
                                self.environment.insert(declaration.local, value);
                            } else {
                                let initial = self.instruction(
                                    InstructionKind::Integer(0),
                                    TypeId::INT,
                                    declaration.span,
                                )?;
                                self.environment.insert(declaration.local, initial);
                            }
                        }
                        TypedBlockItem::Statement(statement) => self.statement(statement)?,
                    }
                }
                self.environment
                    .retain(|local, _| locals_before.contains(local));
            }
            TypedStatementKind::Expression(expression) => {
                if let Some(expression) = expression {
                    self.expression(expression)?;
                }
            }
            TypedStatementKind::If {
                condition,
                then_statement,
                else_statement,
            } => self.if_statement(condition, then_statement, else_statement.as_deref())?,
            TypedStatementKind::While { condition, body } => {
                self.while_statement(condition, body)?;
            }
            TypedStatementKind::Return(expression) => {
                let value = self.expression(expression)?;
                self.terminate(Terminator::Return(value))?;
            }
        }
        Ok(())
    }

    fn if_statement(
        &mut self,
        condition: &TypedExpression,
        then_statement: &TypedStatement,
        else_statement: Option<&TypedStatement>,
    ) -> Result<(), String> {
        let condition = self.expression(condition)?;
        let environment = self.environment.clone();
        let then_block = self.new_block();
        let else_block = self.new_block();
        self.terminate(Terminator::ConditionalBranch {
            condition,
            then_block,
            else_block,
        })?;

        self.current = Some(then_block);
        self.environment = environment.clone();
        self.statement(then_statement)?;
        let then_path = self.current.map(|block| MergePath {
            block,
            environment: self.environment.clone(),
            results: Vec::new(),
        });

        self.current = Some(else_block);
        self.environment = environment;
        if let Some(else_statement) = else_statement {
            self.statement(else_statement)?;
        }
        let else_path = self.current.map(|block| MergePath {
            block,
            environment: self.environment.clone(),
            results: Vec::new(),
        });

        self.merge_paths([then_path, else_path].into_iter().flatten().collect())
            .map(|_| ())
    }

    fn while_statement(
        &mut self,
        condition: &TypedExpression,
        body: &TypedStatement,
    ) -> Result<(), String> {
        let incoming = self
            .environment
            .iter()
            .map(|(local, value)| (*local, *value))
            .collect::<Vec<_>>();
        let header = self.new_block();
        let mut header_environment = BTreeMap::new();
        for (local, incoming_value) in &incoming {
            let ty = self.value_type(*incoming_value)?;
            let parameter = self.add_block_parameter(header, ty);
            header_environment.insert(*local, parameter);
        }
        self.terminate(Terminator::Branch {
            target: header,
            arguments: incoming.iter().map(|(_, value)| *value).collect(),
        })?;
        self.current = Some(header);
        self.environment = header_environment;

        let condition = self.expression(condition)?;
        let condition_environment = self.environment.clone();
        let body_block = self.new_block();
        let exit_block = self.new_block();
        self.terminate(Terminator::ConditionalBranch {
            condition,
            then_block: body_block,
            else_block: exit_block,
        })?;

        self.current = Some(body_block);
        self.environment = condition_environment.clone();
        self.statement(body)?;
        if self.current.is_some() {
            let arguments = incoming
                .iter()
                .map(|(local, _)| {
                    self.environment
                        .get(local)
                        .copied()
                        .ok_or_else(|| format!("local {:?} is missing on a loop backedge", local))
                })
                .collect::<Result<_, _>>()?;
            self.terminate(Terminator::Branch {
                target: header,
                arguments,
            })?;
        }

        self.current = Some(exit_block);
        self.environment = condition_environment;
        Ok(())
    }

    fn expression(&mut self, expression: &TypedExpression) -> Result<ValueId, String> {
        match &expression.kind {
            TypedExpressionKind::Integer(value) => self.instruction(
                InstructionKind::Integer(*value),
                expression.ty,
                expression.span,
            ),
            TypedExpressionKind::LoadLocal(local) => self
                .environment
                .get(local)
                .copied()
                .ok_or_else(|| format!("local {:?} has no SSA value", local)),
            TypedExpressionKind::Unary { operator, operand } => {
                let operand = self.expression(operand)?;
                match operator {
                    AstUnaryOperator::Plus => Ok(operand),
                    AstUnaryOperator::Negate => self.instruction(
                        InstructionKind::Unary {
                            operator: UnaryOperator::Negate,
                            operand,
                        },
                        expression.ty,
                        expression.span,
                    ),
                    AstUnaryOperator::LogicalNot => self.instruction(
                        InstructionKind::Unary {
                            operator: UnaryOperator::LogicalNot,
                            operand,
                        },
                        expression.ty,
                        expression.span,
                    ),
                }
            }
            TypedExpressionKind::Binary {
                operator: AstBinaryOperator::LogicalAnd,
                left,
                right,
            } => self.logical_expression(false, left, right, expression.ty, expression.span),
            TypedExpressionKind::Binary {
                operator: AstBinaryOperator::LogicalOr,
                left,
                right,
            } => self.logical_expression(true, left, right, expression.ty, expression.span),
            TypedExpressionKind::Binary {
                operator,
                left,
                right,
            } => {
                let left = self.expression(left)?;
                let right = self.expression(right)?;
                self.instruction(
                    InstructionKind::Binary {
                        operator: lower_binary_operator(*operator)?,
                        left,
                        right,
                    },
                    expression.ty,
                    expression.span,
                )
            }
            TypedExpressionKind::Assign { local, value } => {
                let value = self.expression(value)?;
                self.environment.insert(*local, value);
                Ok(value)
            }
            TypedExpressionKind::Call {
                function,
                arguments,
            } => {
                let arguments = arguments
                    .iter()
                    .map(|argument| self.expression(argument))
                    .collect::<Result<Vec<_>, _>>()?;
                self.instruction(
                    InstructionKind::Call {
                        function: *function,
                        arguments,
                    },
                    expression.ty,
                    expression.span,
                )
            }
        }
    }

    fn logical_expression(
        &mut self,
        short_value: bool,
        left: &TypedExpression,
        right: &TypedExpression,
        ty: TypeId,
        span: Span,
    ) -> Result<ValueId, String> {
        let left = self.expression(left)?;
        let environment = self.environment.clone();
        let right_block = self.new_block();
        let short_block = self.new_block();
        let (then_block, else_block) = if short_value {
            (short_block, right_block)
        } else {
            (right_block, short_block)
        };
        self.terminate(Terminator::ConditionalBranch {
            condition: left,
            then_block,
            else_block,
        })?;

        self.current = Some(short_block);
        self.environment = environment.clone();
        let short_result =
            self.instruction(InstructionKind::Integer(i32::from(short_value)), ty, span)?;
        let short_path = MergePath {
            block: self.current.expect("short block is reachable"),
            environment: self.environment.clone(),
            results: vec![short_result],
        };

        self.current = Some(right_block);
        self.environment = environment;
        let right_result = self.expression(right)?;
        let zero = self.instruction(InstructionKind::Integer(0), ty, span)?;
        let normalized = self.instruction(
            InstructionKind::Binary {
                operator: BinaryOperator::NotEqual,
                left: right_result,
                right: zero,
            },
            ty,
            span,
        )?;
        let right_path = MergePath {
            block: self.current.expect("expression leaves a reachable block"),
            environment: self.environment.clone(),
            results: vec![normalized],
        };

        self.merge_paths(vec![short_path, right_path])?
            .into_iter()
            .next()
            .ok_or_else(|| "logical-expression merge did not produce a result".to_owned())
    }

    fn merge_paths(&mut self, paths: Vec<MergePath>) -> Result<Vec<ValueId>, String> {
        match paths.as_slice() {
            [] => {
                self.current = None;
                self.environment.clear();
                Ok(Vec::new())
            }
            [path] => {
                self.current = Some(path.block);
                self.environment = path.environment.clone();
                Ok(path.results.clone())
            }
            _ => {
                let merge = self.new_block();
                let result_count = paths[0].results.len();
                if paths.iter().any(|path| path.results.len() != result_count) {
                    return Err("control-flow paths produce different result counts".to_owned());
                }
                let mut results = Vec::with_capacity(result_count);
                for source in &paths[0].results {
                    let ty = self.value_type(*source)?;
                    results.push(self.add_block_parameter(merge, ty));
                }

                let locals = sorted_locals(&paths[0].environment);
                let mut merged = BTreeMap::new();
                for local in &locals {
                    let source = paths[0].environment.get(local).copied().ok_or_else(|| {
                        format!("local {:?} is missing at control-flow merge", local)
                    })?;
                    let ty = self.value_type(source)?;
                    let parameter = self.add_block_parameter(merge, ty);
                    merged.insert(*local, parameter);
                }
                for path in paths {
                    let mut arguments = path.results;
                    arguments.extend(
                        locals
                            .iter()
                            .map(|local| {
                                path.environment.get(local).copied().ok_or_else(|| {
                                    format!("local {:?} is missing at control-flow merge", local)
                                })
                            })
                            .collect::<Result<Vec<_>, _>>()?,
                    );
                    self.set_terminator_at(
                        path.block,
                        Terminator::Branch {
                            target: merge,
                            arguments,
                        },
                    )?;
                }
                self.current = Some(merge);
                self.environment = merged;
                Ok(results)
            }
        }
    }

    fn new_block(&mut self) -> BlockId {
        let id = BlockId(self.function.blocks.len() as u32);
        self.function.blocks.push(Block {
            id,
            parameters: Vec::new(),
            instructions: Vec::new(),
            terminator: None,
        });
        id
    }

    fn add_block_parameter(&mut self, block: BlockId, ty: TypeId) -> ValueId {
        let value = self.new_value(ty);
        self.function.blocks[block.0 as usize]
            .parameters
            .push(value);
        value
    }

    fn instruction(
        &mut self,
        kind: InstructionKind,
        ty: TypeId,
        span: Span,
    ) -> Result<ValueId, String> {
        let block = self
            .current
            .ok_or_else(|| "cannot append an instruction to terminated control flow".to_owned())?;
        let result = self.new_value(ty);
        self.function.blocks[block.0 as usize]
            .instructions
            .push(Instruction {
                result,
                ty,
                kind,
                span,
            });
        Ok(result)
    }

    fn new_value(&mut self, ty: TypeId) -> ValueId {
        let value = ValueId(self.function.value_count);
        self.function.value_count += 1;
        self.function.value_types.push(ty);
        value
    }

    fn value_type(&self, value: ValueId) -> Result<TypeId, String> {
        self.function
            .value_types
            .get(value.0 as usize)
            .copied()
            .ok_or_else(|| format!("value v{} has no type", value.0))
    }

    fn terminate(&mut self, terminator: Terminator) -> Result<(), String> {
        let block = self
            .current
            .take()
            .ok_or_else(|| "control flow is already terminated".to_owned())?;
        self.set_terminator_at(block, terminator)
    }

    fn set_terminator_at(&mut self, block: BlockId, terminator: Terminator) -> Result<(), String> {
        let slot = &mut self.function.blocks[block.0 as usize].terminator;
        if slot.is_some() {
            return Err(format!("block{} already has a terminator", block.0));
        }
        *slot = Some(terminator);
        Ok(())
    }
}

fn sorted_locals(environment: &BTreeMap<LocalId, ValueId>) -> Vec<LocalId> {
    environment.keys().copied().collect()
}

fn expression_reads_local(expression: &TypedExpression, local: LocalId) -> bool {
    match &expression.kind {
        TypedExpressionKind::Integer(_) => false,
        TypedExpressionKind::LoadLocal(loaded) => *loaded == local,
        TypedExpressionKind::Unary { operand, .. } => expression_reads_local(operand, local),
        TypedExpressionKind::Binary { left, right, .. } => {
            expression_reads_local(left, local) || expression_reads_local(right, local)
        }
        TypedExpressionKind::Assign {
            local: assigned,
            value,
        } => *assigned == local || expression_reads_local(value, local),
        TypedExpressionKind::Call { arguments, .. } => arguments
            .iter()
            .any(|argument| expression_reads_local(argument, local)),
    }
}

fn lower_binary_operator(operator: AstBinaryOperator) -> Result<BinaryOperator, String> {
    match operator {
        AstBinaryOperator::Add => Ok(BinaryOperator::Add),
        AstBinaryOperator::Subtract => Ok(BinaryOperator::Subtract),
        AstBinaryOperator::Multiply => Ok(BinaryOperator::Multiply),
        AstBinaryOperator::Divide => Ok(BinaryOperator::Divide),
        AstBinaryOperator::Remainder => Ok(BinaryOperator::Remainder),
        AstBinaryOperator::Less => Ok(BinaryOperator::Less),
        AstBinaryOperator::LessEqual => Ok(BinaryOperator::LessEqual),
        AstBinaryOperator::Greater => Ok(BinaryOperator::Greater),
        AstBinaryOperator::GreaterEqual => Ok(BinaryOperator::GreaterEqual),
        AstBinaryOperator::Equal => Ok(BinaryOperator::Equal),
        AstBinaryOperator::NotEqual => Ok(BinaryOperator::NotEqual),
        AstBinaryOperator::LogicalAnd | AstBinaryOperator::LogicalOr => {
            Err("logical operator reached scalar IR instruction lowering".to_owned())
        }
    }
}

pub fn verify(module: &Module) -> Result<(), IrError> {
    verify_inner(module).map_err(|message| IrError {
        code: "CCC3002",
        message,
    })
}

fn verify_inner(module: &Module) -> Result<(), String> {
    let mut functions = HashMap::with_capacity(module.functions.len());
    for function in &module.functions {
        if functions.insert(function.id, function).is_some() {
            return Err(format!("duplicate function id {}", function.id.0));
        }
    }
    for function in &module.functions {
        if !module.types.contains(function.result_type)
            || function
                .parameter_types
                .iter()
                .any(|ty| !module.types.contains(*ty))
        {
            return Err(format!("function `{}` has an invalid type", function.name));
        }
        if function.entry.is_none() {
            if !function.blocks.is_empty() {
                return Err(format!("declaration `{}` contains blocks", function.name));
            }
            continue;
        }
        if function.blocks.is_empty() {
            return Err(format!("definition `{}` has no blocks", function.name));
        }
        if function.parameters.len() != function.parameter_types.len() {
            return Err(format!(
                "definition `{}` has a parameter mismatch",
                function.name
            ));
        }
        if function.value_types.len() != function.value_count as usize {
            return Err(format!(
                "function `{}` has an incomplete value-type table",
                function.name
            ));
        }
        let mut values = HashSet::new();
        let mut definitions = HashMap::with_capacity(function.value_count as usize);
        for (block_index, block) in function.blocks.iter().enumerate() {
            if block.id.0 as usize != block_index {
                return Err(format!(
                    "block{} is out of order in `{}`",
                    block.id.0, function.name
                ));
            }
            for value in &block.parameters {
                ensure_type(module, value_type(function, *value)?)?;
                if !values.insert(*value) {
                    return Err(format!("duplicate value v{}", value.0));
                }
                definitions.insert(*value, Definition::BlockParameter(block.id));
            }
            for (instruction_index, instruction) in block.instructions.iter().enumerate() {
                ensure_type(module, instruction.ty)?;
                if !values.insert(instruction.result) {
                    return Err(format!("duplicate value v{}", instruction.result.0));
                }
                definitions.insert(
                    instruction.result,
                    Definition::Instruction(block.id, instruction_index),
                );
                if value_type(function, instruction.result)? != instruction.ty {
                    return Err(format!(
                        "result v{} has an inconsistent type",
                        instruction.result.0
                    ));
                }
            }
            if block.terminator.is_none() {
                return Err(format!("block{} has no terminator", block.id.0));
            }
        }
        let dominators = dominators(function)?;
        for block in &function.blocks {
            for (instruction_index, instruction) in block.instructions.iter().enumerate() {
                match &instruction.kind {
                    InstructionKind::Integer(_) => {}
                    InstructionKind::Unary { operand, .. } => {
                        ensure_use(
                            function,
                            &values,
                            &definitions,
                            &dominators,
                            *operand,
                            block.id,
                            Some(instruction_index),
                        )?;
                        ensure_same_type(function, *operand, instruction.result)?;
                    }
                    InstructionKind::Binary { left, right, .. } => {
                        for operand in [*left, *right] {
                            ensure_use(
                                function,
                                &values,
                                &definitions,
                                &dominators,
                                operand,
                                block.id,
                                Some(instruction_index),
                            )?;
                        }
                        ensure_same_type(function, *left, *right)?;
                        ensure_same_type(function, *left, instruction.result)?;
                    }
                    InstructionKind::Call {
                        function: callee,
                        arguments,
                    } => {
                        let target = functions
                            .get(callee)
                            .ok_or_else(|| format!("invalid call target {}", callee.0))?;
                        if target.parameter_types.len() != arguments.len() {
                            return Err(format!("call to `{}` has wrong arity", target.name));
                        }
                        for (argument, parameter_type) in
                            arguments.iter().zip(&target.parameter_types)
                        {
                            ensure_use(
                                function,
                                &values,
                                &definitions,
                                &dominators,
                                *argument,
                                block.id,
                                Some(instruction_index),
                            )?;
                            if value_type(function, *argument)? != *parameter_type {
                                return Err(format!(
                                    "call to `{}` has a type mismatch",
                                    target.name
                                ));
                            }
                        }
                        if instruction.ty != target.result_type {
                            return Err(format!(
                                "call to `{}` has a result-type mismatch",
                                target.name
                            ));
                        }
                    }
                }
            }
            verify_terminator(
                function,
                &values,
                &definitions,
                &dominators,
                block,
                block.terminator.as_ref().unwrap(),
            )?;
        }
    }
    Ok(())
}

fn verify_terminator(
    function: &Function,
    values: &HashSet<ValueId>,
    definitions: &HashMap<ValueId, Definition>,
    dominators: &[HashSet<usize>],
    block: &Block,
    terminator: &Terminator,
) -> Result<(), String> {
    match terminator {
        Terminator::Branch { target, arguments } => {
            let target = function
                .blocks
                .get(target.0 as usize)
                .ok_or_else(|| "branch target is invalid".to_owned())?;
            if target.parameters.len() != arguments.len() {
                return Err(format!("branch to block{} has wrong arity", target.id.0));
            }
            for (argument, parameter) in arguments.iter().zip(&target.parameters) {
                ensure_use(
                    function,
                    values,
                    definitions,
                    dominators,
                    *argument,
                    block.id,
                    None,
                )?;
                ensure_same_type(function, *argument, *parameter)?;
            }
        }
        Terminator::ConditionalBranch {
            condition,
            then_block,
            else_block,
        } => {
            ensure_use(
                function,
                values,
                definitions,
                dominators,
                *condition,
                block.id,
                None,
            )?;
            for target in [then_block, else_block] {
                let block = function
                    .blocks
                    .get(target.0 as usize)
                    .ok_or_else(|| "conditional branch target is invalid".to_owned())?;
                if !block.parameters.is_empty() {
                    return Err(format!(
                        "conditional branch target block{} unexpectedly has parameters",
                        block.id.0
                    ));
                }
            }
        }
        Terminator::Return(value) => {
            ensure_use(
                function,
                values,
                definitions,
                dominators,
                *value,
                block.id,
                None,
            )?;
            if value_type(function, *value)? != function.result_type {
                return Err(format!(
                    "return from `{}` has the wrong type",
                    function.name
                ));
            }
        }
        Terminator::Unreachable => {}
    }
    Ok(())
}

fn ensure_type(module: &Module, ty: TypeId) -> Result<(), String> {
    if !module.types.contains(ty) {
        return Err(format!("invalid type id {ty:?}"));
    }
    Ok(())
}

fn ensure_same_type(function: &Function, left: ValueId, right: ValueId) -> Result<(), String> {
    if value_type(function, left)? != value_type(function, right)? {
        return Err(format!("v{} and v{} have different types", left.0, right.0));
    }
    Ok(())
}

fn value_type(function: &Function, value: ValueId) -> Result<TypeId, String> {
    function
        .value_types
        .get(value.0 as usize)
        .copied()
        .ok_or_else(|| format!("value v{} has no type", value.0))
}

fn ensure_value(
    function: &Function,
    values: &HashSet<ValueId>,
    value: ValueId,
) -> Result<(), String> {
    if value.0 >= function.value_count || !values.contains(&value) {
        return Err(format!("use of invalid value v{}", value.0));
    }
    Ok(())
}

fn ensure_use(
    function: &Function,
    values: &HashSet<ValueId>,
    definitions: &HashMap<ValueId, Definition>,
    dominators: &[HashSet<usize>],
    value: ValueId,
    use_block: BlockId,
    before_instruction: Option<usize>,
) -> Result<(), String> {
    ensure_value(function, values, value)?;
    let definition = definitions
        .get(&value)
        .copied()
        .ok_or_else(|| format!("value v{} has no definition", value.0))?;
    let definition_block = match definition {
        Definition::BlockParameter(block) => block,
        Definition::Instruction(block, instruction) => {
            if block == use_block {
                if before_instruction.is_some_and(|use_index| instruction >= use_index) {
                    return Err(format!("v{} is used before its definition", value.0));
                }
                return Ok(());
            }
            block
        }
    };
    if definition_block == use_block {
        return Ok(());
    }
    let dominates = dominators
        .get(use_block.0 as usize)
        .is_some_and(|blocks| blocks.contains(&(definition_block.0 as usize)));
    if !dominates {
        return Err(format!(
            "definition of v{} does not dominate its use in block{}",
            value.0, use_block.0
        ));
    }
    Ok(())
}

fn dominators(function: &Function) -> Result<Vec<HashSet<usize>>, String> {
    let entry = function
        .entry
        .ok_or_else(|| format!("definition `{}` has no entry block", function.name))?
        .0 as usize;
    if entry >= function.blocks.len() {
        return Err(format!(
            "definition `{}` has an invalid entry block",
            function.name
        ));
    }
    let mut predecessors = vec![Vec::new(); function.blocks.len()];
    for block in &function.blocks {
        match block.terminator.as_ref().expect("terminators were checked") {
            Terminator::Branch { target, .. } => {
                let target = target.0 as usize;
                if target >= predecessors.len() {
                    return Err("branch target is invalid".to_owned());
                }
                predecessors[target].push(block.id.0 as usize);
            }
            Terminator::ConditionalBranch {
                then_block,
                else_block,
                ..
            } => {
                for target in [then_block, else_block] {
                    let target = target.0 as usize;
                    if target >= predecessors.len() {
                        return Err("conditional branch target is invalid".to_owned());
                    }
                    predecessors[target].push(block.id.0 as usize);
                }
            }
            Terminator::Return(_) | Terminator::Unreachable => {}
        }
    }

    let all = (0..function.blocks.len()).collect::<HashSet<_>>();
    let mut result = vec![all; function.blocks.len()];
    result[entry] = HashSet::from([entry]);
    loop {
        let mut changed = false;
        for block in 0..function.blocks.len() {
            if block == entry {
                continue;
            }
            let mut updated = if let Some((first, rest)) = predecessors[block].split_first() {
                let mut intersection = result[*first].clone();
                for predecessor in rest {
                    intersection.retain(|candidate| result[*predecessor].contains(candidate));
                }
                intersection
            } else {
                HashSet::new()
            };
            updated.insert(block);
            if updated != result[block] {
                result[block] = updated;
                changed = true;
            }
        }
        if !changed {
            return Ok(result);
        }
    }
}

pub fn dump(module: &Module) -> String {
    let mut output = String::new();
    for function in &module.functions {
        if function.entry.is_none() {
            let parameters = function
                .parameter_types
                .iter()
                .map(|ty| module.types.display(*ty))
                .collect::<Vec<_>>()
                .join(", ");
            output.push_str(&format!(
                "declare @{}({parameters}) -> {}\n",
                function.name,
                module.types.display(function.result_type)
            ));
            continue;
        }
        let parameters = function
            .parameters
            .iter()
            .map(|value| {
                format!(
                    "v{}: {}",
                    value.0,
                    module.types.display(function.value_types[value.0 as usize])
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        output.push_str(&format!(
            "function @{}({parameters}) -> {} {{\n",
            function.name,
            module.types.display(function.result_type)
        ));
        for block in &function.blocks {
            let parameters = block
                .parameters
                .iter()
                .map(|value| {
                    format!(
                        "v{}: {}",
                        value.0,
                        module.types.display(function.value_types[value.0 as usize])
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            output.push_str(&format!("  block{}({parameters}):\n", block.id.0));
            for instruction in &block.instructions {
                output.push_str(&format!(
                    "    v{} = {}\n",
                    instruction.result.0,
                    display_instruction(module, &instruction.kind)
                ));
            }
            output.push_str(&format!(
                "    {}\n",
                display_terminator(block.terminator.as_ref().unwrap())
            ));
        }
        output.push_str("}\n");
    }
    output
}

fn display_instruction(module: &Module, kind: &InstructionKind) -> String {
    match kind {
        InstructionKind::Integer(value) => format!("iconst {value}"),
        InstructionKind::Unary { operator, operand } => {
            format!("{} v{}", unary_name(*operator), operand.0)
        }
        InstructionKind::Binary {
            operator,
            left,
            right,
        } => format!("{} v{}, v{}", binary_name(*operator), left.0, right.0),
        InstructionKind::Call {
            function,
            arguments,
        } => format!(
            "call @{}({})",
            module
                .functions
                .iter()
                .find(|callee| callee.id == *function)
                .map_or_else(
                    || format!("<invalid:{}>", function.0),
                    |callee| callee.name.clone()
                ),
            arguments
                .iter()
                .map(|value| format!("v{}", value.0))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn display_terminator(terminator: &Terminator) -> String {
    match terminator {
        Terminator::Branch { target, arguments } => format!(
            "br block{}({})",
            target.0,
            arguments
                .iter()
                .map(|value| format!("v{}", value.0))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Terminator::ConditionalBranch {
            condition,
            then_block,
            else_block,
        } => format!(
            "condbr v{}, block{}, block{}",
            condition.0, then_block.0, else_block.0
        ),
        Terminator::Return(value) => format!("return v{}", value.0),
        Terminator::Unreachable => "unreachable".to_owned(),
    }
}

fn unary_name(operator: UnaryOperator) -> &'static str {
    match operator {
        UnaryOperator::Negate => "neg",
        UnaryOperator::LogicalNot => "logical-not",
    }
}

fn binary_name(operator: BinaryOperator) -> &'static str {
    match operator {
        BinaryOperator::Add => "add",
        BinaryOperator::Subtract => "sub",
        BinaryOperator::Multiply => "mul",
        BinaryOperator::Divide => "sdiv",
        BinaryOperator::Remainder => "srem",
        BinaryOperator::Less => "icmp-slt",
        BinaryOperator::LessEqual => "icmp-sle",
        BinaryOperator::Greater => "icmp-sgt",
        BinaryOperator::GreaterEqual => "icmp-sge",
        BinaryOperator::Equal => "icmp-eq",
        BinaryOperator::NotEqual => "icmp-ne",
    }
}

#[cfg(test)]
mod tests {
    use ccc_pp::lex;
    use ccc_sema::analyze;
    use ccc_session::SourceMap;
    use ccc_syntax::{convert_pp_tokens, parse};

    use super::*;

    fn lower_source(source: &str) -> Module {
        let mut sources = SourceMap::new();
        let file = sources.add_file("test.c", source);
        let tokens = convert_pp_tokens(lex(file, sources.source(file).unwrap()).unwrap());
        let typed = analyze(&parse(&tokens).unwrap()).unwrap();
        lower(&typed).unwrap()
    }

    fn assert_golden_ir(source: &str, expected: &str) {
        let module = lower_source(source);
        verify(&module).unwrap();
        assert_eq!(dump(&module), expected);
    }

    #[test]
    fn lowers_function_calls_to_golden_ir() {
        assert_golden_ir(
            "int add(int a, int b) { return a + b; }\n\
             int main(void) { return add(19, 23); }",
            concat!(
                "function @add(v0: int, v1: int) -> int {\n",
                "  block0(v0: int, v1: int):\n",
                "    v2 = add v0, v1\n",
                "    return v2\n",
                "}\n",
                "function @main() -> int {\n",
                "  block0():\n",
                "    v0 = iconst 19\n",
                "    v1 = iconst 23\n",
                "    v2 = call @add(v0, v1)\n",
                "    return v2\n",
                "}\n",
            ),
        );
    }

    #[test]
    fn lowers_loop_assignments_to_golden_ssa_ir() {
        assert_golden_ir(
            "int main(void) { int x = 4; int sum = 0;\
             while (x = x - 1) sum = sum + x; return sum; }",
            concat!(
                "function @main() -> int {\n",
                "  block0():\n",
                "    v0 = iconst 4\n",
                "    v1 = iconst 0\n",
                "    br block1(v0, v1)\n",
                "  block1(v2: int, v3: int):\n",
                "    v4 = iconst 1\n",
                "    v5 = sub v2, v4\n",
                "    condbr v5, block2, block3\n",
                "  block2():\n",
                "    v6 = add v3, v5\n",
                "    br block1(v5, v6)\n",
                "  block3():\n",
                "    return v3\n",
                "}\n",
            ),
        );
    }

    #[test]
    fn lowers_loops_short_circuit_and_calls_to_verified_cfg() {
        let module = lower_source(
            "int add(int a, int b) { return a + b; }\n\
             int main(void) { int x = 0; while (x < 3) x = x + 1;\
             if (x && add(x, 2)) return x; return 0; }",
        );
        verify(&module).unwrap();
        let text = dump(&module);
        assert!(text.contains("condbr"));
        assert!(text.contains("call @add"));
        assert!(text.contains("br block"));
    }

    #[test]
    fn verifier_rejects_a_value_used_before_its_definition() {
        let mut module = lower_source("int main(void) { return 1 + 2; }");
        let block = &mut module.functions[0].blocks[0];
        let later = block.instructions[2].result;
        block.instructions[0].kind = InstructionKind::Unary {
            operator: UnaryOperator::Negate,
            operand: later,
        };
        let error = verify(&module).unwrap_err();
        assert_eq!(error.code, "CCC3002");
        assert!(error.message.contains("before its definition"));
    }
}
