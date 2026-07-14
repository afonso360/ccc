use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;

use ccc_target::{PackingPolicy, TargetDataLayout};

use crate::{
    ArrayType, BuiltinType, EnumBody, EnumDefinition, EnumId, Enumerator, FunctionParameters,
    FunctionType, LayoutError, QualifiedType, RecordDefinition, RecordId, RecordKind, TypeId,
    TypeKind, TypeLayout, VariableLengthId,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct LayoutCacheKey {
    pub ty: TypeId,
    pub target: TargetDataLayout,
    pub packing: PackingPolicy,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DefinitionError {
    UnknownRecord(RecordId),
    RecordAlreadyComplete(RecordId),
    UnknownEnum(EnumId),
    EnumAlreadyComplete(EnumId),
    InvalidEnumUnderlying(TypeId),
}

impl fmt::Display for DefinitionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownRecord(id) => write!(formatter, "unknown record {}", id.0),
            Self::RecordAlreadyComplete(id) => {
                write!(formatter, "record {} is already complete", id.0)
            }
            Self::UnknownEnum(id) => write!(formatter, "unknown enum {}", id.0),
            Self::EnumAlreadyComplete(id) => {
                write!(formatter, "enum {} is already complete", id.0)
            }
            Self::InvalidEnumUnderlying(ty) => {
                write!(
                    formatter,
                    "type {} is not an integer enum representation",
                    ty.0
                )
            }
        }
    }
}

impl std::error::Error for DefinitionError {}

#[derive(Clone, Debug)]
pub struct TypeStore {
    pub(crate) kinds: Vec<TypeKind>,
    interned: HashMap<TypeKind, TypeId>,
    pub(crate) records: Vec<RecordDefinition>,
    pub(crate) enums: Vec<EnumDefinition>,
    next_variable_length: u32,
    pub(crate) layout_cache: RefCell<Vec<(LayoutCacheKey, Result<TypeLayout, LayoutError>)>>,
}

impl Default for TypeStore {
    fn default() -> Self {
        let kinds = vec![
            TypeKind::Builtin(BuiltinType::Void),
            TypeKind::Builtin(BuiltinType::Int),
            TypeKind::Builtin(BuiltinType::Bool),
            TypeKind::Builtin(BuiltinType::Char),
            TypeKind::Builtin(BuiltinType::SignedChar),
            TypeKind::Builtin(BuiltinType::UnsignedChar),
            TypeKind::Builtin(BuiltinType::Short),
            TypeKind::Builtin(BuiltinType::UnsignedShort),
            TypeKind::Builtin(BuiltinType::UnsignedInt),
            TypeKind::Builtin(BuiltinType::Long),
            TypeKind::Builtin(BuiltinType::UnsignedLong),
            TypeKind::Builtin(BuiltinType::LongLong),
            TypeKind::Builtin(BuiltinType::UnsignedLongLong),
            TypeKind::Builtin(BuiltinType::Float),
            TypeKind::Builtin(BuiltinType::Double),
            TypeKind::Builtin(BuiltinType::LongDouble),
        ];
        let interned = kinds
            .iter()
            .cloned()
            .enumerate()
            .map(|(index, kind)| (kind, TypeId(index as u32)))
            .collect();
        Self {
            kinds,
            interned,
            records: Vec::new(),
            enums: Vec::new(),
            next_variable_length: 0,
            layout_cache: RefCell::new(Vec::new()),
        }
    }
}

impl TypeStore {
    pub fn kind(&self, id: TypeId) -> &TypeKind {
        &self.kinds[id.index()]
    }

    pub fn try_kind(&self, id: TypeId) -> Option<&TypeKind> {
        self.kinds.get(id.index())
    }

    pub fn contains(&self, id: TypeId) -> bool {
        id.index() < self.kinds.len()
    }

    pub const fn builtin(&self, kind: BuiltinType) -> TypeId {
        TypeId::builtin(kind)
    }

    pub fn builtin_type(&self, id: TypeId) -> Option<BuiltinType> {
        match self.try_kind(id)? {
            TypeKind::Builtin(kind) => Some(*kind),
            _ => None,
        }
    }

    pub fn is_integer(&self, id: TypeId) -> bool {
        self.builtin_type(id).is_some_and(BuiltinType::is_integer)
            || matches!(self.try_kind(id), Some(TypeKind::Enum(_)))
    }

    pub fn is_arithmetic(&self, id: TypeId) -> bool {
        self.builtin_type(id)
            .is_some_and(|kind| kind.is_integer() || kind.is_floating())
            || matches!(self.try_kind(id), Some(TypeKind::Enum(_)))
    }

    pub fn function_type(&mut self, signature: FunctionType) -> TypeId {
        self.intern(TypeKind::Function(signature))
    }

    pub fn function_signature(&self, id: TypeId) -> Option<FunctionType> {
        match self.try_kind(id)? {
            TypeKind::Function(signature) => Some(signature.clone()),
            _ => None,
        }
    }

    pub fn pointer(&mut self, pointee: impl Into<QualifiedType>) -> TypeId {
        self.intern(TypeKind::Pointer(crate::PointerType {
            pointee: pointee.into(),
        }))
    }

    pub fn array(&mut self, array: ArrayType) -> TypeId {
        self.intern(TypeKind::Array(array))
    }

    pub fn fresh_variable_length(&mut self) -> VariableLengthId {
        let id = VariableLengthId(self.next_variable_length);
        self.next_variable_length = self
            .next_variable_length
            .checked_add(1)
            .expect("variable-length type id space exhausted");
        id
    }

