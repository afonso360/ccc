//! Semantic analysis and typed-AST construction for the supported scalar C subset.

use std::collections::{HashMap, HashSet};

use ccc_diag::Diagnostic;
use ccc_session::Span;
pub use ccc_syntax::{BinaryOperator, UnaryOperator};
use ccc_syntax::{
    BlockItem, Expression, ExpressionKind, FunctionDeclaration, LocalDeclaration, Statement,
    StatementKind, TranslationUnit,
};
use ccc_target::EffectiveCompilationConfig;
use ccc_types::{TypeId, TypeStore};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct FunctionId(pub u32);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct LocalId(pub u32);

#[derive(Clone, Debug)]
pub struct TypedTranslationUnit {
    pub types: TypeStore,
    pub functions: Vec<TypedFunction>,
}

#[derive(Clone, Debug)]
pub struct TypedFunction {
    pub id: FunctionId,
    pub name: String,
    pub signature: TypeId,
    pub parameters: Vec<TypedParameter>,
    pub body: Option<TypedStatement>,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypedParameter {
    pub local: LocalId,
    pub name: String,
    pub ty: TypeId,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypedLocalDeclaration {
    pub local: LocalId,
    pub name: String,
    pub initializer: Option<TypedExpression>,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TypedBlockItem {
    Declaration(TypedLocalDeclaration),
    Statement(TypedStatement),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypedStatement {
    pub kind: TypedStatementKind,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TypedStatementKind {
    Compound(Vec<TypedBlockItem>),
    Expression(Option<TypedExpression>),
    If {
        condition: TypedExpression,
        then_statement: Box<TypedStatement>,
        else_statement: Option<Box<TypedStatement>>,
    },
    While {
        condition: TypedExpression,
        body: Box<TypedStatement>,
    },
    Return(TypedExpression),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypedExpression {
    pub kind: TypedExpressionKind,
    pub ty: TypeId,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TypedExpressionKind {
    Integer(i32),
    LoadLocal(LocalId),
    Unary {
        operator: UnaryOperator,
        operand: Box<TypedExpression>,
    },
    Binary {
        operator: BinaryOperator,
        left: Box<TypedExpression>,
        right: Box<TypedExpression>,
    },
    Assign {
        local: LocalId,
        value: Box<TypedExpression>,
    },
    Call {
        function: FunctionId,
        arguments: Vec<TypedExpression>,
    },
}

#[derive(Clone, Copy)]
struct FunctionSymbol {
    id: FunctionId,
    signature: TypeId,
    parameter_count: usize,
}

pub fn analyze(unit: &TranslationUnit) -> Result<TypedTranslationUnit, Vec<Diagnostic>> {
    analyze_with_config(unit, &EffectiveCompilationConfig::default())
}

pub fn analyze_with_config(
    unit: &TranslationUnit,
    config: &EffectiveCompilationConfig,
) -> Result<TypedTranslationUnit, Vec<Diagnostic>> {
    let Some(int_width) = config.target.int_width() else {
        return Err(vec![Diagnostic::error(
            "CCC2012",
            "the configured target does not define a C `int` width",
        )]);
    };
    let mut analyzer = TranslationUnitAnalyzer {
        types: TypeStore::default(),
        functions: Vec::new(),
        symbols: HashMap::new(),
        definitions: HashSet::new(),
        int_max: signed_maximum(int_width),
    };
    let mut diagnostics = Vec::new();
    for declaration in &unit.declarations {
        if let Err(diagnostic) = analyzer.function_declaration(declaration) {
            diagnostics.push(diagnostic);
        }
    }
    if diagnostics.is_empty() {
        Ok(TypedTranslationUnit {
            types: analyzer.types,
            functions: analyzer.functions,
        })
    } else {
        Err(diagnostics)
    }
}

struct TranslationUnitAnalyzer {
    types: TypeStore,
    functions: Vec<TypedFunction>,
    symbols: HashMap<String, FunctionSymbol>,
    definitions: HashSet<FunctionId>,
    int_max: u64,
}

impl TranslationUnitAnalyzer {
    fn function_declaration(
        &mut self,
        declaration: &FunctionDeclaration,
    ) -> Result<(), Diagnostic> {
        let signature = self
            .types
            .function(TypeId::INT, vec![TypeId::INT; declaration.parameters.len()]);
        let symbol = if let Some(existing) = self.symbols.get(&declaration.name).copied() {
            if existing.signature != signature {
                return Err(error(
                    "CCC2001",
                    declaration.name_span,
                    format!("conflicting declaration of `{}`", declaration.name),
                ));
            }
            existing
        } else {
            let id = FunctionId(self.functions.len() as u32);
            let symbol = FunctionSymbol {
                id,
                signature,
                parameter_count: declaration.parameters.len(),
            };
            self.symbols.insert(declaration.name.clone(), symbol);
            self.functions.push(TypedFunction {
                id,
                name: declaration.name.clone(),
                signature,
                parameters: Vec::new(),
                body: None,
                span: declaration.span,
            });
            symbol
        };

        if declaration.body.is_none() {
            return Ok(());
        }
        if !self.definitions.insert(symbol.id) {
            return Err(error(
                "CCC2002",
                declaration.name_span,
                format!("redefinition of `{}`", declaration.name),
            ));
        }

        let mut body_analyzer = FunctionAnalyzer::new(&self.symbols, self.int_max);
        let parameters = body_analyzer.parameters(declaration)?;
        let body =
            body_analyzer.statement(declaration.body.as_ref().expect("body was checked"), true)?;
        let body = append_implicit_return(body, declaration.span);
        let function = &mut self.functions[symbol.id.0 as usize];
        function.parameters = parameters;
        function.body = Some(body);
        function.span = declaration.span;
        Ok(())
    }
}

struct FunctionAnalyzer<'a> {
    functions: &'a HashMap<String, FunctionSymbol>,
    scopes: Vec<HashMap<String, LocalId>>,
    next_local: u32,
    int_max: u64,
}

impl<'a> FunctionAnalyzer<'a> {
    fn new(functions: &'a HashMap<String, FunctionSymbol>, int_max: u64) -> Self {
        Self {
            functions,
            scopes: vec![HashMap::new()],
            next_local: 0,
            int_max,
        }
    }

    fn parameters(
        &mut self,
        declaration: &FunctionDeclaration,
    ) -> Result<Vec<TypedParameter>, Diagnostic> {
        let mut parameters = Vec::new();
        for parameter in &declaration.parameters {
            let Some(name) = &parameter.name else {
                return Err(error(
                    "CCC2003",
                    parameter.span,
                    "a parameter in a function definition needs a name",
                ));
            };
            let span = parameter.name_span.expect("named parameter has a span");
            let local = self.declare_local(name, span)?;
            parameters.push(TypedParameter {
                local,
                name: name.clone(),
                ty: TypeId::INT,
                span: parameter.span,
            });
        }
        Ok(parameters)
    }

    fn statement(
        &mut self,
        statement: &Statement,
        function_body: bool,
    ) -> Result<TypedStatement, Diagnostic> {
        let kind = match &statement.kind {
            StatementKind::Compound(items) => {
                if !function_body {
                    self.scopes.push(HashMap::new());
                }
                let result = items
                    .iter()
                    .map(|item| self.block_item(item))
                    .collect::<Result<Vec<_>, _>>();
                if !function_body {
                    self.scopes.pop();
                }
                TypedStatementKind::Compound(result?)
            }
            StatementKind::Expression(expression) => TypedStatementKind::Expression(
                expression
                    .as_ref()
                    .map(|expression| self.expression(expression))
                    .transpose()?,
            ),
            StatementKind::If {
                condition,
                then_statement,
                else_statement,
            } => TypedStatementKind::If {
                condition: self.expression(condition)?,
                then_statement: Box::new(self.statement(then_statement, false)?),
                else_statement: else_statement
                    .as_ref()
                    .map(|statement| self.statement(statement, false).map(Box::new))
                    .transpose()?,
            },
            StatementKind::While { condition, body } => TypedStatementKind::While {
                condition: self.expression(condition)?,
                body: Box::new(self.statement(body, false)?),
            },
            StatementKind::Return(Some(expression)) => {
                TypedStatementKind::Return(self.expression(expression)?)
            }
            StatementKind::Return(None) => {
                return Err(error(
                    "CCC2011",
                    statement.span,
                    "a non-void function must return a value",
                ));
            }
        };
        Ok(TypedStatement {
            kind,
            span: statement.span,
        })
    }

    fn block_item(&mut self, item: &BlockItem) -> Result<TypedBlockItem, Diagnostic> {
        match item {
            BlockItem::Declaration(declaration) => self
                .local_declaration(declaration)
                .map(TypedBlockItem::Declaration),
            BlockItem::Statement(statement) => self
                .statement(statement, false)
                .map(TypedBlockItem::Statement),
        }
    }

    fn local_declaration(
        &mut self,
        declaration: &LocalDeclaration,
    ) -> Result<TypedLocalDeclaration, Diagnostic> {
        let local = self.declare_local(&declaration.name, declaration.name_span)?;
        let initializer = declaration
            .initializer
            .as_ref()
            .map(|expression| self.expression(expression))
            .transpose()?;
        Ok(TypedLocalDeclaration {
            local,
            name: declaration.name.clone(),
            initializer,
            span: declaration.span,
        })
    }

    fn expression(&mut self, expression: &Expression) -> Result<TypedExpression, Diagnostic> {
        let kind = match &expression.kind {
            ExpressionKind::Integer { value, .. } => {
                if *value > self.int_max {
                    return Err(error(
                        "CCC2004",
                        expression.span,
                        "integer constant is outside the supported `int` range",
                    ));
                }
                let value = i32::try_from(*value).map_err(|_| {
                    error(
                        "CCC2004",
                        expression.span,
                        "integer constant is outside the supported `int` range",
                    )
                })?;
                TypedExpressionKind::Integer(value)
            }
            ExpressionKind::Name(name) => {
                let local = self.lookup_local(name).ok_or_else(|| {
                    error(
                        "CCC2005",
                        expression.span,
                        format!("use of undeclared identifier `{name}`"),
                    )
                })?;
                TypedExpressionKind::LoadLocal(local)
            }
            ExpressionKind::Unary {
                operator: UnaryOperator::Negate,
                operand,
            } if matches!(
                operand.kind,
                ExpressionKind::Integer { value, .. }
                    if value == self.int_max.saturating_add(1)
                        && self.int_max == i32::MAX as u64
            ) =>
            {
                TypedExpressionKind::Integer(i32::MIN)
            }
            ExpressionKind::Unary { operator, operand } => TypedExpressionKind::Unary {
                operator: *operator,
                operand: Box::new(self.expression(operand)?),
            },
            ExpressionKind::Binary {
                operator,
                left,
                right,
            } => TypedExpressionKind::Binary {
                operator: *operator,
                left: Box::new(self.expression(left)?),
                right: Box::new(self.expression(right)?),
            },
            ExpressionKind::Assign { target, value } => {
                let ExpressionKind::Name(name) = &target.kind else {
                    return Err(error(
                        "CCC2006",
                        target.span,
                        "assignment target is not a modifiable local object",
                    ));
                };
                let local = self.lookup_local(name).ok_or_else(|| {
                    error(
                        "CCC2005",
                        target.span,
                        format!("use of undeclared identifier `{name}`"),
                    )
                })?;
                TypedExpressionKind::Assign {
                    local,
                    value: Box::new(self.expression(value)?),
                }
            }
            ExpressionKind::Call { callee, arguments } => {
                let ExpressionKind::Name(name) = &callee.kind else {
                    return Err(error(
                        "CCC2007",
                        callee.span,
                        "only direct calls to declared functions are supported",
                    ));
                };
                if self.lookup_local(name).is_some() {
                    return Err(error(
                        "CCC2007",
                        callee.span,
                        format!("`{name}` names a local object, not a function"),
                    ));
                }
                let function = self.functions.get(name).copied().ok_or_else(|| {
                    error(
                        "CCC2008",
                        callee.span,
                        format!("call to undeclared function `{name}`"),
                    )
                })?;
                if function.parameter_count != arguments.len() {
                    return Err(error(
                        "CCC2009",
                        expression.span,
                        format!(
                            "function `{name}` expects {} arguments but {} were supplied",
                            function.parameter_count,
                            arguments.len()
                        ),
                    ));
                }
                TypedExpressionKind::Call {
                    function: function.id,
                    arguments: arguments
                        .iter()
                        .map(|argument| self.expression(argument))
                        .collect::<Result<_, _>>()?,
                }
            }
        };
        Ok(TypedExpression {
            kind,
            ty: TypeId::INT,
            span: expression.span,
        })
    }

    fn declare_local(&mut self, name: &str, span: Span) -> Result<LocalId, Diagnostic> {
        let scope = self.scopes.last_mut().expect("every function has a scope");
        if scope.contains_key(name) {
            return Err(error(
                "CCC2010",
                span,
                format!("redeclaration of `{name}` in the same scope"),
            ));
        }
        let local = LocalId(self.next_local);
        self.next_local += 1;
        scope.insert(name.to_owned(), local);
        Ok(local)
    }

    fn lookup_local(&self, name: &str) -> Option<LocalId> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).copied())
    }
}

fn error(code: &str, span: Span, message: impl Into<String>) -> Diagnostic {
    Diagnostic::error(code, message).with_primary(span, "here")
}

fn signed_maximum(width: u8) -> u64 {
    debug_assert!((2..=64).contains(&width));
    if width == 64 {
        i64::MAX as u64
    } else {
        (1_u64 << (width - 1)) - 1
    }
}

fn append_implicit_return(mut body: TypedStatement, function_span: Span) -> TypedStatement {
    let end = Span::with_origin(
        function_span.file,
        function_span.end,
        function_span.end,
        function_span.origin,
    );
    let return_statement = TypedStatement {
        kind: TypedStatementKind::Return(TypedExpression {
            kind: TypedExpressionKind::Integer(0),
            ty: TypeId::INT,
            span: end,
        }),
        span: end,
    };
    match &mut body.kind {
        TypedStatementKind::Compound(items) => {
            items.push(TypedBlockItem::Statement(return_statement));
        }
        _ => unreachable!("a parsed function body is always a compound statement"),
    }
    body
}

#[cfg(test)]
mod tests {
    use ccc_pp::lex;
    use ccc_session::SourceMap;
    use ccc_syntax::{convert_pp_tokens, parse};

    use super::*;

    fn analyze_source(source: &str) -> Result<TypedTranslationUnit, Vec<Diagnostic>> {
        let mut sources = SourceMap::new();
        let file = sources.add_file("test.c", source);
        let tokens = convert_pp_tokens(lex(file, sources.source(file).unwrap()).unwrap());
        analyze(&parse(&tokens).unwrap())
    }

    #[test]
    fn resolves_scopes_calls_and_assignments() {
        let unit = analyze_source(
            "int add(int a, int b) { return a + b; }\n\
             int main(void) { int x = 1; { int x = 2; x = add(x, 3); } return x; }",
        )
        .unwrap();
        assert_eq!(unit.functions.len(), 2);
        assert!(
            unit.functions
                .iter()
                .all(|function| function.body.is_some())
        );
    }

    #[test]
    fn requires_a_prior_function_declaration() {
        let diagnostics =
            analyze_source("int main(void) { return later(); } int later(void) { return 0; }")
                .unwrap_err();
        assert_eq!(diagnostics[0].code, "CCC2008");
    }

    #[test]
    fn rejects_duplicate_locals() {
        let diagnostics = analyze_source("int main(void) { int x; int x; return 0; }").unwrap_err();
        assert_eq!(diagnostics[0].code, "CCC2010");
    }

    #[test]
    fn accepts_the_minimum_signed_int_constant() {
        let unit = analyze_source("int main(void) { return -2147483648; }").unwrap();
        assert!(unit.functions[0].body.is_some());
    }

    #[test]
    fn diagnoses_a_valueless_return_semantically() {
        let diagnostics = analyze_source("int main(void) { return; }").unwrap_err();
        assert_eq!(diagnostics[0].code, "CCC2011");
    }

    #[test]
    fn reports_redefinition_after_an_erroring_first_body() {
        let diagnostics =
            analyze_source("int f(void) { return missing; } int f(void) { return 1; }")
                .unwrap_err();
        assert_eq!(
            diagnostics
                .iter()
                .map(|diagnostic| diagnostic.code.as_str())
                .collect::<Vec<_>>(),
            ["CCC2005", "CCC2002"]
        );
    }
}
