use ccc_pp::{PragmaEvent, StringLiteralPrefix};
use ccc_session::Span;
use ccc_syntax::frontend::{AssignmentOperator, BinaryOperator, UnaryOperator};
use ccc_target::{CapabilityState, PackingPolicy};
use ccc_types::{QualifiedType, TypeId, TypeStore, VariableLengthId};

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct GlobalId(pub u32);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct FullFunctionId(pub u32);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct FullLocalId(pub u32);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct LabelId(pub u32);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct StringId(pub u32);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TypedefId(pub u32);

#[derive(Clone, Debug)]
pub struct FullTypedTranslationUnit {
    pub types: TypeStore,
    /// Source-ordered external items. Definitions referenced here live in the
    /// corresponding stable-ID arena below.
    pub external_items: Vec<FullTypedExternalItem>,
    pub globals: Vec<FullTypedGlobal>,
    pub functions: Vec<FullTypedFunction>,
    pub typedefs: Vec<FullTypedTypedef>,
    pub strings: Vec<FullTypedString>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FullTypedExternalItem {
    Global(GlobalId),
    Function(FullFunctionId),
    Typedef(TypedefId),
    TypeDeclaration { ty: TypeId, span: Span },
    StaticAssert { value: i128, span: Span },
    Pragma(PragmaEvent),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SemanticStorageClass {
    Automatic,
    Register,
    Static,
    Extern,
    ThreadLocal,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Linkage {
    None,
    Internal,
    External,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SymbolBinding {
    #[default]
    Strong,
    Weak,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageDuration {
    Automatic,
    Static,
    Thread,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FunctionProperties {
    pub inline: bool,
    pub no_return: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FullTypedAttribute {
    pub introducer: String,
    pub name: String,
    pub arguments: Vec<String>,
    pub capability: CapabilityState,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FullTypedAsmLabel {
    pub keyword_spelling: String,
    pub literal_spelling: String,
    pub symbol: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FullTypedGlobal {
    pub id: GlobalId,
    pub name: String,
    pub ty: QualifiedType,
    pub storage: SemanticStorageClass,
    pub linkage: Linkage,
    pub duration: StorageDuration,
    pub initializer: Option<FullTypedInitializer>,
    pub tentative: bool,
    pub asm_label: Option<FullTypedAsmLabel>,
    pub attributes: Vec<FullTypedAttribute>,
    pub emission: GlobalEmission,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GlobalEmission {
    pub symbol_name: String,
    pub binding: SymbolBinding,
    pub visibility: SymbolVisibility,
    pub section: Option<String>,
    pub requested_alignment: Option<u64>,
    pub tls: Option<TlsModel>,
    pub definition: ObjectDefinitionPolicy,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SymbolVisibility {
    #[default]
    Default,
    Hidden,
    Protected,
    Internal,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TlsModel {
    GeneralDynamic,
    LocalDynamic,
    InitialExec,
    LocalExec,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObjectDefinitionPolicy {
    Declaration,
    TentativeCommon,
    Definition,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FullTypedFunction {
    pub id: FullFunctionId,
    pub name: String,
    pub signature: TypeId,
    pub storage: SemanticStorageClass,
    pub linkage: Linkage,
    pub binding: SymbolBinding,
    pub visibility: SymbolVisibility,
    pub properties: FunctionProperties,
    pub parameters: Vec<FullTypedParameter>,
    pub body: Option<FullTypedStatement>,
    pub asm_label: Option<FullTypedAsmLabel>,
    pub attributes: Vec<FullTypedAttribute>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FullTypedParameter {
    pub local: FullLocalId,
    pub name: String,
    pub ty: QualifiedType,
    /// Runtime bounds evaluated for this parameter declaration. Each entry is
    /// keyed by the ID assigned before array-parameter adjustment; bounds for
    /// nested arrays remain embedded in the adjusted pointer type.
    pub variable_length_bounds: Vec<FullTypedVariableLengthBound>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FullTypedVariableLengthBound {
    pub id: VariableLengthId,
    pub expression: FullTypedExpression,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FullTypedTypedef {
    pub id: TypedefId,
    pub name: String,
    pub ty: QualifiedType,
    pub attributes: Vec<FullTypedAttribute>,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FullTypedString {
    pub id: StringId,
    pub prefix: StringLiteralPrefix,
    /// Includes the implicit trailing zero code unit.
    pub code_units: Vec<u32>,
    pub ty: QualifiedType,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FullTypedLocalDeclaration {
    pub local: FullLocalId,
    pub name: String,
    pub ty: QualifiedType,
    pub storage: SemanticStorageClass,
    pub duration: StorageDuration,
    /// Runtime bounds evaluated once when this declaration is reached.
    pub variable_length_bounds: Vec<FullTypedVariableLengthBound>,
    pub initializer: Option<FullTypedInitializer>,
    pub attributes: Vec<FullTypedAttribute>,
    /// Strongest object-specific alignment requested by a standard alignment
    /// specifier or an implemented GNU alignment attribute.
    pub requested_alignment: Option<u64>,
    /// Present for static- or thread-duration block objects. These objects are
    /// emitted as data and never initialized by a runtime stack store.
    pub emission: Option<GlobalEmission>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FullTypedBlockItem {
    Declaration(Box<FullTypedLocalDeclaration>),
    Typedef(Box<FullTypedTypedef>),
    ExternalObject(GlobalId),
    FunctionDeclaration(FullFunctionId),
    StaticAssert { value: i128, span: Span },
    Statement(Box<FullTypedStatement>),
    Pragma(PragmaEvent),
}

#[derive(Clone, Debug, PartialEq)]
pub struct FullTypedStatement {
    pub kind: FullTypedStatementKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FullTypedStatementKind {
    Label {
        label: LabelId,
        name: String,
        statement: Box<FullTypedStatement>,
    },
    Case {
        value: i128,
        statement: Box<FullTypedStatement>,
    },
    Default(Box<FullTypedStatement>),
    Compound(Vec<FullTypedBlockItem>),
    Expression(Option<FullTypedExpression>),
    If {
        condition: FullTypedExpression,
        then_statement: Box<FullTypedStatement>,
        else_statement: Option<Box<FullTypedStatement>>,
    },
    Switch {
        expression: FullTypedExpression,
        statement: Box<FullTypedStatement>,
    },
    While {
        condition: FullTypedExpression,
        statement: Box<FullTypedStatement>,
    },
    DoWhile {
        statement: Box<FullTypedStatement>,
        condition: FullTypedExpression,
    },
    For {
        initializer: FullTypedForInitializer,
        condition: Option<Box<FullTypedExpression>>,
        step: Option<Box<FullTypedExpression>>,
        statement: Box<FullTypedStatement>,
    },
    Goto {
        label: LabelId,
        name: String,
    },
    ComputedGoto(FullTypedExpression),
    Continue,
    Break,
    Return(Option<FullTypedExpression>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum FullTypedForInitializer {
    Empty,
    Expression(FullTypedExpression),
    Declarations(Vec<FullTypedBlockItem>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct FullTypedExpression {
    pub kind: FullTypedExpressionKind,
    pub ty: QualifiedType,
    pub category: ValueCategory,
    pub place: Option<Place>,
    pub constant: Option<ConstantValue>,
    /// How this expression may participate in a C integer constant expression.
    /// `UnevaluatedOnly` has permitted operands but contains an operator that
    /// C allows only inside a statically unevaluated subexpression. A floating
    /// literal is tracked separately because C permits it only as the immediate
    /// operand of an explicit cast to integer type.
    pub constant_expression_kind: ConstantExpressionKind,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConstantExpressionKind {
    Invalid,
    Integer,
    UnevaluatedOnly,
    FloatingLiteral,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValueCategory {
    Value,
    Lvalue,
    FunctionDesignator,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemoryOrder {
    SequentiallyConsistent,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AtomicReadModifyWriteOperation {
    Add,
    Subtract,
    Exchange,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IntegerIntrinsicOperation {
    ByteSwap64,
    CountLeadingZerosInt,
    CountLeadingZerosLong,
    CountLeadingZerosLongLong,
    CountTrailingZerosLongLong,
    PopulationCountInt,
    PopulationCountLongLong,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AccessSemantics {
    pub volatile: bool,
    pub atomic: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Place {
    pub base: PlaceBase,
    pub projections: Vec<PlaceProjection>,
    pub access: AccessSemantics,
    pub modifiable: bool,
    pub addressable: bool,
    pub bitfield: Option<BitfieldPlace>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlaceBase {
    Global(GlobalId),
    Local(FullLocalId),
    String(StringId),
    Indirect,
    CompoundLiteral(FullLocalId),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PlaceProjection {
    Dereference,
    Index,
    Field { index: usize, name: Option<String> },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BitfieldPlace {
    pub field_index: usize,
    /// Byte offset from the selected field's projected address to its access unit.
    pub storage_offset: u64,
    pub storage_size: u64,
    pub storage_align: u64,
    pub bit_offset: u32,
    pub width: u32,
    pub signed: bool,
    pub access: AccessSemantics,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ConstantValue {
    Signed(i128),
    Unsigned(u128),
    Floating(f64),
    NullPointer,
    Address(RelocatableAddress),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RelocatableAddress {
    pub base: RelocatableBase,
    pub addend: i128,
    pub one_past: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RelocatableBase {
    Global(GlobalId),
    BlockStatic {
        function: FullFunctionId,
        local: FullLocalId,
    },
    Function(FullFunctionId),
    String(StringId),
    Label {
        function: FullFunctionId,
        label: LabelId,
    },
}

impl ConstantValue {
    pub fn as_i128(self) -> Option<i128> {
        match self {
            Self::Signed(value) => Some(value),
            Self::Unsigned(value) => i128::try_from(value).ok(),
            Self::Floating(_) | Self::NullPointer | Self::Address(_) => None,
        }
    }

    pub fn is_zero(self) -> bool {
        match self {
            Self::Signed(value) => value == 0,
            Self::Unsigned(value) => value == 0,
            Self::Floating(value) => value == 0.0,
            Self::NullPointer => true,
            Self::Address(_) => false,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum FullTypedExpressionKind {
    Constant(ConstantValue),
    StringLiteral(StringId),
    DeclRef(SymbolReference),
    Conversion {
        kind: ConversionKind,
        expression: Box<FullTypedExpression>,
    },
    Unary {
        operator: UnaryOperator,
        operand: Box<FullTypedExpression>,
    },
    Binary {
        operator: BinaryOperator,
        left: Box<FullTypedExpression>,
        right: Box<FullTypedExpression>,
    },
    AddressOf(Box<FullTypedExpression>),
    Dereference(Box<FullTypedExpression>),
    Subscript {
        base: Box<FullTypedExpression>,
        index: Box<FullTypedExpression>,
    },
    Member {
        base: Box<FullTypedExpression>,
        field_index: usize,
        name: Option<String>,
        indirect: bool,
        bitfield: Option<Box<BitfieldPlace>>,
    },
    CompoundLiteral {
        local: FullLocalId,
        initializer: Box<FullTypedInitializer>,
    },
    Assignment {
        operator: AssignmentOperator,
        target: Box<FullTypedExpression>,
        value: Box<FullTypedExpression>,
        store: AccessSemantics,
        compound: Option<CompoundAssignmentPlan>,
    },
    Increment {
        operand: Box<FullTypedExpression>,
        decrement: bool,
        postfix: bool,
        store: AccessSemantics,
    },
    Call {
        callee: Box<FullTypedExpression>,
        function: Option<FullFunctionId>,
        arguments: Vec<FullTypedExpression>,
        variadic_boundary: usize,
    },
    Conditional {
        condition: Box<FullTypedExpression>,
        then_expression: Box<FullTypedExpression>,
        else_expression: Box<FullTypedExpression>,
    },
    Comma(Vec<FullTypedExpression>),
    BuiltinExpect {
        value: Box<FullTypedExpression>,
        expected: Box<FullTypedExpression>,
    },
    Sizeof {
        operand_ty: QualifiedType,
        size: u64,
    },
    Alignof {
        operand_ty: QualifiedType,
        align: u64,
    },
    Offsetof {
        record_ty: QualifiedType,
        path: Vec<ResolvedOffsetDesignator>,
        offset: u64,
    },
    VaStart {
        list: Box<FullTypedExpression>,
        last_named_parameter: FullLocalId,
    },
    VaArg {
        list: Box<FullTypedExpression>,
        requested: QualifiedType,
    },
    VaCopy {
        destination: Box<FullTypedExpression>,
        source: Box<FullTypedExpression>,
    },
    VaEnd {
        list: Box<FullTypedExpression>,
    },
    IntegerIntrinsic {
        operation: IntegerIntrinsicOperation,
        operand: Box<FullTypedExpression>,
    },
    Prefetch {
        address: Box<FullTypedExpression>,
        write: bool,
        locality: u8,
    },
    AtomicReadModifyWrite {
        operation: AtomicReadModifyWriteOperation,
        pointer: Box<FullTypedExpression>,
        operand: Box<FullTypedExpression>,
        object: QualifiedType,
        return_new: bool,
        order: MemoryOrder,
    },
    AtomicCompareExchange {
        pointer: Box<FullTypedExpression>,
        expected: Box<FullTypedExpression>,
        replacement: Box<FullTypedExpression>,
        object: QualifiedType,
        return_boolean: bool,
        order: MemoryOrder,
    },
    MemoryFence {
        order: MemoryOrder,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SymbolReference {
    Global(GlobalId),
    Function(FullFunctionId),
    Local(FullLocalId),
    PredefinedFunctionName(StringId),
    Enumerator { value: i128 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConversionKind {
    LvalueToValue { access: AccessSemantics },
    ArrayToPointer,
    FunctionToPointer,
    IntegerPromotion,
    IntegerConversion,
    FloatingConversion,
    IntegerToFloating,
    FloatingToInteger,
    PointerConversion,
    QualificationAdjustment,
    ToBoolean,
    ToVoid,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CompoundAssignmentPlan {
    pub operator: BinaryOperator,
    /// Type produced by loading and promoting/converting the target once.
    pub load_ty: QualifiedType,
    /// Type in which the binary operation is performed.
    pub calculation_ty: QualifiedType,
    pub load: AccessSemantics,
    /// Explicit conversion applied to the calculation before the store.
    pub result_conversion: Option<ConversionKind>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ResolvedOffsetDesignator {
    Field { index: usize, name: String },
    Index { value: u64 },
}

#[derive(Clone, Debug, PartialEq)]
pub struct FullTypedInitializer {
    pub ty: QualifiedType,
    pub kind: FullTypedInitializerKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FullTypedInitializerKind {
    Scalar(FullTypedExpression),
    Aggregate(Vec<FullTypedInitializerEntry>),
    String(StringId),
    Zero,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FullTypedInitializerEntry {
    pub path: Vec<InitializerPathElement>,
    pub initializer: Box<FullTypedInitializer>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InitializerPathElement {
    Index(u64),
    Field {
        index: usize,
        name: Option<String>,
        bitfield: Option<BitfieldPlace>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppliedPacking {
    pub policy: PackingPolicy,
    pub stack_depth: usize,
    pub label: Option<String>,
}
