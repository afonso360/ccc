//! Canonical C types and target-derived layout queries.

mod layout;
mod model;
mod store;

pub use layout::{BitfieldLayout, FieldLayout, LayoutError, LayoutShape, RecordLayout, TypeLayout};
pub use model::{
    ArrayLength, ArrayType, Bitfield, BuiltinType, EnumBody, EnumDefinition, EnumId, Enumerator,
    Field, FunctionParameters, FunctionType, PointerType, QualType, QualifiedType,
    RecordDefinition, RecordId, RecordKind, TypeId, TypeKind, TypeQualifiers, VariableLengthId,
};
pub use store::{DefinitionError, TypeStore};

#[cfg(test)]
mod tests {
    use ccc_target::EffectiveCompilationConfig;

    use super::*;

    #[test]
    fn interns_canonical_function_types() {
        let mut types = TypeStore::default();
        let signature =
            FunctionType::prototype(TypeId::INT, vec![QualifiedType::unqualified(TypeId::INT)]);
        let first = types.function_type(signature.clone());
        let second = types.function_type(signature.clone());
        assert_eq!(first, second);
        assert_eq!(
            types.kind(TypeId::INT),
            &TypeKind::Builtin(BuiltinType::Int)
        );
        assert_eq!(types.kind(first), &TypeKind::Function(signature.clone()));
        assert_eq!(types.display(first), "int (int)");
        assert_eq!(
            types
                .layout_of(TypeId::INT, &EffectiveCompilationConfig::default())
                .unwrap()
                .size,
            4
        );
        assert_eq!(types.function_signature(first), Some(signature));

        let variadic = types.function_type(FunctionType::variadic(
            TypeId::INT,
            vec![QualifiedType::unqualified(TypeId::INT)],
        ));
        assert_ne!(variadic, first);
        assert!(matches!(types.kind(variadic), TypeKind::Function(_)));
    }

    #[test]
    fn layout_cache_is_keyed_by_target_layout_and_invalidated_by_new_types() {
        let mut types = TypeStore::default();
        let pointer = types.pointer(TypeId::INT);
        let config = EffectiveCompilationConfig::default();

        assert_eq!(types.layout_of(pointer, &config).unwrap().size, 8);
        assert_eq!(types.layout_of(pointer, &config).unwrap().size, 8);
        assert_eq!(types.layout_cache.borrow().len(), 1);

        let mut narrow = config.clone();
        narrow.target.data_layout.pointer_width = 32;
        narrow.target.data_layout.pointer_align = 4;
        assert_eq!(types.layout_of(pointer, &narrow).unwrap().size, 4);
        assert_eq!(types.layout_cache.borrow().len(), 2);

        let _ = types.array(ArrayType {
            element: TypeId::INT.into(),
            length: ArrayLength::Constant(2),
        });
        assert!(types.layout_cache.borrow().is_empty());
    }
}
