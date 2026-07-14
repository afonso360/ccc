use std::fmt;

use ccc_sema::generic::{
    FullFunctionId, FullLocalId, FunctionProperties, GlobalEmission, GlobalId, Linkage,
    SemanticStorageClass, StorageDuration, StringId,
};
use ccc_session::Span;
use ccc_types::{QualifiedType, TypeId, TypeStore};

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BlockId(pub u32);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ValueId(pub u32);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct InstructionId(pub u32);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct StorageId(pub u32);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct InitializerNodeId(pub u32);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct DataId(pub u32);

#[derive(Clone, Debug)]
pub struct FullModule {
    pub types: TypeStore,
    pub globals: Vec<FullGlobal>,
    pub strings: Vec<FullString>,
    /// Includes declarations so consumers can declare every symbol before
    /// translating function bodies.
    pub functions: Vec<FullFunction>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FullGlobal {
    pub id: DataId,
    pub source: DataOrigin,
    pub name: String,
    pub ty: QualifiedType,
    pub storage: SemanticStorageClass,
    pub linkage: Linkage,
    pub duration: StorageDuration,
    pub initializer: Option<InitializerGraph>,
    pub tentative: bool,
    pub emission: GlobalEmission,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DataOrigin {
    FileScope(GlobalId),
    BlockStatic {
        function: FullFunctionId,
        local: FullLocalId,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StringEncoding {
    Ordinary,
    Utf8,
    Wide,
    Utf16,
    Utf32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FullString {
    pub id: StringId,
    pub encoding: StringEncoding,
    pub code_units: Vec<u32>,
    pub ty: QualifiedType,
}

#[derive(Clone, Debug, PartialEq)]
pub struct InitializerGraph {
    pub root: InitializerNodeId,
    pub nodes: Vec<InitializerNode>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct InitializerNode {
    pub id: InitializerNodeId,
    pub ty: QualifiedType,
    pub kind: InitializerNodeKind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum InitializerNodeKind {
    Zero,
    Scalar(ScalarConstant),
    Relocation {
        target: RelocationTarget,
        addend: i128,
        one_past: bool,
        kind: RelocationKind,
    },
    /// Copies a destination-limited prefix of the literal. The count can
    /// exclude the trailing zero when the array bound exactly fits the
    /// nonzero code units.
    StringData {
        string: StringId,
        copy_code_units: u64,
    },
    /// Places one child fragment in consecutive elements of an array.
    Repeat {
        element: InitializerNodeId,
        count: u64,
    },
    Aggregate(Vec<InitializerEdge>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct InitializerEdge {
    pub path: Vec<InitializerPath>,
    pub node: InitializerNodeId,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InitializerPath {
    Index(u64),
    Field {
        index: usize,
        name: Option<String>,
        bitfield: Option<BitfieldDescriptor>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ScalarConstant {
    Signed(i128),
    Unsigned(u128),
    Floating(f64),
    NullPointer,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RelocationTarget {
    Object(DataId),
    Function(FullFunctionId),
    String(StringId),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RelocationKind {
    ObjectAddress,
    FunctionAddress,
    StringAddress,
    ThreadLocalAddress,
}

#[derive(Clone, Debug)]
pub struct FullFunction {
    pub id: FullFunctionId,
    pub name: String,
    /// The canonical C function type used for both direct and indirect calls.
    pub signature: TypeId,
    pub storage_class: SemanticStorageClass,
    pub linkage: Linkage,
    pub properties: FunctionProperties,
    pub symbol_name: String,
    pub result_type: QualifiedType,
    pub parameters: Vec<FullParameter>,
    pub storage: Vec<FullStorage>,
    pub blocks: Vec<FullBlock>,
    pub entry: Option<BlockId>,
    /// Canonical, top-level-unqualified types for SSA values. Qualifiers are
    /// properties of declarations and memory places, not values.
    pub value_types: Vec<TypeId>,
    pub instruction_count: u32,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FullParameter {
    pub local: FullLocalId,
    pub name: String,
    pub ty: QualifiedType,
    /// Present for definitions and absent for declarations.
    pub incoming: Option<ValueId>,
    pub storage: Option<StorageId>,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FullStorage {
    pub id: StorageId,
    pub local: FullLocalId,
    pub name: String,
    pub ty: QualifiedType,
    pub duration: StorageDuration,
    pub location: StorageLocation,
    pub required_by: Vec<MemoryResidencyReason>,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageLocation {
    Automatic,
    Static,
    ThreadLocal,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum MemoryResidencyReason {
    AddressTaken,
    Volatile,
    Aggregate,
    Atomic,
    VariablyModified,
}

#[derive(Clone, Debug)]
pub struct FullBlock {
    pub id: BlockId,
    pub parameters: Vec<ValueId>,
    pub instructions: Vec<FullInstruction>,
    pub terminator: Option<FullTerminator>,
}

#[derive(Clone, Debug)]
pub struct FullInstruction {
    pub id: InstructionId,
    pub result: Option<ValueId>,
    pub kind: FullInstructionKind,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum FullInstructionKind {
    Constant(ScalarConstant),
    AddressConstant {
        target: RelocationTarget,
        addend: i128,
        one_past: bool,
    },
    AddressOfGlobal {
        global: DataId,
    },
    AddressOfFunction {
        function: FullFunctionId,
        signature: TypeId,
    },
    AddressOfString {
        string: StringId,
    },
    AddressOfStorage {
        storage: StorageId,
    },
    ProjectField {
        base: ValueId,
        record: QualifiedType,
        field_index: usize,
        field_name: Option<String>,
    },
    PointerOffset {
        base: ValueId,
        index: ValueId,
        element: QualifiedType,
        subtract: bool,
    },
    PointerDifference {
        left: ValueId,
        right: ValueId,
        element: QualifiedType,
    },
    Load {
        address: ValueId,
        object: QualifiedType,
        access: MemoryAccess,
    },
    Store {
        address: ValueId,
        value: ValueId,
        object: QualifiedType,
        access: MemoryAccess,
    },
    BitfieldLoad {
        address: ValueId,
        descriptor: BitfieldDescriptor,
        access: MemoryAccess,
    },
    BitfieldStore {
        address: ValueId,
        value: ValueId,
        descriptor: BitfieldDescriptor,
        access: MemoryAccess,
    },
    ZeroInitialize {
        destination: ValueId,
        object: QualifiedType,
    },
    StringInitialize {
        destination: ValueId,
        string: StringId,
        object: QualifiedType,
        copy_code_units: u64,
    },
    AggregateCopy {
        destination: ValueId,
        source: ValueId,
        destination_object: QualifiedType,
        source_object: QualifiedType,
        destination_access: MemoryAccess,
        source_access: MemoryAccess,
        overlap: AggregateOverlap,
    },
    AggregateValue {
        address: ValueId,
        object: QualifiedType,
        access: MemoryAccess,
    },
    Convert {
        kind: ScalarConversion,
        operand: ValueId,
        from: QualifiedType,
        to: QualifiedType,
    },
    Unary {
        operator: UnaryOperation,
        operand: ValueId,
    },
    Binary {
        operator: BinaryOperation,
        left: ValueId,
        right: ValueId,
    },
    DirectCall {
        function: FullFunctionId,
        signature: TypeId,
        arguments: Vec<ValueId>,
        variadic_boundary: usize,
        effects: CallEffects,
    },
    IndirectCall {
        callee: ValueId,
        signature: TypeId,
        arguments: Vec<ValueId>,
        variadic_boundary: usize,
        effects: CallEffects,
    },
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MemoryAccess {
    pub volatile: bool,
    pub atomic: Option<MemoryOrder>,
    pub non_elidable: bool,
    pub non_movable: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemoryOrder {
    SequentiallyConsistent,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BitfieldDescriptor {
    pub field_index: usize,
    pub storage_offset: u64,
    pub storage_size: u64,
    pub storage_align: u64,
    pub bit_offset: u32,
    pub width: u32,
    pub signed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AggregateOverlap {
    MayOverlap,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScalarConversion {
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
pub enum UnaryOperation {
    Plus,
    Negate,
    BitwiseNot,
    LogicalNot,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BinaryOperation {
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
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CallEffects {
    pub reads_memory: bool,
    pub writes_memory: bool,
    pub may_unwind: bool,
    pub no_return: bool,
}

impl Default for CallEffects {
    fn default() -> Self {
        Self {
            reads_memory: true,
            writes_memory: true,
            may_unwind: true,
            no_return: false,
        }
    }
}

#[derive(Clone, Debug)]
pub enum FullTerminator {
    Branch(FullEdge),
    Conditional {
        condition: ValueId,
        then_edge: FullEdge,
        else_edge: FullEdge,
    },
    Switch {
        selector: ValueId,
        cases: Vec<SwitchEdge>,
        default: FullEdge,
    },
    Return(Option<ValueId>),
    Unreachable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FullEdge {
    pub target: BlockId,
    pub arguments: Vec<ValueId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SwitchEdge {
    pub value: i128,
    pub edge: FullEdge,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IrError {
    pub code: &'static str,
    pub message: String,
    pub span: Option<Span>,
}

impl IrError {
    pub(crate) fn lower(code: &'static str, span: Span, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            span: Some(span),
        }
    }

    pub(crate) fn verify(message: impl Into<String>) -> Self {
        Self {
            code: "CCC3102",
            message: message.into(),
            span: None,
        }
    }
}

impl fmt::Display for IrError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.message.fmt(formatter)
    }
}

impl std::error::Error for IrError {}
