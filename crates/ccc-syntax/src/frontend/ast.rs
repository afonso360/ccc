//! Syntax tree definitions shared by parsing, semantic analysis, and dumps.

use ccc_pp::{CharacterConstant, FloatingConstant, IntegerConstant, PragmaEvent, StringLiteral};
use ccc_session::Span;

use super::{names::ScopeEvent, token::Token};

#[derive(Clone, Debug, PartialEq)]
pub struct TranslationUnit {
    pub items: Vec<ExternalItem>,
    pub scope_events: Vec<ScopeEvent>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ExternalItem {
    Pragma(PragmaEvent),
    Declaration(Declaration),
    FunctionDefinition(Box<FunctionDefinition>),
    StaticAssert(Box<StaticAssert>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Declaration {
    pub specifiers: DeclarationSpecifiers,
    pub declarators: Vec<InitDeclarator>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FunctionDefinition {
    pub specifiers: DeclarationSpecifiers,
    pub declarator: Declarator,
    pub declarations: Vec<Declaration>,
    pub body: Statement,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DeclarationSpecifiers {
    pub items: Vec<DeclarationSpecifier>,
    pub extension: bool,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum DeclarationSpecifier {
    StorageClass(StorageClass),
    Type(TypeSpecifier),
    Qualifier(TypeQualifier),
    Function(FunctionSpecifier),
    Alignment(AlignmentSpecifier),
    Attribute(Attribute),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageClass {
    Typedef,
    Extern,
    Static,
    ThreadLocal,
    GnuThreadLocal,
    Auto,
    Register,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TypeQualifier {
    Const,
    Restrict,
    Volatile,
    Atomic,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FunctionSpecifier {
    Inline,
    NoReturn,
}

#[derive(Clone, Debug, PartialEq)]
pub enum AlignmentSpecifier {
    Type(Box<TypeName>),
    Expression(Box<Expression>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum TypeSpecifier {
    Void,
    Char,
    Short,
    Int,
    Long,
    Float,
    Double,
    Signed,
    Unsigned,
    Bool,
    Complex,
    Imaginary,
    Atomic(Box<TypeName>),
    Struct(Box<RecordSpecifier>),
    Union(Box<RecordSpecifier>),
    Enum(Box<EnumSpecifier>),
    TypedefName(Identifier),
    Typeof(TypeofSpecifier),
    /// A compiler-provided target type. This is intentionally separate from
    /// the arithmetic builtin type specifiers.
    BuiltinVaList,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TypeofSpecifier {
    Type(Box<TypeName>),
    Expression(Box<Expression>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Identifier {
    pub name: String,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RecordSpecifier {
    pub tag: Option<Identifier>,
    pub attributes: Vec<Attribute>,
    pub items: Option<Vec<RecordItem>>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RecordItem {
    Declaration(RecordDeclaration),
    StaticAssert(Box<StaticAssert>),
    Pragma(PragmaEvent),
}

#[derive(Clone, Debug, PartialEq)]
pub struct RecordDeclaration {
    pub specifiers: DeclarationSpecifiers,
    pub declarators: Vec<RecordDeclarator>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RecordDeclarator {
    pub declarator: Option<Declarator>,
    pub bit_width: Option<Expression>,
    pub attributes: Vec<Attribute>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EnumSpecifier {
    pub tag: Option<Identifier>,
    pub enumerators: Option<Vec<Enumerator>>,
    pub attributes: Vec<Attribute>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Enumerator {
    pub name: Identifier,
    pub value: Option<Expression>,
    pub attributes: Vec<Attribute>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct InitDeclarator {
    pub declarator: Declarator,
    pub asm_label: Option<AsmLabel>,
    pub attributes: Vec<Attribute>,
    pub initializer: Option<Initializer>,
    pub span: Span,
}

/// The recursive shape of a C declarator is retained instead of prematurely
/// folding it into a semantic type.
#[derive(Clone, Debug, PartialEq)]
pub struct Declarator {
    pub pointers: Vec<Pointer>,
    pub direct: DirectDeclarator,
    pub attributes: Vec<Attribute>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Pointer {
    pub qualifiers: Vec<TypeQualifier>,
    pub attributes: Vec<Attribute>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum DirectDeclarator {
    Identifier(Identifier),
    Abstract(Span),
    Parenthesized(Box<Declarator>, Span),
    Array {
        inner: Box<DirectDeclarator>,
        qualifiers: Vec<TypeQualifier>,
        is_static: bool,
        size: ArraySize,
        span: Span,
    },
    Function {
        inner: Box<DirectDeclarator>,
        parameters: Vec<ParameterDeclaration>,
        /// Distinguishes an empty prototype `f(void)` from an old-style
        /// unspecified parameter list `f()`.
        has_parameter_type_list: bool,
        variadic: bool,
        old_style_names: Vec<Identifier>,
        span: Span,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum ArraySize {
    Unspecified,
    Star,
    Expression(Box<Expression>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ParameterDeclaration {
    pub specifiers: DeclarationSpecifiers,
    pub declarator: Option<Declarator>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TypeName {
    pub specifiers: DeclarationSpecifiers,
    pub declarator: Option<Declarator>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Attribute {
    pub introducer: String,
    pub name: Identifier,
    pub arguments: Vec<Token>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AsmLabel {
    pub keyword_spelling: String,
    pub literal_spelling: String,
    pub literal: StringLiteral,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Initializer {
    Expression(Box<Expression>),
    List {
        entries: Vec<InitializerEntry>,
        span: Span,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct InitializerEntry {
    pub designation: Vec<Designator>,
    pub initializer: Initializer,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Designator {
    Index(Box<Expression>),
    Member(Identifier),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Expression {
    pub kind: ExpressionKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ExpressionKind {
    Identifier(Identifier),
    Integer(IntegerConstant),
    Floating(FloatingConstant),
    Character(CharacterConstant),
    String(StringLiteral),
    Parenthesized(Box<Expression>),
    GenericSelection {
        controlling: Box<Expression>,
        associations: Vec<GenericAssociation>,
    },
    Subscript {
        base: Box<Expression>,
        index: Box<Expression>,
    },
    Call {
        callee: Box<Expression>,
        arguments: Vec<Expression>,
    },
    Member {
        base: Box<Expression>,
        member: Identifier,
        indirect: bool,
    },
    PostfixIncrement(Box<Expression>),
    PostfixDecrement(Box<Expression>),
    CompoundLiteral {
        ty: TypeName,
        initializer: Box<Initializer>,
    },
    Unary {
        operator: UnaryOperator,
        operand: Box<Expression>,
    },
    SizeofExpression(Box<Expression>),
    SizeofType(TypeName),
    AlignofType(TypeName),
    Cast {
        ty: TypeName,
        expression: Box<Expression>,
    },
    Binary {
        operator: BinaryOperator,
        left: Box<Expression>,
        right: Box<Expression>,
    },
    Conditional {
        condition: Box<Expression>,
        then_expression: Box<Expression>,
        else_expression: Box<Expression>,
    },
    Assignment {
        operator: AssignmentOperator,
        target: Box<Expression>,
        value: Box<Expression>,
    },
    Comma(Vec<Expression>),
    Extension(Box<Expression>),
    BuiltinOffsetof {
        ty: Box<TypeName>,
        designator: Vec<OffsetDesignator>,
    },
    BuiltinVaStart {
        list: Box<Expression>,
        last_named_parameter: Box<Expression>,
    },
    BuiltinVaArg {
        list: Box<Expression>,
        ty: Box<TypeName>,
    },
    BuiltinVaCopy {
        destination: Box<Expression>,
        source: Box<Expression>,
    },
    BuiltinVaEnd {
        list: Box<Expression>,
    },
    BuiltinSyncSynchronize,
}

#[derive(Clone, Debug, PartialEq)]
pub enum OffsetDesignator {
    Member(Identifier),
    Index(Box<Expression>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct GenericAssociation {
    pub ty: Option<TypeName>,
    pub expression: Expression,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnaryOperator {
    PrefixIncrement,
    PrefixDecrement,
    Address,
    Dereference,
    Plus,
    Minus,
    BitwiseNot,
    LogicalNot,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BinaryOperator {
    Multiply,
    Divide,
    Remainder,
    Add,
    Subtract,
    LeftShift,
    RightShift,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    Equal,
    NotEqual,
    BitwiseAnd,
    BitwiseXor,
    BitwiseOr,
    LogicalAnd,
    LogicalOr,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AssignmentOperator {
    Assign,
    Multiply,
    Divide,
    Remainder,
    Add,
    Subtract,
    LeftShift,
    RightShift,
    BitwiseAnd,
    BitwiseXor,
    BitwiseOr,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Statement {
    pub kind: StatementKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum StatementKind {
    Label {
        label: Identifier,
        statement: Box<Statement>,
        attributes: Vec<Attribute>,
    },
    Case {
        value: Box<Expression>,
        statement: Box<Statement>,
    },
    Default(Box<Statement>),
    Compound(Vec<BlockItem>),
    Expression(Option<Box<Expression>>),
    If {
        condition: Box<Expression>,
        then_statement: Box<Statement>,
        else_statement: Option<Box<Statement>>,
    },
    Switch {
        expression: Box<Expression>,
        statement: Box<Statement>,
    },
    While {
        condition: Box<Expression>,
        statement: Box<Statement>,
    },
    DoWhile {
        statement: Box<Statement>,
        condition: Box<Expression>,
    },
    For {
        initializer: Box<ForInitializer>,
        condition: Option<Box<Expression>>,
        step: Option<Box<Expression>>,
        statement: Box<Statement>,
    },
    Goto(Identifier),
    Continue,
    Break,
    Return(Option<Box<Expression>>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum BlockItem {
    Declaration(Declaration),
    StaticAssert(Box<StaticAssert>),
    Statement(Box<Statement>),
    Pragma(PragmaEvent),
}

#[derive(Clone, Debug, PartialEq)]
pub enum ForInitializer {
    Empty,
    Expression(Box<Expression>),
    Declaration(Box<Declaration>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct StaticAssert {
    pub condition: Expression,
    pub message: Option<StringLiteral>,
    pub span: Span,
}

impl DeclarationSpecifiers {
    pub fn is_typedef(&self) -> bool {
        self.items.iter().any(|item| {
            matches!(
                item,
                DeclarationSpecifier::StorageClass(StorageClass::Typedef)
            )
        })
    }
}

impl Declarator {
    pub fn identifier(&self) -> Option<&Identifier> {
        self.direct.identifier()
    }

    pub fn is_function(&self) -> bool {
        matches!(self.direct, DirectDeclarator::Function { .. })
    }

    pub fn has_old_style_names(&self) -> bool {
        matches!(
            &self.direct,
            DirectDeclarator::Function {
                old_style_names,
                ..
            } if !old_style_names.is_empty()
        )
    }

    pub fn parameters(&self) -> &[ParameterDeclaration] {
        match &self.direct {
            DirectDeclarator::Function { parameters, .. } => parameters,
            _ => &[],
        }
    }

    pub fn old_style_names(&self) -> &[Identifier] {
        match &self.direct {
            DirectDeclarator::Function {
                old_style_names, ..
            } => old_style_names,
            _ => &[],
        }
    }
}

impl DirectDeclarator {
    pub fn identifier(&self) -> Option<&Identifier> {
        match self {
            Self::Identifier(identifier) => Some(identifier),
            Self::Parenthesized(declarator, _) => declarator.identifier(),
            Self::Array { inner, .. } | Self::Function { inner, .. } => inner.identifier(),
            Self::Abstract(_) => None,
        }
    }

    pub fn span(&self) -> Span {
        match self {
            Self::Identifier(identifier) => identifier.span,
            Self::Abstract(span)
            | Self::Parenthesized(_, span)
            | Self::Array { span, .. }
            | Self::Function { span, .. } => *span,
        }
    }
}

impl Initializer {
    pub fn span(&self) -> Span {
        match self {
            Self::Expression(expression) => expression.span,
            Self::List { span, .. } => *span,
        }
    }
}
