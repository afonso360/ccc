//! Canonical C types and target-derived layout queries.

pub use ccc_target::TargetBuiltinType;

mod layout;
mod model;
mod store;

pub use layout::{BitfieldLayout, FieldLayout, LayoutError, LayoutShape, RecordLayout, TypeLayout};
pub use model::{
    ArrayLength, ArrayType, Bitfield, BuiltinType, EnumBody, EnumDefinition, EnumId, Enumerator,
    Field, FunctionParameters, FunctionType, PointerType, QualType, QualifiedType,
    RecordDefinition, RecordId, RecordKind, TypeId, TypeKind, TypeQualifiers, VariableLengthId,
};
pub use store::{DefinitionError, TargetBuiltinTypeError, TypeStore};

#[cfg(test)]
mod tests {
    use ccc_target::EffectiveCompilationConfig;

    use super::*;

    #[test]
    fn builtin_ids_and_store_prefix_remain_append_only_and_in_lockstep() {
        let expected = [
            (BuiltinType::Void, TypeId::VOID, 0),
            (BuiltinType::Int, TypeId::INT, 1),
            (BuiltinType::Bool, TypeId::BOOL, 2),
            (BuiltinType::Char, TypeId::CHAR, 3),
            (BuiltinType::SignedChar, TypeId::SIGNED_CHAR, 4),
            (BuiltinType::UnsignedChar, TypeId::UNSIGNED_CHAR, 5),
            (BuiltinType::Short, TypeId::SHORT, 6),
            (BuiltinType::UnsignedShort, TypeId::UNSIGNED_SHORT, 7),
            (BuiltinType::UnsignedInt, TypeId::UNSIGNED_INT, 8),
            (BuiltinType::Long, TypeId::LONG, 9),
            (BuiltinType::UnsignedLong, TypeId::UNSIGNED_LONG, 10),
            (BuiltinType::LongLong, TypeId::LONG_LONG, 11),
            (
                BuiltinType::UnsignedLongLong,
                TypeId::UNSIGNED_LONG_LONG,
                12,
            ),
            (BuiltinType::Float, TypeId::FLOAT, 13),
            (BuiltinType::Double, TypeId::DOUBLE, 14),
            (BuiltinType::LongDouble, TypeId::LONG_DOUBLE, 15),
            (BuiltinType::Int128, TypeId::INT128, 16),
            (BuiltinType::UnsignedInt128, TypeId::UNSIGNED_INT128, 17),
            (BuiltinType::Float16, TypeId::FLOAT16, 18),
        ];
        assert_eq!(BuiltinType::ALL.len(), expected.len());

        let types = TypeStore::default();
        for (kind, id, index) in expected {
            assert!(BuiltinType::ALL.contains(&kind));
            assert_eq!(id.index(), index);
            assert_eq!(TypeId::builtin(kind), id);
            assert_eq!(types.kind(id), &TypeKind::Builtin(kind));
        }
    }

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

    #[test]
    fn target_va_list_is_a_canonical_array_of_one_public_abi_record() {
        let mut types = TypeStore::default();
        let config = EffectiveCompilationConfig::default();
        let first = types
            .target_builtin(TargetBuiltinType::VaList, &config)
            .unwrap();
        let second = types
            .target_builtin(TargetBuiltinType::VaList, &config)
            .unwrap();
        assert_eq!(first, second);
        assert_eq!(
            types.target_builtin_type(first),
            Some(TargetBuiltinType::VaList)
        );
        let TypeKind::Array(array) = types.kind(first) else {
            panic!("va_list must retain array parameter-adjustment semantics")
        };
        assert_eq!(array.length, ArrayLength::Constant(1));
        let layout = types.layout_of(first, &config).unwrap();
        assert_eq!((layout.size, layout.align), (24, 8));
        let record = types.layout_of(array.element.ty, &config).unwrap();
        let LayoutShape::Record(record) = record.shape else {
            panic!("va_list element must be a record")
        };
        assert_eq!(
            record
                .fields
                .iter()
                .map(|field| field.offset)
                .collect::<Vec<_>>(),
            vec![0, 4, 8, 16]
        );
    }

    #[test]
    fn target_va_list_representation_follows_each_enabled_abi() {
        let mut aapcs_types = TypeStore::default();
        let aapcs = EffectiveCompilationConfig::aarch64_unknown_linux_gnu();
        let aapcs_va_list = aapcs_types
            .target_builtin(TargetBuiltinType::VaList, &aapcs)
            .unwrap();
        let TypeKind::Array(array) = aapcs_types.kind(aapcs_va_list) else {
            panic!("AAPCS64 va_list must retain array parameter adjustment")
        };
        assert_eq!(array.length, ArrayLength::Constant(1));
        let layout = aapcs_types.layout_of(aapcs_va_list, &aapcs).unwrap();
        assert_eq!((layout.size, layout.align), (32, 8));
        let element = aapcs_types.layout_of(array.element.ty, &aapcs).unwrap();
        let LayoutShape::Record(record) = element.shape else {
            panic!("AAPCS64 va_list element must be a record")
        };
        assert_eq!(
            record
                .fields
                .iter()
                .map(|field| field.offset)
                .collect::<Vec<_>>(),
            vec![0, 8, 16, 24, 28]
        );

        for config in [
            EffectiveCompilationConfig::riscv64_unknown_linux_gnu(),
            EffectiveCompilationConfig::aarch64_apple_darwin(),
        ] {
            let mut types = TypeStore::default();
            let va_list = types
                .target_builtin(TargetBuiltinType::VaList, &config)
                .unwrap();
            assert!(matches!(types.kind(va_list), TypeKind::Pointer(_)));
            assert_eq!(types.layout_of(va_list, &config).unwrap().size, 8);
        }
    }
}