    pub fn declare_record(&mut self, kind: RecordKind, tag: Option<String>) -> (RecordId, TypeId) {
        let id = RecordId(self.records.len() as u32);
        self.records.push(RecordDefinition {
            id,
            kind,
            tag,
            fields: None,
            packing: PackingPolicy::NATIVE,
        });
        let ty = self.intern(TypeKind::Record(id));
        (id, ty)
    }

    pub fn record(&self, id: RecordId) -> Option<&RecordDefinition> {
        self.records.get(id.index())
    }

    pub fn complete_record(
        &mut self,
        id: RecordId,
        fields: Vec<crate::Field>,
    ) -> Result<(), DefinitionError> {
        self.complete_record_with_packing(id, fields, PackingPolicy::NATIVE)
    }

    pub fn complete_record_with_packing(
        &mut self,
        id: RecordId,
        fields: Vec<crate::Field>,
        packing: PackingPolicy,
    ) -> Result<(), DefinitionError> {
        let definition = self
            .records
            .get_mut(id.index())
            .ok_or(DefinitionError::UnknownRecord(id))?;
        if definition.fields.is_some() {
            return Err(DefinitionError::RecordAlreadyComplete(id));
        }
        definition.fields = Some(fields);
        definition.packing = packing;
        self.layout_cache.get_mut().clear();
        Ok(())
    }

    pub fn declare_enum(&mut self, tag: Option<String>) -> (EnumId, TypeId) {
        let id = EnumId(self.enums.len() as u32);
        self.enums.push(EnumDefinition {
            id,
            tag,
            body: None,
        });
        let ty = self.intern(TypeKind::Enum(id));
        (id, ty)
    }

    pub fn enumeration(&self, id: EnumId) -> Option<&EnumDefinition> {
        self.enums.get(id.index())
    }

    pub fn complete_enum(
        &mut self,
        id: EnumId,
        underlying: TypeId,
        enumerators: Vec<Enumerator>,
    ) -> Result<(), DefinitionError> {
        if !self
            .builtin_type(underlying)
            .is_some_and(BuiltinType::is_integer)
        {
            return Err(DefinitionError::InvalidEnumUnderlying(underlying));
        }
        let definition = self
            .enums
            .get_mut(id.index())
            .ok_or(DefinitionError::UnknownEnum(id))?;
        if definition.body.is_some() {
            return Err(DefinitionError::EnumAlreadyComplete(id));
        }
        definition.body = Some(EnumBody {
            underlying,
            enumerators,
        });
        self.layout_cache.get_mut().clear();
        Ok(())
    }

    pub fn display(&self, id: TypeId) -> String {
        let Some(kind) = self.try_kind(id) else {
            return format!("<invalid-type-{}>", id.0);
        };
        match kind {
            TypeKind::Builtin(kind) => kind.spelling().to_owned(),
            TypeKind::Pointer(pointer) => {
                format!("pointer to {}", self.display_qualified(pointer.pointee))
            }
            TypeKind::Array(array) => {
                let length = match array.length {
                    crate::ArrayLength::Incomplete => "".to_owned(),
                    crate::ArrayLength::Constant(length) => length.to_string(),
                    crate::ArrayLength::Variable(id) => format!("vla{}", id.0),
                };
                format!(
                    "array[{length}] of {}",
                    self.display_qualified(array.element)
                )
            }
            TypeKind::Function(signature) => self.display_function(signature),
            TypeKind::Enum(id) => self.display_enum(*id),
            TypeKind::Record(id) => self.display_record(*id),
        }
    }

    pub fn display_qualified(&self, ty: QualifiedType) -> String {
        let mut qualifiers = Vec::new();
        for (qualifier, spelling) in [
            (crate::TypeQualifiers::CONST, "const"),
            (crate::TypeQualifiers::VOLATILE, "volatile"),
            (crate::TypeQualifiers::RESTRICT, "restrict"),
            (crate::TypeQualifiers::ATOMIC, "_Atomic"),
        ] {
            if ty.qualifiers.contains(qualifier) {
                qualifiers.push(spelling);
            }
        }
        let prefix = qualifiers.into_iter().collect::<Vec<_>>().join(" ");
        if prefix.is_empty() {
            self.display(ty.ty)
        } else {
            format!("{prefix} {}", self.display(ty.ty))
        }
    }

    fn display_function(&self, signature: &FunctionType) -> String {
        let parameters = match &signature.parameters {
            FunctionParameters::Unspecified => String::new(),
            FunctionParameters::Prototype(parameters) => {
                let mut rendered = parameters
                    .iter()
                    .map(|parameter| self.display_qualified(*parameter))
                    .collect::<Vec<_>>();
                if signature.variadic {
                    rendered.push("...".to_owned());
                }
                rendered.join(", ")
            }
        };
        format!(
            "{} ({parameters})",
            self.display_qualified(signature.result)
        )
    }

    fn display_record(&self, id: RecordId) -> String {
        let Some(record) = self.record(id) else {
            return format!("<invalid-record-{}>", id.0);
        };
        let name = record
            .tag
            .clone()
            .unwrap_or_else(|| format!("<anonymous-{}>", id.0));
        format!("{} {name}", record.kind.spelling())
    }

    fn display_enum(&self, id: EnumId) -> String {
        let Some(enumeration) = self.enumeration(id) else {
            return format!("<invalid-enum-{}>", id.0);
        };
        let name = enumeration
            .tag
            .clone()
            .unwrap_or_else(|| format!("<anonymous-{}>", id.0));
        format!("enum {name}")
    }

    fn intern(&mut self, kind: TypeKind) -> TypeId {
        if let Some(id) = self.interned.get(&kind) {
            return *id;
        }
        let id = TypeId(self.kinds.len() as u32);
        self.kinds.push(kind.clone());
        self.interned.insert(kind, id);
        self.layout_cache.get_mut().clear();
        id
    }
}
