use ccc_target::{EffectiveCompilationConfig, PackingPolicy};
use ccc_types::{
    ArrayLength, ArrayType, BitfieldLayout, BuiltinType, Enumerator, Field, FunctionType,
    LayoutError, LayoutShape, QualifiedType, RecordKind, TypeId, TypeLayout, TypeQualifiers,
    TypeStore,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SizeAlign {
    size: u64,
    align: u64,
}

trait TypeLayoutExt {
    fn size_align(&self) -> SizeAlign;
}

impl TypeLayoutExt for TypeLayout {
    fn size_align(&self) -> SizeAlign {
        SizeAlign {
            size: self.size,
            align: self.align,
        }
    }
}

fn config() -> EffectiveCompilationConfig {
    EffectiveCompilationConfig::default()
}

fn record_fields(types: &TypeStore, ty: TypeId) -> Vec<(u64, u64, u64, Option<BitfieldLayout>)> {
    let layout = types.layout_of(ty, &config()).unwrap();
    let LayoutShape::Record(record) = layout.shape else {
        panic!("expected a record layout");
    };
    record
        .fields
        .into_iter()
        .map(|field| (field.offset, field.size, field.align, field.bitfield))
        .collect()
}

#[test]
fn x86_64_builtin_and_pointer_layouts_are_explicit() {
    let mut types = TypeStore::default();
    for (builtin, expected) in [
        (BuiltinType::Bool, SizeAlign { size: 1, align: 1 }),
        (BuiltinType::Char, SizeAlign { size: 1, align: 1 }),
        (BuiltinType::SignedChar, SizeAlign { size: 1, align: 1 }),
        (BuiltinType::UnsignedChar, SizeAlign { size: 1, align: 1 }),
        (BuiltinType::Short, SizeAlign { size: 2, align: 2 }),
        (BuiltinType::UnsignedShort, SizeAlign { size: 2, align: 2 }),
        (BuiltinType::Int, SizeAlign { size: 4, align: 4 }),
        (BuiltinType::UnsignedInt, SizeAlign { size: 4, align: 4 }),
        (BuiltinType::Long, SizeAlign { size: 8, align: 8 }),
        (BuiltinType::UnsignedLong, SizeAlign { size: 8, align: 8 }),
        (BuiltinType::LongLong, SizeAlign { size: 8, align: 8 }),
        (
            BuiltinType::UnsignedLongLong,
            SizeAlign { size: 8, align: 8 },
        ),
        (BuiltinType::Float, SizeAlign { size: 4, align: 4 }),
        (BuiltinType::Double, SizeAlign { size: 8, align: 8 }),
        (
            BuiltinType::LongDouble,
            SizeAlign {
                size: 16,
                align: 16,
            },
        ),
    ] {
        assert_eq!(
            types
                .layout_of(types.builtin(builtin), &config())
                .unwrap()
                .size_align(),
            expected,
            "{builtin:?}"
        );
    }

    let (_, incomplete) = types.declare_record(RecordKind::Struct, Some("opaque".to_owned()));
    let pointer = types.pointer(QualifiedType::new(incomplete, TypeQualifiers::CONST));
    assert_eq!(
        types.layout_of(pointer, &config()).unwrap().size_align(),
        SizeAlign { size: 8, align: 8 }
    );
}

#[test]
fn compiler_128_bit_integer_layout_is_stable_for_every_enabled_target() {
    for config in [
        EffectiveCompilationConfig::default(),
        EffectiveCompilationConfig::aarch64_unknown_linux_gnu(),
        EffectiveCompilationConfig::riscv64_unknown_linux_gnu(),
        EffectiveCompilationConfig::aarch64_apple_darwin(),
    ] {
        let types = TypeStore::default();
        for builtin in [BuiltinType::Int128, BuiltinType::UnsignedInt128] {
            let layout = types.layout_of(types.builtin(builtin), &config).unwrap();
            assert_eq!(layout.size, 16, "{} {builtin:?}", config.target.triple);
            assert_eq!(layout.align, 16, "{} {builtin:?}", config.target.triple);
        }
    }
}

#[test]
fn arrays_report_static_incomplete_and_runtime_shapes() {
    let mut types = TypeStore::default();
    let static_array = types.array(ArrayType {
        element: TypeId::INT.into(),
        length: ArrayLength::Constant(7),
    });
    let layout = types.layout_of(static_array, &config()).unwrap();
    assert_eq!(layout.size_align(), SizeAlign { size: 28, align: 4 });
    assert_eq!(
        layout.shape,
        LayoutShape::Array {
            length: 7,
            stride: 4
        }
    );

    let incomplete = types.array(ArrayType {
        element: TypeId::INT.into(),
        length: ArrayLength::Incomplete,
    });
    assert_eq!(
        types.layout_of(incomplete, &config()),
        Err(LayoutError::IncompleteArray(incomplete))
    );

    let bound = types.fresh_variable_length();
    let variable = types.array(ArrayType {
        element: TypeId::INT.into(),
        length: ArrayLength::Variable(bound),
    });
    assert_eq!(
        types.layout_of(variable, &config()),
        Err(LayoutError::VariableLengthArray {
            ty: variable,
            bound
        })
    );
}

#[test]
fn nominal_records_layout_fields_unions_and_packing() {
    let mut types = TypeStore::default();
    let (first_id, first) = types.declare_record(RecordKind::Struct, Some("sample".to_owned()));
    let (_, second) = types.declare_record(RecordKind::Struct, Some("sample".to_owned()));
    assert_ne!(first, second, "tag spelling must not provide type identity");
    types
        .complete_record(
            first_id,
            vec![
                Field::named("head", TypeId::CHAR),
                Field::named("value", TypeId::INT),
                Field::named("tail", TypeId::CHAR),
            ],
        )
        .unwrap();
    assert_eq!(
        types.layout_of(first, &config()).unwrap().size_align(),
        SizeAlign { size: 12, align: 4 }
    );
    assert_eq!(
        record_fields(&types, first)
            .into_iter()
            .map(|field| field.0)
            .collect::<Vec<_>>(),
        [0, 4, 8]
    );

    let (union_id, union) = types.declare_record(RecordKind::Union, None);
    types
        .complete_record(
            union_id,
            vec![
                Field::named("byte", TypeId::CHAR),
                Field::named("integer", TypeId::INT),
                Field::named("wide", TypeId::LONG),
            ],
        )
        .unwrap();
    assert_eq!(
        types.layout_of(union, &config()).unwrap().size_align(),
        SizeAlign { size: 8, align: 8 }
    );
    assert!(
        record_fields(&types, union)
            .iter()
            .all(|field| field.0 == 0)
    );

    let (packed_id, packed) = types.declare_record(RecordKind::Struct, None);
    types
        .complete_record_with_packing(
            packed_id,
            vec![
                Field::named("byte", TypeId::CHAR),
                Field::named("wide", TypeId::LONG),
            ],
            PackingPolicy::PACKED,
        )
        .unwrap();
    assert_eq!(
        types.layout_of(packed, &config()).unwrap().size_align(),
        SizeAlign { size: 9, align: 1 }
    );
    assert_eq!(record_fields(&types, packed)[1].0, 1);
}

#[test]
fn x86_64_bitfields_observe_units_zero_width_and_anonymous_fields() {
    let mut types = TypeStore::default();
    let (record_id, record) = types.declare_record(RecordKind::Struct, None);
    types
        .complete_record(
            record_id,
            vec![
                Field::bitfield(Some("low".to_owned()), TypeId::UNSIGNED_INT, 3),
                Field::bitfield(None, TypeId::UNSIGNED_INT, 0),
                Field::bitfield(Some("high".to_owned()), TypeId::UNSIGNED_INT, 5),
            ],
        )
        .unwrap();
    assert_eq!(
        types.layout_of(record, &config()).unwrap().size_align(),
        SizeAlign { size: 8, align: 4 }
    );
    let fields = record_fields(&types, record);
    assert_eq!(
        fields
            .iter()
            .map(|field| (field.0, field.3.unwrap().bit_offset))
            .collect::<Vec<_>>(),
        [(0, 0), (4, 0), (4, 0)]
    );

    let (anonymous_id, anonymous) = types.declare_record(RecordKind::Struct, None);
    types
        .complete_record(
            anonymous_id,
            vec![
                Field::bitfield(None, TypeId::UNSIGNED_INT, 3),
                Field::bitfield(Some("named".to_owned()), TypeId::UNSIGNED_INT, 5),
            ],
        )
        .unwrap();
    let fields = record_fields(&types, anonymous);
    assert_eq!(
        fields
            .iter()
            .map(|field| (field.0, field.3.unwrap().bit_offset))
            .collect::<Vec<_>>(),
        [(0, 0), (0, 3)]
    );

    let (straddle_id, straddle) = types.declare_record(RecordKind::Struct, None);
    types
        .complete_record(
            straddle_id,
            vec![
                Field::bitfield(Some("left".to_owned()), TypeId::UNSIGNED_INT, 31),
                Field::bitfield(Some("right".to_owned()), TypeId::UNSIGNED_INT, 2),
            ],
        )
        .unwrap();
    assert_eq!(
        record_fields(&types, straddle)
            .iter()
            .map(|field| field.0)
            .collect::<Vec<_>>(),
        [0, 4]
    );

    let (plain_id, plain) = types.declare_record(RecordKind::Struct, None);
    types
        .complete_record(
            plain_id,
            vec![
                Field::bitfield(Some("plain".to_owned()), TypeId::INT, 3),
                Field::named("tail", TypeId::CHAR),
            ],
        )
        .unwrap();
    assert_eq!(record_fields(&types, plain)[1].0, 1);
    assert_eq!(
        types.layout_of(plain, &config()).unwrap().size_align(),
        SizeAlign { size: 4, align: 4 }
    );
}

#[test]
fn x86_64_bitfields_continue_after_a_byte_member() {
    let mut types = TypeStore::default();
    let (record_id, record) = types.declare_record(RecordKind::Struct, None);
    types
        .complete_record(
            record_id,
            vec![
                Field::named("a", TypeId::CHAR),
                Field::bitfield(Some("b".to_owned()), TypeId::INT, 20),
                Field::bitfield(Some("c".to_owned()), TypeId::INT, 20),
            ],
        )
        .unwrap();

    assert_eq!(
        types.layout_of(record, &config()).unwrap().size_align(),
        SizeAlign { size: 8, align: 4 }
    );
    assert_eq!(
        record_fields(&types, record),
        [
            (0, 1, 1, None),
            (
                0,
                4,
                4,
                Some(BitfieldLayout {
                    storage_offset: 0,
                    storage_size: 4,
                    storage_align: 4,
                    bit_offset: 8,
                    width: 20,
                }),
            ),
            (
                4,
                4,
                4,
                Some(BitfieldLayout {
                    storage_offset: 4,
                    storage_size: 4,
                    storage_align: 4,
                    bit_offset: 0,
                    width: 20,
                }),
            ),
        ]
    );
}

#[test]
fn x86_64_bitfields_coalesce_mixed_declared_types() {
    let mut types = TypeStore::default();
    let (record_id, record) = types.declare_record(RecordKind::Struct, None);
    types
        .complete_record(
            record_id,
            vec![
                Field::bitfield(Some("byte".to_owned()), TypeId::UNSIGNED_CHAR, 3),
                Field::bitfield(Some("half".to_owned()), TypeId::UNSIGNED_SHORT, 9),
                Field::bitfield(Some("word".to_owned()), TypeId::UNSIGNED_INT, 17),
                Field::bitfield(Some("wide".to_owned()), TypeId::UNSIGNED_LONG, 33),
            ],
        )
        .unwrap();

    assert_eq!(
        types.layout_of(record, &config()).unwrap().size_align(),
        SizeAlign { size: 8, align: 8 }
    );
    assert_eq!(
        record_fields(&types, record)
            .into_iter()
            .map(|field| {
                let bitfield = field.3.unwrap();
                (
                    bitfield.storage_offset,
                    bitfield.storage_size,
                    bitfield.bit_offset,
                    bitfield.width,
                )
            })
            .collect::<Vec<_>>(),
        [(0, 1, 0, 3), (0, 2, 3, 9), (0, 4, 12, 17), (0, 8, 29, 33)]
    );
}

#[test]
fn zero_width_bitfields_align_the_cursor_without_aligning_the_record() {
    let mut types = TypeStore::default();
    let (record_id, record) = types.declare_record(RecordKind::Struct, None);
    types
        .complete_record(
            record_id,
            vec![
                Field::named("prefix", TypeId::CHAR),
                Field::bitfield(None, TypeId::UNSIGNED_INT, 0),
                Field::named("suffix", TypeId::CHAR),
            ],
        )
        .unwrap();

    assert_eq!(
        types.layout_of(record, &config()).unwrap().size_align(),
        SizeAlign { size: 5, align: 1 }
    );
    assert_eq!(record_fields(&types, record)[2].0, 4);
}

#[test]
fn packed_bitfields_are_contiguous_but_zero_width_barriers_remain_natural() {
    let mut types = TypeStore::default();
    let (packed_id, packed) = types.declare_record(RecordKind::Struct, None);
    types
        .complete_record_with_packing(
            packed_id,
            vec![
                Field::named("prefix", TypeId::CHAR),
                Field::bitfield(Some("low".to_owned()), TypeId::UNSIGNED_INT, 20),
                Field::bitfield(Some("high".to_owned()), TypeId::UNSIGNED_INT, 20),
                Field::named("suffix", TypeId::CHAR),
            ],
            PackingPolicy::PACKED,
        )
        .unwrap();
    assert_eq!(
        types.layout_of(packed, &config()).unwrap().size_align(),
        SizeAlign { size: 7, align: 1 }
    );
    let fields = record_fields(&types, packed);
    assert_eq!(fields[1].3.unwrap().storage_offset, 1);
    assert_eq!(fields[1].3.unwrap().bit_offset, 0);
    assert_eq!(fields[2].3.unwrap().storage_offset, 3);
    assert_eq!(fields[2].3.unwrap().bit_offset, 4);
    assert_eq!(fields[3].0, 6);

    let (barrier_id, barrier) = types.declare_record(RecordKind::Struct, None);
    types
        .complete_record_with_packing(
            barrier_id,
            vec![
                Field::named("prefix", TypeId::CHAR),
                Field::bitfield(Some("low".to_owned()), TypeId::UNSIGNED_INT, 3),
                Field::bitfield(None, TypeId::UNSIGNED_INT, 0),
                Field::bitfield(Some("high".to_owned()), TypeId::UNSIGNED_INT, 3),
                Field::named("suffix", TypeId::CHAR),
            ],
            PackingPolicy::PACKED,
        )
        .unwrap();
    assert_eq!(
        types.layout_of(barrier, &config()).unwrap().size_align(),
        SizeAlign { size: 6, align: 1 }
    );
    let fields = record_fields(&types, barrier);
    assert_eq!(fields[2].0, 4);
    assert_eq!(fields[3].3.unwrap().storage_offset, 4);
    assert_eq!(fields[4].0, 5);
}

#[test]
fn bitfields_reject_invalid_widths_and_types() {
    let mut types = TypeStore::default();
    let (named_zero_id, named_zero) = types.declare_record(RecordKind::Struct, None);
    types
        .complete_record(
            named_zero_id,
            vec![Field::bitfield(Some("zero".to_owned()), TypeId::INT, 0)],
        )
        .unwrap();
    assert_eq!(
        types.layout_of(named_zero, &config()),
        Err(LayoutError::NamedZeroWidthBitfield {
            record: named_zero_id,
            field: 0
        })
    );

    let (boolean_id, boolean) = types.declare_record(RecordKind::Struct, None);
    types
        .complete_record(
            boolean_id,
            vec![Field::bitfield(Some("boolean".to_owned()), TypeId::BOOL, 2)],
        )
        .unwrap();
    assert_eq!(
        types.layout_of(boolean, &config()),
        Err(LayoutError::BitfieldTooWide {
            record: boolean_id,
            field: 0,
            width: 2,
            maximum: 1
        })
    );

    let (floating_id, floating) = types.declare_record(RecordKind::Struct, None);
    types
        .complete_record(
            floating_id,
            vec![Field::bitfield(
                Some("floating".to_owned()),
                TypeId::FLOAT,
                1,
            )],
        )
        .unwrap();
    assert_eq!(
        types.layout_of(floating, &config()),
        Err(LayoutError::NonIntegerBitfield {
            record: floating_id,
            field: 0,
            ty: TypeId::FLOAT
        })
    );
}

#[test]
fn enums_flexible_arrays_and_recursive_records_are_structured() {
    let mut types = TypeStore::default();
    let (enum_id, enumeration) = types.declare_enum(Some("answer".to_owned()));
    assert_eq!(
        types.layout_of(enumeration, &config()),
        Err(LayoutError::IncompleteEnum(enum_id))
    );
    types
        .complete_enum(
            enum_id,
            TypeId::UNSIGNED_INT,
            vec![Enumerator {
                name: "answer_value".to_owned(),
                value: 42,
            }],
        )
        .unwrap();
    assert_eq!(
        types
            .layout_of(enumeration, &config())
            .unwrap()
            .size_align(),
        SizeAlign { size: 4, align: 4 }
    );

    let flexible_array = types.array(ArrayType {
        element: TypeId::CHAR.into(),
        length: ArrayLength::Incomplete,
    });
    let (flexible_id, flexible) = types.declare_record(RecordKind::Struct, None);
    types
        .complete_record(
            flexible_id,
            vec![
                Field::named("length", TypeId::INT),
                Field::named("bytes", flexible_array),
            ],
        )
        .unwrap();
    assert_eq!(
        types.layout_of(flexible, &config()).unwrap().size_align(),
        SizeAlign { size: 4, align: 4 }
    );
    assert_eq!(record_fields(&types, flexible)[1], (4, 0, 1, None));

    let (recursive_id, recursive) = types.declare_record(RecordKind::Struct, None);
    types
        .complete_record(recursive_id, vec![Field::named("self", recursive)])
        .unwrap();
    assert_eq!(
        types.layout_of(recursive, &config()),
        Err(LayoutError::RecursiveType(recursive))
    );

    let pointer = types.pointer(recursive);
    let (linked_id, linked) = types.declare_record(RecordKind::Struct, None);
    types
        .complete_record(linked_id, vec![Field::named("next", pointer)])
        .unwrap();
    assert_eq!(
        types.layout_of(linked, &config()).unwrap().size_align(),
        SizeAlign { size: 8, align: 8 }
    );
}

#[test]
fn member_alignment_requests_shape_struct_union_and_flexible_array_layouts() {
    let mut types = TypeStore::default();
    let values = types.array(ArrayType {
        element: TypeId::LONG.into(),
        length: ArrayLength::Constant(8),
    });
    let (record_id, record) = types.declare_record(RecordKind::Struct, None);
    types
        .complete_record(
            record_id,
            vec![
                Field::named("tag", TypeId::CHAR),
                Field::named("values", values).with_requested_alignment(Some(64)),
                Field::named("tail", TypeId::CHAR),
            ],
        )
        .unwrap();
    assert_eq!(
        types.layout_of(record, &config()).unwrap().size_align(),
        SizeAlign {
            size: 192,
            align: 64
        }
    );
    assert_eq!(
        record_fields(&types, record)
            .into_iter()
            .map(|(offset, size, align, _)| (offset, size, align))
            .collect::<Vec<_>>(),
        vec![(0, 1, 1), (64, 64, 64), (128, 1, 1)]
    );

    let (union_id, union) = types.declare_record(RecordKind::Union, None);
    types
        .complete_record(
            union_id,
            vec![
                Field::named("byte", TypeId::CHAR),
                Field::named("word", TypeId::LONG).with_requested_alignment(Some(64)),
            ],
        )
        .unwrap();
    assert_eq!(
        types.layout_of(union, &config()).unwrap().size_align(),
        SizeAlign {
            size: 64,
            align: 64
        }
    );

    let flexible = types.array(ArrayType {
        element: TypeId::CHAR.into(),
        length: ArrayLength::Incomplete,
    });
    let (flexible_id, flexible_record) = types.declare_record(RecordKind::Struct, None);
    types
        .complete_record(
            flexible_id,
            vec![
                Field::named("length", TypeId::INT),
                Field::named("bytes", flexible).with_requested_alignment(Some(64)),
            ],
        )
        .unwrap();
    assert_eq!(
        types
            .layout_of(flexible_record, &config())
            .unwrap()
            .size_align(),
        SizeAlign {
            size: 64,
            align: 64
        }
    );
    assert_eq!(record_fields(&types, flexible_record)[1].0, 64);
}

#[test]
fn function_types_remain_unsized() {
    let mut types = TypeStore::default();
    let function = types.function_type(FunctionType::unspecified(TypeId::INT));
    assert_eq!(
        types.layout_of(function, &config()),
        Err(LayoutError::UnsizedType(function))
    );
}
