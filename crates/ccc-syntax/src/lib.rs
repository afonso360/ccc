//! Translation-phase token conversion and the recursive-descent C parser.

use std::fmt;

use ccc_pp::{PpToken, PpTokenKind, decode_integer_constant};
use ccc_session::Span;

const MAX_EXPRESSION_DEPTH: usize = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Keyword {
    Auto,
    Break,
    Case,
    Char,
    Const,
    Continue,
    Default,
    Do,
    Double,
    Else,
    Enum,
    Extern,
    Float,
    For,
    Goto,
    If,
    Int,
    Inline,
    Long,
    Register,
    Restrict,
    Return,
    Short,
    Signed,
    Sizeof,
    Static,
    Struct,
    Switch,
    Typedef,
    Union,
    Unsigned,
    Void,
    Volatile,
    While,
    Alignas,
    Alignof,
    Atomic,
    Bool,
    Complex,
    Generic,
    Imaginary,
    Noreturn,
    StaticAssert,
    ThreadLocal,
}

impl Keyword {
    fn from_spelling(spelling: &str) -> Option<Self> {
        match spelling {
            "auto" => Some(Self::Auto),
            "break" => Some(Self::Break),
            "case" => Some(Self::Case),
            "char" => Some(Self::Char),
            "const" => Some(Self::Const),
            "continue" => Some(Self::Continue),
            "default" => Some(Self::Default),
            "do" => Some(Self::Do),
            "double" => Some(Self::Double),
            "else" => Some(Self::Else),
            "enum" => Some(Self::Enum),
            "extern" => Some(Self::Extern),
            "float" => Some(Self::Float),
            "for" => Some(Self::For),
            "goto" => Some(Self::Goto),
            "if" => Some(Self::If),
            "int" => Some(Self::Int),
            "inline" => Some(Self::Inline),
            "long" => Some(Self::Long),
            "register" => Some(Self::Register),
            "restrict" => Some(Self::Restrict),
            "return" => Some(Self::Return),
            "short" => Some(Self::Short),
            "signed" => Some(Self::Signed),
            "sizeof" => Some(Self::Sizeof),
            "static" => Some(Self::Static),
            "struct" => Some(Self::Struct),
            "switch" => Some(Self::Switch),
            "typedef" => Some(Self::Typedef),
            "union" => Some(Self::Union),
            "unsigned" => Some(Self::Unsigned),
            "void" => Some(Self::Void),
            "volatile" => Some(Self::Volatile),
            "while" => Some(Self::While),
            "_Alignas" => Some(Self::Alignas),
            "_Alignof" => Some(Self::Alignof),
            "_Atomic" => Some(Self::Atomic),
            "_Bool" => Some(Self::Bool),
            "_Complex" => Some(Self::Complex),
            "_Generic" => Some(Self::Generic),
            "_Imaginary" => Some(Self::Imaginary),
            "_Noreturn" => Some(Self::Noreturn),
            "_Static_assert" => Some(Self::StaticAssert),
            "_Thread_local" => Some(Self::ThreadLocal),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Punctuator {
    LeftParen,
    RightParen,
    LeftBrace,
    RightBrace,
    Comma,
    Semicolon,
    Assign,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Bang,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    EqualEqual,
    BangEqual,
    AmpAmp,
    PipePipe,
    Unsupported,
}

impl Punctuator {
    fn from_spelling(spelling: &str) -> Self {
        match spelling {
            "(" => Self::LeftParen,
            ")" => Self::RightParen,
            "{" => Self::LeftBrace,
            "}" => Self::RightBrace,
            "," => Self::Comma,
            ";" => Self::Semicolon,
            "=" => Self::Assign,
            "+" => Self::Plus,
            "-" => Self::Minus,
            "*" => Self::Star,
            "/" => Self::Slash,
            "%" => Self::Percent,
            "!" => Self::Bang,
            "<" => Self::Less,
            "<=" => Self::LessEqual,
            ">" => Self::Greater,
            ">=" => Self::GreaterEqual,
            "==" => Self::EqualEqual,
            "!=" => Self::BangEqual,
            "&&" => Self::AmpAmp,
            "||" => Self::PipePipe,
            _ => Self::Unsupported,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TokenKind {
    Keyword(Keyword),
    Identifier,
    IntegerConstant,
    StringLiteral,
    CharacterConstant,
    Punctuator(Punctuator),
}

impl TokenKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Keyword(_) => "keyword",
            Self::Identifier => "identifier",
            Self::IntegerConstant => "integer-constant",
            Self::StringLiteral => "string-literal",
            Self::CharacterConstant => "character-constant",
            Self::Punctuator(_) => "punctuator",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub spelling: String,
    pub span: Span,
}

pub fn convert_pp_tokens(tokens: Vec<PpToken>) -> Vec<Token> {
    tokens
        .into_iter()
        .map(|token| Token {
            kind: token_kind(&token),
            spelling: token.spelling,
            span: token.span,
        })
        .collect()
}

fn token_kind(token: &PpToken) -> TokenKind {
    match token.kind {
        PpTokenKind::Identifier => Keyword::from_spelling(&token.spelling)
            .map_or(TokenKind::Identifier, TokenKind::Keyword),
        PpTokenKind::PpNumber => TokenKind::IntegerConstant,
        PpTokenKind::StringLiteral => TokenKind::StringLiteral,
        PpTokenKind::CharacterConstant => TokenKind::CharacterConstant,
        PpTokenKind::Punctuator => {
            TokenKind::Punctuator(Punctuator::from_spelling(&token.spelling))
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TypeNameKind {
    Int,
    Void,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypeName {
    pub kind: TypeNameKind,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TranslationUnit {
    pub declarations: Vec<FunctionDeclaration>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FunctionDeclaration {
    pub result: TypeName,
    pub name: String,
    pub name_span: Span,
    pub parameters: Vec<Parameter>,
    pub body: Option<Statement>,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Parameter {
    pub name: Option<String>,
    pub name_span: Option<Span>,
    pub ty: TypeName,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BlockItem {
    Declaration(LocalDeclaration),
    Statement(Statement),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalDeclaration {
    pub name: String,
    pub name_span: Span,
    pub initializer: Option<Expression>,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Statement {
    pub kind: StatementKind,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StatementKind {
    Compound(Vec<BlockItem>),
    Expression(Option<Expression>),
    If {
        condition: Expression,
        then_statement: Box<Statement>,
        else_statement: Option<Box<Statement>>,
    },
    While {
        condition: Expression,
        body: Box<Statement>,
    },
    Return(Option<Expression>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Expression {
    pub kind: ExpressionKind,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExpressionKind {
    Integer {
        value: u64,
        spelling: String,
    },
    Name(String),
    Unary {
        operator: UnaryOperator,
        operand: Box<Expression>,
    },
    Binary {
        operator: BinaryOperator,
        left: Box<Expression>,
        right: Box<Expression>,
    },
    Assign {
        target: Box<Expression>,
        value: Box<Expression>,
    },
    Call {
        callee: Box<Expression>,
        arguments: Vec<Expression>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnaryOperator {
    Plus,
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
    LogicalAnd,
    LogicalOr,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParseError {
    pub code: &'static str,
    pub span: Span,
    pub message: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.message.fmt(formatter)
    }
}

impl std::error::Error for ParseError {}

pub fn parse(tokens: &[Token]) -> Result<TranslationUnit, ParseError> {
    Parser {
        tokens,
        position: 0,
        expression_depth: 0,
    }
    .translation_unit()
}

struct Parser<'a> {
    tokens: &'a [Token],
    position: usize,
    expression_depth: usize,
}

impl Parser<'_> {
    fn translation_unit(mut self) -> Result<TranslationUnit, ParseError> {
        let mut declarations = Vec::new();
        while self.current().is_some() {
            declarations.push(self.function_declaration()?);
        }
        Ok(TranslationUnit { declarations })
    }

    fn function_declaration(&mut self) -> Result<FunctionDeclaration, ParseError> {
        let result = self.type_name()?;
        if result.kind != TypeNameKind::Int {
            return Err(self.error_at(result.span, "only functions returning `int` are supported"));
        }
        let (name, name_span) = self.identifier()?;
        self.expect_punctuator(Punctuator::LeftParen, "expected `(` after function name")?;
        let parameters = self.parameter_list()?;
        self.expect_punctuator(Punctuator::RightParen, "expected `)` after parameter list")?;

        let (body, end) = if self.check_punctuator(Punctuator::LeftBrace) {
            let body = self.compound_statement()?;
            let end = body.span.end;
            (Some(body), end)
        } else {
            let semicolon = self.expect_punctuator(
                Punctuator::Semicolon,
                "expected a function body or `;` after declaration",
            )?;
            (None, semicolon.span.end)
        };

        Ok(FunctionDeclaration {
            span: Span::with_origin(result.span.file, result.span.start, end, result.span.origin),
            result,
            name,
            name_span,
            parameters,
            body,
        })
    }

    fn parameter_list(&mut self) -> Result<Vec<Parameter>, ParseError> {
        if self.check_punctuator(Punctuator::RightParen) {
            return Err(self.error_current(
                "empty parameter lists are not supported; use `(void)` for no parameters",
            ));
        }
        if self.check_keyword(Keyword::Void)
            && self
                .tokens
                .get(self.position + 1)
                .is_some_and(|token| token.kind == TokenKind::Punctuator(Punctuator::RightParen))
        {
            self.position += 1;
            return Ok(Vec::new());
        }

        let mut parameters = Vec::new();
        loop {
            let ty = self.type_name()?;
            if ty.kind != TypeNameKind::Int {
                return Err(self.error_at(ty.span, "only `int` parameters are supported"));
            }
            let (name, name_span) = if self
                .current()
                .is_some_and(|token| token.kind == TokenKind::Identifier)
            {
                let (name, span) = self.identifier()?;
                (Some(name), Some(span))
            } else {
                (None, None)
            };
            let end = name_span.map_or(ty.span.end, |span| span.end);
            parameters.push(Parameter {
                span: Span::with_origin(ty.span.file, ty.span.start, end, ty.span.origin),
                name,
                name_span,
                ty,
            });
            if !self.consume_punctuator(Punctuator::Comma) {
                break;
            }
        }
        Ok(parameters)
    }

    fn type_name(&mut self) -> Result<TypeName, ParseError> {
        let token = self
            .current()
            .ok_or_else(|| self.error_eof("expected a type name"))?;
        let kind = match token.kind {
            TokenKind::Keyword(Keyword::Int) => TypeNameKind::Int,
            TokenKind::Keyword(Keyword::Void) => TypeNameKind::Void,
            _ => return Err(self.error_at(token.span, "expected `int` or `void`")),
        };
        let span = token.span;
        self.position += 1;
        Ok(TypeName { kind, span })
    }

    fn compound_statement(&mut self) -> Result<Statement, ParseError> {
        let start = self
            .expect_punctuator(Punctuator::LeftBrace, "expected `{`")?
            .span;
        let mut items = Vec::new();
        while !self.check_punctuator(Punctuator::RightBrace) {
            if self.current().is_none() {
                return Err(self.error_eof("expected `}` to close compound statement"));
            }
            if self.check_keyword(Keyword::Int) {
                items.push(BlockItem::Declaration(self.local_declaration()?));
            } else {
                items.push(BlockItem::Statement(self.statement()?));
            }
        }
        let end = self
            .expect_punctuator(Punctuator::RightBrace, "expected `}`")?
            .span;
        Ok(Statement {
            span: Span::with_origin(start.file, start.start, end.end, start.origin),
            kind: StatementKind::Compound(items),
        })
    }

    fn local_declaration(&mut self) -> Result<LocalDeclaration, ParseError> {
        let ty = self.type_name()?;
        debug_assert_eq!(ty.kind, TypeNameKind::Int);
        let (name, name_span) = self.identifier()?;
        let initializer = if self.consume_punctuator(Punctuator::Assign) {
            Some(self.expression()?)
        } else {
            None
        };
        let end = self
            .expect_punctuator(Punctuator::Semicolon, "expected `;` after declaration")?
            .span;
        Ok(LocalDeclaration {
            span: Span::with_origin(ty.span.file, ty.span.start, end.end, ty.span.origin),
            name,
            name_span,
            initializer,
        })
    }

    fn statement(&mut self) -> Result<Statement, ParseError> {
        if self.check_punctuator(Punctuator::LeftBrace) {
            return self.compound_statement();
        }
        if let Some(keyword) = self.consume_keyword(Keyword::If) {
            self.expect_punctuator(Punctuator::LeftParen, "expected `(` after `if`")?;
            let condition = self.expression()?;
            self.expect_punctuator(Punctuator::RightParen, "expected `)` after condition")?;
            let then_statement = Box::new(self.statement()?);
            let else_statement = if self.consume_keyword(Keyword::Else).is_some() {
                Some(Box::new(self.statement()?))
            } else {
                None
            };
            let end = else_statement
                .as_ref()
                .map_or(then_statement.span.end, |statement| statement.span.end);
            return Ok(Statement {
                span: Span::with_origin(
                    keyword.span.file,
                    keyword.span.start,
                    end,
                    keyword.span.origin,
                ),
                kind: StatementKind::If {
                    condition,
                    then_statement,
                    else_statement,
                },
            });
        }
        if let Some(keyword) = self.consume_keyword(Keyword::While) {
            self.expect_punctuator(Punctuator::LeftParen, "expected `(` after `while`")?;
            let condition = self.expression()?;
            self.expect_punctuator(Punctuator::RightParen, "expected `)` after condition")?;
            let body = Box::new(self.statement()?);
            return Ok(Statement {
                span: Span::with_origin(
                    keyword.span.file,
                    keyword.span.start,
                    body.span.end,
                    keyword.span.origin,
                ),
                kind: StatementKind::While { condition, body },
            });
        }
        if let Some(keyword) = self.consume_keyword(Keyword::Return) {
            let value = if self.check_punctuator(Punctuator::Semicolon) {
                None
            } else {
                Some(self.expression()?)
            };
            let end = self
                .expect_punctuator(Punctuator::Semicolon, "expected `;` after return value")?
                .span;
            return Ok(Statement {
                span: Span::with_origin(
                    keyword.span.file,
                    keyword.span.start,
                    end.end,
                    keyword.span.origin,
                ),
                kind: StatementKind::Return(value),
            });
        }
        if let Some(semicolon) = self.consume_token(Punctuator::Semicolon) {
            return Ok(Statement {
                span: semicolon.span,
                kind: StatementKind::Expression(None),
            });
        }
        let expression = self.expression()?;
        let start = expression.span;
        let end = self
            .expect_punctuator(Punctuator::Semicolon, "expected `;` after expression")?
            .span;
        Ok(Statement {
            span: Span::with_origin(start.file, start.start, end.end, start.origin),
            kind: StatementKind::Expression(Some(expression)),
        })
    }

    fn expression(&mut self) -> Result<Expression, ParseError> {
        self.nested_expression(Self::assignment_expression)
    }

    fn assignment_expression(&mut self) -> Result<Expression, ParseError> {
        let target = self.logical_or_expression()?;
        if self.consume_punctuator(Punctuator::Assign) {
            let value = self.nested_expression(Self::assignment_expression)?;
            let span = joined_span(target.span, value.span);
            Ok(Expression {
                span,
                kind: ExpressionKind::Assign {
                    target: Box::new(target),
                    value: Box::new(value),
                },
            })
        } else {
            Ok(target)
        }
    }

    fn logical_or_expression(&mut self) -> Result<Expression, ParseError> {
        self.left_associative(
            Self::logical_and_expression,
            &[(Punctuator::PipePipe, BinaryOperator::LogicalOr)],
        )
    }

    fn logical_and_expression(&mut self) -> Result<Expression, ParseError> {
        self.left_associative(
            Self::equality_expression,
            &[(Punctuator::AmpAmp, BinaryOperator::LogicalAnd)],
        )
    }

    fn equality_expression(&mut self) -> Result<Expression, ParseError> {
        self.left_associative(
            Self::relational_expression,
            &[
                (Punctuator::EqualEqual, BinaryOperator::Equal),
                (Punctuator::BangEqual, BinaryOperator::NotEqual),
            ],
        )
    }

    fn relational_expression(&mut self) -> Result<Expression, ParseError> {
        self.left_associative(
            Self::additive_expression,
            &[
                (Punctuator::Less, BinaryOperator::Less),
                (Punctuator::LessEqual, BinaryOperator::LessEqual),
                (Punctuator::Greater, BinaryOperator::Greater),
                (Punctuator::GreaterEqual, BinaryOperator::GreaterEqual),
            ],
        )
    }

    fn additive_expression(&mut self) -> Result<Expression, ParseError> {
        self.left_associative(
            Self::multiplicative_expression,
            &[
                (Punctuator::Plus, BinaryOperator::Add),
                (Punctuator::Minus, BinaryOperator::Subtract),
            ],
        )
    }

    fn multiplicative_expression(&mut self) -> Result<Expression, ParseError> {
        self.left_associative(
            Self::unary_expression,
            &[
                (Punctuator::Star, BinaryOperator::Multiply),
                (Punctuator::Slash, BinaryOperator::Divide),
                (Punctuator::Percent, BinaryOperator::Remainder),
            ],
        )
    }

    fn left_associative(
        &mut self,
        operand: fn(&mut Self) -> Result<Expression, ParseError>,
        operators: &[(Punctuator, BinaryOperator)],
    ) -> Result<Expression, ParseError> {
        let mut expression = operand(self)?;
        while let Some((_, operator)) = operators
            .iter()
            .find(|(punctuator, _)| self.check_punctuator(*punctuator))
        {
            self.position += 1;
            let right = operand(self)?;
            let span = joined_span(expression.span, right.span);
            expression = Expression {
                span,
                kind: ExpressionKind::Binary {
                    operator: *operator,
                    left: Box::new(expression),
                    right: Box::new(right),
                },
            };
        }
        Ok(expression)
    }

    fn unary_expression(&mut self) -> Result<Expression, ParseError> {
        let operator = if self.consume_punctuator(Punctuator::Plus) {
            Some(UnaryOperator::Plus)
        } else if self.consume_punctuator(Punctuator::Minus) {
            Some(UnaryOperator::Negate)
        } else if self.consume_punctuator(Punctuator::Bang) {
            Some(UnaryOperator::LogicalNot)
        } else {
            None
        };
        if let Some(operator) = operator {
            let operator_span = self.tokens[self.position - 1].span;
            let operand = self.nested_expression(Self::unary_expression)?;
            return Ok(Expression {
                span: Span::with_origin(
                    operator_span.file,
                    operator_span.start,
                    operand.span.end,
                    operator_span.origin,
                ),
                kind: ExpressionKind::Unary {
                    operator,
                    operand: Box::new(operand),
                },
            });
        }
        self.postfix_expression()
    }

    fn postfix_expression(&mut self) -> Result<Expression, ParseError> {
        let mut expression = self.primary_expression()?;
        while self.consume_punctuator(Punctuator::LeftParen) {
            let mut arguments = Vec::new();
            if !self.check_punctuator(Punctuator::RightParen) {
                loop {
                    arguments.push(self.nested_expression(Self::assignment_expression)?);
                    if !self.consume_punctuator(Punctuator::Comma) {
                        break;
                    }
                }
            }
            let end = self
                .expect_punctuator(Punctuator::RightParen, "expected `)` after arguments")?
                .span;
            let start = expression.span;
            expression = Expression {
                span: Span::with_origin(start.file, start.start, end.end, start.origin),
                kind: ExpressionKind::Call {
                    callee: Box::new(expression),
                    arguments,
                },
            };
        }
        Ok(expression)
    }

    fn primary_expression(&mut self) -> Result<Expression, ParseError> {
        let token = self
            .current()
            .ok_or_else(|| self.error_eof("expected an expression"))?
            .clone();
        match token.kind {
            TokenKind::IntegerConstant => {
                let value = decode_integer(&token.spelling).map_err(|message| ParseError {
                    code: "CCC1003",
                    span: token.span,
                    message,
                })?;
                self.position += 1;
                Ok(Expression {
                    span: token.span,
                    kind: ExpressionKind::Integer {
                        value,
                        spelling: token.spelling,
                    },
                })
            }
            TokenKind::Identifier => {
                self.position += 1;
                Ok(Expression {
                    span: token.span,
                    kind: ExpressionKind::Name(token.spelling),
                })
            }
            TokenKind::Punctuator(Punctuator::LeftParen) => {
                self.position += 1;
                let mut expression = self.expression()?;
                let end = self
                    .expect_punctuator(Punctuator::RightParen, "expected `)`")?
                    .span;
                expression.span = Span::with_origin(
                    token.span.file,
                    token.span.start,
                    end.end,
                    token.span.origin,
                );
                Ok(expression)
            }
            _ => Err(self.error_at(
                token.span,
                "expected an integer, name, or parenthesized expression",
            )),
        }
    }

    fn identifier(&mut self) -> Result<(String, Span), ParseError> {
        let token = self
            .current()
            .ok_or_else(|| self.error_eof("expected an identifier"))?;
        if token.kind != TokenKind::Identifier {
            return Err(self.error_at(token.span, "expected an identifier"));
        }
        let result = (token.spelling.clone(), token.span);
        self.position += 1;
        Ok(result)
    }

    fn expect_punctuator(
        &mut self,
        punctuator: Punctuator,
        message: &str,
    ) -> Result<Token, ParseError> {
        if let Some(token) = self.consume_token(punctuator) {
            Ok(token)
        } else {
            Err(self.error_current(message))
        }
    }

    fn consume_token(&mut self, punctuator: Punctuator) -> Option<Token> {
        if !self.check_punctuator(punctuator) {
            return None;
        }
        let token = self.tokens[self.position].clone();
        self.position += 1;
        Some(token)
    }

    fn consume_punctuator(&mut self, punctuator: Punctuator) -> bool {
        self.consume_token(punctuator).is_some()
    }

    fn check_punctuator(&self, punctuator: Punctuator) -> bool {
        self.current()
            .is_some_and(|token| token.kind == TokenKind::Punctuator(punctuator))
    }

    fn consume_keyword(&mut self, keyword: Keyword) -> Option<Token> {
        if !self.check_keyword(keyword) {
            return None;
        }
        let token = self.tokens[self.position].clone();
        self.position += 1;
        Some(token)
    }

    fn check_keyword(&self, keyword: Keyword) -> bool {
        self.current()
            .is_some_and(|token| token.kind == TokenKind::Keyword(keyword))
    }

    fn current(&self) -> Option<&Token> {
        self.tokens.get(self.position)
    }

    fn nested_expression(
        &mut self,
        parser: fn(&mut Self) -> Result<Expression, ParseError>,
    ) -> Result<Expression, ParseError> {
        if self.expression_depth >= MAX_EXPRESSION_DEPTH {
            return Err(self.error_with_code(
                "CCC1004",
                self.current().map_or_else(
                    || {
                        let last = self.tokens.last().expect("an expression has a token");
                        Span::with_origin(
                            last.span.file,
                            last.span.end,
                            last.span.end,
                            last.span.origin,
                        )
                    },
                    |token| token.span,
                ),
                "expression is too deeply nested",
            ));
        }
        self.expression_depth += 1;
        let result = parser(self);
        self.expression_depth -= 1;
        result
    }

    fn error_current(&self, message: &str) -> ParseError {
        self.current().map_or_else(
            || self.error_eof(message),
            |token| self.error_at(token.span, message),
        )
    }

    fn error_eof(&self, message: &str) -> ParseError {
        let span = self
            .tokens
            .last()
            .map(|token| {
                Span::with_origin(
                    token.span.file,
                    token.span.end,
                    token.span.end,
                    token.span.origin,
                )
            })
            .expect("the parser only asks for an EOF span after seeing a token");
        ParseError {
            code: "CCC1001",
            span,
            message: message.to_owned(),
        }
    }

    fn error_at(&self, span: Span, message: &str) -> ParseError {
        self.error_with_code("CCC1001", span, message)
    }

    fn error_with_code(&self, code: &'static str, span: Span, message: &str) -> ParseError {
        ParseError {
            code,
            span,
            message: message.to_owned(),
        }
    }
}

fn decode_integer(spelling: &str) -> Result<u64, String> {
    let decoded = decode_integer_constant(spelling).map_err(|error| error.message)?;
    if decoded.suffix.unsigned || decoded.suffix.long_count != 0 {
        return Err(format!(
            "integer suffix in `{spelling}` requires unsupported integer-type semantics"
        ));
    }
    decoded
        .value
        .try_into()
        .map_err(|_| format!("integer constant `{spelling}` is too large"))
}

fn joined_span(left: Span, right: Span) -> Span {
    debug_assert_eq!(left.file, right.file);
    Span::with_origin(left.file, left.start, right.end, left.origin)
}

pub fn dump_ast(unit: &TranslationUnit) -> String {
    let mut output = String::new();
    output.push_str("translation-unit\n");
    for declaration in &unit.declarations {
        dump_function(&mut output, declaration, 1);
    }
    output
}

fn dump_function(output: &mut String, declaration: &FunctionDeclaration, indent: usize) {
    line(
        output,
        indent,
        &format!(
            "function {} -> int {}",
            declaration.name,
            if declaration.body.is_some() {
                "definition"
            } else {
                "declaration"
            }
        ),
    );
    for parameter in &declaration.parameters {
        line(
            output,
            indent + 1,
            &format!("parameter int {}", parameter.name.as_deref().unwrap_or("_")),
        );
    }
    if let Some(body) = &declaration.body {
        dump_statement(output, body, indent + 1);
    }
}

fn dump_statement(output: &mut String, statement: &Statement, indent: usize) {
    match &statement.kind {
        StatementKind::Compound(items) => {
            line(output, indent, "compound");
            for item in items {
                match item {
                    BlockItem::Declaration(declaration) => {
                        line(
                            output,
                            indent + 1,
                            &format!("local int {}", declaration.name),
                        );
                        if let Some(initializer) = &declaration.initializer {
                            dump_expression(output, initializer, indent + 2);
                        }
                    }
                    BlockItem::Statement(statement) => {
                        dump_statement(output, statement, indent + 1);
                    }
                }
            }
        }
        StatementKind::Expression(expression) => {
            line(output, indent, "expression-statement");
            if let Some(expression) = expression {
                dump_expression(output, expression, indent + 1);
            }
        }
        StatementKind::If {
            condition,
            then_statement,
            else_statement,
        } => {
            line(output, indent, "if");
            dump_expression(output, condition, indent + 1);
            dump_statement(output, then_statement, indent + 1);
            if let Some(else_statement) = else_statement {
                dump_statement(output, else_statement, indent + 1);
            }
        }
        StatementKind::While { condition, body } => {
            line(output, indent, "while");
            dump_expression(output, condition, indent + 1);
            dump_statement(output, body, indent + 1);
        }
        StatementKind::Return(expression) => {
            line(output, indent, "return");
            if let Some(expression) = expression {
                dump_expression(output, expression, indent + 1);
            }
        }
    }
}

fn dump_expression(output: &mut String, expression: &Expression, indent: usize) {
    match &expression.kind {
        ExpressionKind::Integer { spelling, .. } => {
            line(output, indent, &format!("integer {spelling}"))
        }
        ExpressionKind::Name(name) => line(output, indent, &format!("name {name}")),
        ExpressionKind::Unary { operator, operand } => {
            line(output, indent, &format!("unary {operator:?}"));
            dump_expression(output, operand, indent + 1);
        }
        ExpressionKind::Binary {
            operator,
            left,
            right,
        } => {
            line(output, indent, &format!("binary {operator:?}"));
            dump_expression(output, left, indent + 1);
            dump_expression(output, right, indent + 1);
        }
        ExpressionKind::Assign { target, value } => {
            line(output, indent, "assign");
            dump_expression(output, target, indent + 1);
            dump_expression(output, value, indent + 1);
        }
        ExpressionKind::Call { callee, arguments } => {
            line(output, indent, "call");
            dump_expression(output, callee, indent + 1);
            for argument in arguments {
                dump_expression(output, argument, indent + 1);
            }
        }
    }
}

fn line(output: &mut String, indent: usize, text: &str) {
    output.push_str(&"  ".repeat(indent));
    output.push_str(text);
    output.push('\n');
}

#[cfg(test)]
mod tests {
    use ccc_pp::lex;
    use ccc_session::SourceMap;

    use super::*;

    fn parse_source(source: &str) -> Result<TranslationUnit, ParseError> {
        let mut sources = SourceMap::new();
        let file = sources.add_file("test.c", source);
        let tokens = convert_pp_tokens(lex(file, sources.source(file).unwrap()).unwrap());
        parse(&tokens)
    }

    #[test]
    fn distinguishes_keywords_from_identifiers() {
        let mut sources = SourceMap::new();
        let file = sources.add_file("test.c", "int integer;");
        let tokens = convert_pp_tokens(lex(file, sources.source(file).unwrap()).unwrap());
        assert_eq!(tokens[0].kind, TokenKind::Keyword(Keyword::Int));
        assert_eq!(tokens[1].kind, TokenKind::Identifier);
    }

    #[test]
    fn parses_precedence_control_flow_and_calls() {
        let unit = parse_source(
            "int add(int a, int b) { return a + b * 2; }\n\
             int main(void) { int x = 0; while (x < 3) x = x + 1;\
             if (x == 3) return add(x, 4); else return 0; }",
        )
        .unwrap();
        assert_eq!(unit.declarations.len(), 2);
        let dump = dump_ast(&unit);
        assert!(dump.contains("binary Multiply"));
        assert!(dump.contains("while"));
        assert!(dump.contains("call"));
    }

    #[test]
    fn rejects_old_style_empty_parameter_lists() {
        assert!(
            parse_source("int main() { return 0; }")
                .unwrap_err()
                .message
                .contains("use `(void)`")
        );
    }

    #[test]
    fn rejects_excessively_nested_expressions_without_overflowing_the_stack() {
        let depth = 100_000;
        let source = format!(
            "int main(void) {{ return {}0{}; }}",
            "(".repeat(depth),
            ")".repeat(depth)
        );
        let error = parse_source(&source).unwrap_err();
        assert_eq!(error.code, "CCC1004");
        assert!(error.message.contains("too deeply nested"));
    }

    #[test]
    fn accepts_a_return_statement_without_an_expression_syntactically() {
        let unit = parse_source("int main(void) { return; }").unwrap();
        let body = unit.declarations[0].body.as_ref().unwrap();
        let StatementKind::Compound(items) = &body.kind else {
            panic!("function body should be a compound statement");
        };
        let BlockItem::Statement(statement) = &items[0] else {
            panic!("body item should be a statement");
        };
        assert!(matches!(statement.kind, StatementKind::Return(None)));
    }

    #[test]
    fn classifies_integer_suffixes_as_unsupported_type_semantics() {
        let error = parse_source("int main(void) { return 1U; }").unwrap_err();
        assert_eq!(error.code, "CCC1003");
        assert!(error.message.contains("integer-type semantics"));
    }
}
