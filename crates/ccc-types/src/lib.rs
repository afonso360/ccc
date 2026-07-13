//! Canonical C types and target-derived layout queries.

use std::collections::HashMap;

use ccc_target::EffectiveCompilationConfig;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TypeId(u32);

impl TypeId {
    pub const VOID: Self = Self(0);
    pub const INT: Self = Self(1);

    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum TypeKind {
    Void,
    Int,
    Function {
        result: TypeId,
        parameters: Vec<TypeId>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Layout {
    pub size: u64,
    pub align: u64,
}

#[derive(Clone, Debug)]
pub struct TypeStore {
    kinds: Vec<TypeKind>,
    interned: HashMap<TypeKind, TypeId>,
}

impl Default for TypeStore {
    fn default() -> Self {
        let kinds = vec![TypeKind::Void, TypeKind::Int];
        let interned = kinds
            .iter()
            .cloned()
            .enumerate()
            .map(|(index, kind)| (kind, TypeId(index as u32)))
            .collect();
        Self { kinds, interned }
    }
}

impl TypeStore {
    pub fn kind(&self, id: TypeId) -> &TypeKind {
        &self.kinds[id.index()]
    }

    pub fn contains(&self, id: TypeId) -> bool {
        id.index() < self.kinds.len()
    }

    pub fn function(&mut self, result: TypeId, parameters: Vec<TypeId>) -> TypeId {
        self.intern(TypeKind::Function { result, parameters })
    }

    pub fn layout(&self, id: TypeId, config: &EffectiveCompilationConfig) -> Option<Layout> {
        match self.kind(id) {
            TypeKind::Int => config.target.int_width().map(|width| Layout {
                size: u64::from(width / 8),
                align: u64::from(config.target.int_align),
            }),
            TypeKind::Void | TypeKind::Function { .. } => None,
        }
    }

    pub fn display(&self, id: TypeId) -> String {
        match self.kind(id) {
            TypeKind::Void => "void".to_owned(),
            TypeKind::Int => "int".to_owned(),
            TypeKind::Function { result, parameters } => {
                let parameters = parameters
                    .iter()
                    .map(|parameter| self.display(*parameter))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{} ({parameters})", self.display(*result))
            }
        }
    }

    fn intern(&mut self, kind: TypeKind) -> TypeId {
        if let Some(id) = self.interned.get(&kind) {
            return *id;
        }
        let id = TypeId(self.kinds.len() as u32);
        self.kinds.push(kind.clone());
        self.interned.insert(kind, id);
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interns_function_types_and_uses_target_layout() {
        let mut types = TypeStore::default();
        let first = types.function(TypeId::INT, vec![TypeId::INT]);
        let second = types.function(TypeId::INT, vec![TypeId::INT]);
        assert_eq!(first, second);
        assert_eq!(
            types.layout(TypeId::INT, &EffectiveCompilationConfig::default()),
            Some(Layout { size: 4, align: 4 })
        );
    }
}
