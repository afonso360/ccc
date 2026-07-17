use std::ops::{BitOr, BitOrAssign};

use ccc_target::PackingPolicy;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TypeId(pub(crate) u32);

impl TypeId {
    pub const VOID: Self = Self(0);
    pub const INT: Self = Self(1);
    pub const BOOL: Self = Self(2);
    pub const CHAR: Self = Self(3);
    pub const SIGNED_CHAR: Self = Self(4);
    pub const UNSIGNED_CHAR: Self = Self(5);
    pub const SHORT: Self = Self(6);
    pub const UNSIGNED_SHORT: Self = Self(7);
    pub const UNSIGNED_INT: Self = Self(8);
    pub const LONG: Self = Self(9);
    pub const UNSIGNED_LONG: Self = Self(10);
    pub const LONG_LONG: Self = Self(11);
    pub const UNSIGNED_LONG_LONG: Self = Self(12);
    pub const FLOAT: Self = Self(13);
    pub const DOUBLE: Self = Self(14);
    pub const LONG_DOUBLE: Self = Self(15);

    pub const fn index(self) -> usize {
        self.0 as usize
    }

    pub const fn builtin(kind: BuiltinType) -> Self {
        match kind {
            BuiltinType::Void => Self::VOID,
            BuiltinType::Bool => Self::BOOL,
            BuiltinType::Char => Self::CHAR,
            BuiltinType::SignedChar => Self::SIGNED_CHAR,
            BuiltinType::UnsignedChar => Self::UNSIGNED_CHAR,
            BuiltinType::Short => Self::SHORT,
            BuiltinType::UnsignedShort => Self::UNSIGNED_SHORT,
            BuiltinType::Int => Self::INT,
            BuiltinType::UnsignedInt => Self::UNSIGNED_INT,
            BuiltinType::Long => Self::LONG,
            BuiltinType::UnsignedLong => Self::UNSIGNED_LONG,
            BuiltinType::LongLong => Self::LONG_LONG,
            BuiltinType::UnsignedLongLong => Self::UNSIGNED_LONG_LONG,
            BuiltinType::Float => Self::FLOAT,
            BuiltinType::Double => Self::DOUBLE,
            BuiltinType::LongDouble => Self::LONG_DOUBLE,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum BuiltinType {
    Void,
    Bool,
    Char,
    SignedChar,
    UnsignedChar,
    Short,
    UnsignedShort,
    Int,
    UnsignedInt,
    Long,
    UnsignedLong,
    LongLong,
    UnsignedLongLong,
    Float,
    Double,
    LongDouble,
}

impl BuiltinType {
    pub const ALL: [Self; 16] = [
        Self::Void,
        Self::Bool,
        Self::Char,
        Self::SignedChar,
        Self::UnsignedChar,
        Self::Short,
        Self::UnsignedShort,
        Self::Int,
        Self::UnsignedInt,
        Self::Long,
        Self::UnsignedLong,
        Self::LongLong,
        Self::UnsignedLongLong,
        Self::Float,
        Self::Double,
        Self::LongDouble,
    ];

    pub const fn is_integer(self) -> bool {
        matches!(
            self,
            Self::Bool
                | Self::Char
                | Self::SignedChar
                | Self::UnsignedChar
                | Self::Short
                | Self::UnsignedShort
                | Self::Int
                | Self::UnsignedInt
                | Self::Long
                | Self::UnsignedLong
                | Self::LongLong
                | Self::UnsignedLongLong
        )
    }

    pub const fn is_floating(self) -> bool {
        matches!(self, Self::Float | Self::Double | Self::LongDouble)
    }

    pub const fn spelling(self) -> &'static str {
        match self {
            Self::Void => "void",
            Self::Bool => "_Bool",
            Self::Char => "char",
            Self::SignedChar => "signed char",
            Self::UnsignedChar => "unsigned char",
            Self::Short => "short int",
            Self::UnsignedShort => "unsigned short int",
            Self::Int => "int",
            Self::UnsignedInt => "unsigned int",
            Self::Long => "long int",
            Self::UnsignedLong => "unsigned long int",
            Self::LongLong => "long long int",
            Self::UnsignedLongLong => "unsigned long long int",
            Self::Float => "float",
            Self::Double => "double",
            Self::LongDouble => "long double",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct TypeQualifiers(u8);

impl TypeQualifiers {
    pub const NONE: Self = Self(0);
    pub const CONST: Self = Self(1 << 0);
    pub const VOLATILE: Self = Self(1 << 1);
    pub const RESTRICT: Self = Self(1 << 2);
    pub const ATOMIC: Self = Self(1 << 3);

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl BitOr for TypeQualifiers {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for TypeQualifiers {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct QualifiedType {
    pub ty: TypeId,
    pub qualifiers: TypeQualifiers,
}

impl QualifiedType {
    pub const fn new(ty: TypeId, qualifiers: TypeQualifiers) -> Self {
        Self { ty, qualifiers }
    }

    pub const fn unqualified(ty: TypeId) -> Self {
        Self::new(ty, TypeQualifiers::NONE)
    }

    pub const fn is_unqualified(self) -> bool {
        self.qualifiers.is_empty()
    }
}

impl From<TypeId> for QualifiedType {
    fn from(ty: TypeId) -> Self {
        Self::unqualified(ty)
    }
}

pub type QualType = QualifiedType;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct VariableLengthId(pub u32);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ArrayLength {
    Incomplete,
    Constant(u64),
    Variable(VariableLengthId),
    /// A `[*]` bound from function prototype scope. Unlike an incomplete
    /// array bound, this denotes a variable-length array whose bound is
    /// intentionally unspecified by the declaration.
    UnspecifiedVariable(VariableLengthId),
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct PointerType {
    pub pointee: QualifiedType,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ArrayType {
    pub element: QualifiedType,
    pub length: ArrayLength,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum FunctionParameters {
    Unspecified,
    Prototype(Vec<QualifiedType>),
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct FunctionType {
    pub result: QualifiedType,
    pub parameters: FunctionParameters,
    pub variadic: bool,
}

impl FunctionType {
    pub fn prototype(result: impl Into<QualifiedType>, parameters: Vec<QualifiedType>) -> Self {
        Self {
            result: result.into(),
            parameters: FunctionParameters::Prototype(parameters),
            variadic: false,
        }
    }

    pub fn variadic(result: impl Into<QualifiedType>, parameters: Vec<QualifiedType>) -> Self {
        Self {
            result: result.into(),
            parameters: FunctionParameters::Prototype(parameters),
            variadic: true,
        }
    }

    pub fn unspecified(result: impl Into<QualifiedType>) -> Self {
        Self {
            result: result.into(),
            parameters: FunctionParameters::Unspecified,
            variadic: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RecordId(pub u32);

impl RecordId {
    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RecordKind {
    Struct,
    Union,
}

impl RecordKind {
    pub const fn spelling(self) -> &'static str {
        match self {
            Self::Struct => "struct",
            Self::Union => "union",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Bitfield {
    pub width: u32,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Field {
    pub name: Option<String>,
    pub ty: QualifiedType,
    pub bitfield: Option<Bitfield>,
}

impl Field {
    pub fn new(name: Option<String>, ty: impl Into<QualifiedType>) -> Self {
        Self {
            name,
            ty: ty.into(),
            bitfield: None,
        }
    }

    pub fn named(name: impl Into<String>, ty: impl Into<QualifiedType>) -> Self {
        Self::new(Some(name.into()), ty)
    }

    pub fn anonymous(ty: impl Into<QualifiedType>) -> Self {
        Self::new(None, ty)
    }

    pub fn bitfield(name: Option<String>, ty: impl Into<QualifiedType>, width: u32) -> Self {
        Self {
            name,
            ty: ty.into(),
            bitfield: Some(Bitfield { width }),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecordDefinition {
    pub id: RecordId,
    pub kind: RecordKind,
    pub tag: Option<String>,
    pub fields: Option<Vec<Field>>,
    pub packing: PackingPolicy,
}

impl RecordDefinition {
    pub fn is_complete(&self) -> bool {
        self.fields.is_some()
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct EnumId(pub u32);

impl EnumId {
    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Enumerator {
    pub name: String,
    pub value: i128,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EnumBody {
    pub underlying: TypeId,
    pub enumerators: Vec<Enumerator>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EnumDefinition {
    pub id: EnumId,
    pub tag: Option<String>,
    pub body: Option<EnumBody>,
}

impl EnumDefinition {
    pub fn is_complete(&self) -> bool {
        self.body.is_some()
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum TypeKind {
    Builtin(BuiltinType),
    Pointer(PointerType),
    Array(ArrayType),
    Function(FunctionType),
    Enum(EnumId),
    Record(RecordId),
}
