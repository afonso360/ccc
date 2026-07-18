use ccc_pp::{PpItem, lex};
use ccc_sema::generic::{
    ConstantValue, FullTypedBlockItem, FullTypedInitializer, FullTypedInitializerKind,
    FullTypedStatementKind, analyze_frontend,
};
use ccc_session::SourceMap;
use ccc_syntax::frontend as syntax;
use ccc_target::EffectiveCompilationConfig;
use ccc_types::{ArrayLength, ArrayType, QualifiedType, RecordKind, TypeId};

use super::*;

fn typed_source(source: &str) -> ccc_sema::generic::FullTypedTranslationUnit {
    let mut sources = SourceMap::new();
    let file = sources.add_file("generic-ir-test.c", source);
    let tokens = lex(file, sources.source(file).unwrap()).unwrap();
    let items = syntax::convert_pp_items(tokens.into_iter().map(PpItem::Token)).unwrap();
    let parsed = syntax::parse(&items).unwrap();
    analyze_frontend(&parsed, &EffectiveCompilationConfig::default()).unwrap()
}

fn lower_source(source: &str) -> FullModule {
    lower_frontend(&typed_source(source)).unwrap()
}

#[test]
fn rejects_unlowered_variable_bound_effects() {
    for source in [
        "int f(int n, int (*value)[n]) { return 0; }",
        "int f(int n) { static int (*value)[n++]; return 0; }",
        "int f(int n) { int (*value)[(n++, 4)]; return n; }",
        "int f(int n) { int (*(*value)(void))[n++]; return n; }",
    ] {
        let error = lower_frontend(&typed_source(source)).unwrap_err();
        assert_eq!(error.code, "CCC3101");
        assert!(error.message.contains("bounds are not yet lowered"));
    }
}

#[test]
fn computed_goto_has_exact_dense_table_ir_and_owned_static_tokens() {
    let module = lower_source(
        "int dispatch(int opcode) {\n\
             static const void *const table[2] = {&&zero, &&one};\n\
             goto *table[opcode];\n\
         zero: return 10;\n\
         one: return 20;\n\
         }",
    );
    verify_frontend(&module).unwrap();
    assert_eq!(
        module.globals[0].source,
        DataOrigin::BlockStatic {
            function: ccc_sema::generic::FullFunctionId(0),
            local: ccc_sema::generic::FullLocalId(1),
        }
    );
    assert_eq!(
        dump_frontend_ir(&module),
        concat!(
            "data d0 @__ccc_block_static.dispatch.0.1.table : array[2] of const pointer to const void [block-static:f0:l1 linkage=None duration=Static visibility=Internal definition=Definition]\n",
            "  initializer root=n2 {\n",
            "    n0: const pointer to const void = const unsigned:1\n",
            "    n1: const pointer to const void = const unsigned:2\n",
            "    n2: array[2] of const pointer to const void = aggregate [[0] -> n0, [1] -> n1]\n",
            "  }\n",
            "function f0 @dispatch(v0 %opcode: int -> m0) -> int [signature=int (int) linkage=External visibility=Default inline=false noreturn=false] {\n",
            "  storage m0 l0 %opcode: int [Automatic; IndirectControlFlow]\n",
            "  b0():\n",
            "    i8: v8: int = const signed:10\n",
            "    return v8\n",
            "  b1():\n",
            "    i9: v9: int = const signed:20\n",
            "    return v9\n",
            "  b2(v0: int):\n",
            "    i0: v1: pointer to int = address.storage m0\n",
            "    i1: store v0 -> v1 object=int [plain]\n",
            "    i2: v2: pointer to array[2] of const pointer to const void = address.data d0\n",
            "    i3: v3: pointer to const pointer to const void = convert.array-to-pointer v2 array[2] of const pointer to const void -> pointer to const pointer to const void\n",
            "    i4: v4: pointer to int = address.storage m0\n",
            "    i5: v5: int = load v4 object=int [plain]\n",
            "    i6: v6: pointer to const pointer to const void = pointer.offset v3, v5 element=const pointer to const void\n",
            "    i7: v7: pointer to const void = load v6 object=const pointer to const void [plain]\n",
            "    br_table v7 [b0(), b1()]\n",
            "}\n",
        )
    );
}

#[test]
fn computed_goto_retains_current_local_state_in_memory() {
    let module = lower_source(
        "int f(int limit) {\n\
             void *target = &&step;\n\
             int state = 0;\n\
             goto *target;\n\
         again:\n\
             state += 10;\n\
             goto *target;\n\
         step:\n\
             state++;\n\
             if (state < limit) goto again;\n\
             return state;\n\
         }",
    );
    verify_frontend(&module).unwrap();
    assert_eq!(
        dump_frontend_ir(&module),
        concat!(
            "function f0 @f(v0 %limit: int -> m0) -> int [signature=int (int) linkage=External visibility=Default inline=false noreturn=false] {\n",
            "  storage m0 l0 %limit: int [Automatic; IndirectControlFlow]\n",
            "  storage m1 l1 %target: pointer to void [Automatic; IndirectControlFlow]\n",
            "  storage m2 l2 %state: int [Automatic; IndirectControlFlow]\n",
            "  b0():\n",
            "    i11: v9: pointer to int = address.storage m2\n",
            "    i12: v10: int = load v9 object=int [plain]\n",
            "    i13: v11: int = const signed:10\n",
            "    i14: v12: int = add v10, v11\n",
            "    i15: store v12 -> v9 object=int [plain]\n",
            "    i16: v13: pointer to pointer to void = address.storage m1\n",
            "    i17: v14: pointer to void = load v13 object=pointer to void [plain]\n",
            "    br_table v14 [b0(), b1()]\n",
            "  b1():\n",
            "    i18: v15: pointer to int = address.storage m2\n",
            "    i19: v16: int = load v15 object=int [plain]\n",
            "    i20: v17: int = const signed:1\n",
            "    i21: v18: int = add v16, v17\n",
            "    i22: store v18 -> v15 object=int [plain]\n",
            "    i23: v19: pointer to int = address.storage m2\n",
            "    i24: v20: int = load v19 object=int [plain]\n",
            "    i25: v21: pointer to int = address.storage m0\n",
            "    i26: v22: int = load v21 object=int [plain]\n",
            "    i27: v23: int = less v20, v22\n",
            "    i28: v24: int = convert.to-boolean v23 int -> int\n",
            "    conditional v24 ? b3() : b4()\n",
            "  b2(v0: int):\n",
            "    i0: v1: pointer to int = address.storage m0\n",
            "    i1: store v0 -> v1 object=int [plain]\n",
            "    i2: v2: pointer to pointer to void = address.storage m1\n",
            "    i3: v3: pointer to void = const unsigned:2\n",
            "    i4: v4: pointer to void = convert.pointer-conversion v3 pointer to void -> pointer to void\n",
            "    i5: store v4 -> v2 object=pointer to void [plain]\n",
            "    i6: v5: pointer to int = address.storage m2\n",
            "    i7: v6: int = const signed:0\n",
            "    i8: store v6 -> v5 object=int [plain]\n",
            "    i9: v7: pointer to pointer to void = address.storage m1\n",
            "    i10: v8: pointer to void = load v7 object=pointer to void [plain]\n",
            "    br_table v8 [b0(), b1()]\n",
            "  b3():\n",
            "    branch b0()\n",
            "  b4():\n",
            "    branch b5()\n",
            "  b5():\n",
            "    i29: v25: pointer to int = address.storage m2\n",
            "    i30: v26: int = load v25 object=int [plain]\n",
            "    return v26\n",
            "}\n",
        )
    );
}

#[test]
fn label_addresses_alone_do_not_disable_scalar_promotion() {
    let module = lower_source(
        "int f(int value) { void *token = &&done; done: return value + (token != 0); }",
    );
    assert!(module.functions[0].storage.is_empty());
    assert!(module.functions[0].blocks.iter().all(|block| !matches!(
        block.terminator,
        Some(FullTerminator::IndirectBranch { .. })
    )));
}

#[test]
fn verifier_rejects_invalid_computed_goto_target_tables() {
    let module = lower_source("int f(void) { goto *&&done; done: return 0; }");

    let mut empty = module.clone();
    let terminator = empty.functions[0]
        .blocks
        .iter_mut()
        .find_map(|block| match block.terminator.as_mut() {
            Some(FullTerminator::IndirectBranch { targets, .. }) => Some(targets),
            _ => None,
        })
        .unwrap();
    terminator.clear();
    assert!(
        verify_frontend(&empty)
            .unwrap_err()
            .message
            .contains("no target blocks")
    );

    let mut duplicate = module.clone();
    let terminator = duplicate.functions[0]
        .blocks
        .iter_mut()
        .find_map(|block| match block.terminator.as_mut() {
            Some(FullTerminator::IndirectBranch { targets, .. }) => Some(targets),
            _ => None,
        })
        .unwrap();
    terminator.push(terminator[0].clone());
    assert!(
        verify_frontend(&duplicate)
            .unwrap_err()
            .message
            .contains("duplicate target blocks")
    );

    let mut with_arguments = module.clone();
    let terminator = with_arguments.functions[0]
        .blocks
        .iter_mut()
        .find_map(|block| match block.terminator.as_mut() {
            Some(FullTerminator::IndirectBranch { selector, targets }) => {
                Some((*selector, targets))
            }
            _ => None,
        })
        .unwrap();
    terminator.1[0].arguments.push(terminator.0);
    assert!(
        verify_frontend(&with_arguments)
            .unwrap_err()
            .message
            .contains("must not carry arguments")
    );

    let mut with_parameters = module;
    let (selector, target) = with_parameters.functions[0]
        .blocks
        .iter()
        .find_map(|block| match block.terminator.as_ref() {
            Some(FullTerminator::IndirectBranch { selector, targets }) => {
                Some((*selector, targets[0].target))
            }
            _ => None,
        })
        .unwrap();
    let parameter = ValueId(with_parameters.functions[0].value_types.len() as u32);
    let parameter_type = with_parameters.functions[0].value_types[selector.0 as usize];
    with_parameters.functions[0]
        .value_types
        .push(parameter_type);
    with_parameters.functions[0].blocks[target.0 as usize]
        .parameters
        .push(parameter);
    assert!(
        verify_frontend(&with_parameters)
            .unwrap_err()
            .message
            .contains("must not have parameters")
    );
}

#[test]
fn lowering_rejects_missing_targets_and_cross_function_static_label_tokens() {
    let error = lower_frontend(&typed_source("int f(void *target) { goto *target; }")).unwrap_err();
    assert_eq!(error.code, "CCC3101");
    assert!(error.message.contains("at least one label"));

    let mut unit = typed_source(
        "int first(void) { static void *table[1] = {&&done}; goto *table[0]; done: return 0; }\n\
         int second(void) { return 0; }",
    );
    let FullTypedStatementKind::Compound(items) =
        &mut unit.functions[0].body.as_mut().unwrap().kind
    else {
        panic!("expected compound function body");
    };
    let FullTypedBlockItem::Declaration(declaration) = &mut items[0] else {
        panic!("expected static table declaration");
    };
    let Some(FullTypedInitializer {
        kind: FullTypedInitializerKind::Aggregate(entries),
        ..
    }) = declaration.initializer.as_mut()
    else {
        panic!("expected aggregate initializer");
    };
    let FullTypedInitializerKind::Scalar(expression) = &mut entries[0].initializer.kind else {
        panic!("expected scalar label token");
    };
    expression.constant = Some(ConstantValue::Address(
        ccc_sema::generic::RelocatableAddress {
            base: ccc_sema::generic::RelocatableBase::Label {
                function: ccc_sema::generic::FullFunctionId(1),
                label: ccc_sema::generic::LabelId(0),
            },
            addend: 0,
            one_past: false,
        },
    ));
    let error = lower_frontend(&unit).unwrap_err();
    assert_eq!(error.code, "CCC3101");
    assert!(error.message.contains("cross-function"));
}

#[test]
fn dumps_explicit_places_compound_updates_and_volatile_effects_exactly() {
    let module = lower_source(
        "volatile int g;\n\
         int f(int *p) { *p += 2; g = *p; return g; }",
    );
    assert_eq!(
        dump_frontend_ir(&module),
        concat!(
            "data d0 @g : volatile int [file:g0 linkage=External duration=Static visibility=Default definition=TentativeCommon]\n",
            "function f0 @f(v0 %p: pointer to int -> ssa) -> int [signature=int (pointer to int) linkage=External visibility=Default inline=false noreturn=false] {\n",
            "  b0(v0: pointer to int):\n",
            "    i0: v1: int = load v0 object=int [plain]\n",
            "    i1: v2: int = const signed:2\n",
            "    i2: v3: int = add v1, v2\n",
            "    i3: store v3 -> v0 object=int [plain]\n",
            "    i4: v4: pointer to volatile int = address.data d0\n",
            "    i5: v5: int = load v0 object=int [plain]\n",
            "    i6: store v5 -> v4 object=volatile int [volatile=true atomic=None non-elidable=true non-movable=true]\n",
            "    i7: v6: pointer to volatile int = address.data d0\n",
            "    i8: v7: int = load v6 object=volatile int [volatile=true atomic=None non-elidable=true non-movable=true]\n",
            "    return v7\n",
            "}\n",
        )
    );
}

#[test]
fn pointer_difference_uses_the_unqualified_compatible_element_type() {
    let module =
        lower_source("long difference(const char *cursor, char *start) { return cursor - start; }");
    verify_frontend(&module).unwrap();
    assert_eq!(
        dump_frontend_ir(&module),
        concat!(
            "function f0 @difference(v0 %cursor: pointer to const char -> ssa, v1 %start: pointer to char -> ssa) -> long int [signature=long int (pointer to const char, pointer to char) linkage=External visibility=Default inline=false noreturn=false] {\n",
            "  b0(v0: pointer to const char, v1: pointer to char):\n",
            "    i0: v2: long int = pointer.difference v0, v1 element=char\n",
            "    return v2\n",
            "}\n",
        )
    );
}

#[test]
fn declaration_assembly_labels_are_exact_in_ir_symbols_and_references() {
    let module = lower_source(
        "extern int source_object asm(\"linked_object\");\n\
         extern int source_function(int) asm(\"linked_function\");\n\
         int invoke(int value) { return source_function(value) + source_object; }",
    );
    verify_frontend(&module).unwrap();
    assert_eq!(
        dump_frontend_ir(&module),
        concat!(
            "data d0 @linked_object : int [file:g0 linkage=External duration=Static visibility=Default definition=Declaration]\n",
            "declare f0 @linked_function : int (int) [linkage=External visibility=Default]\n",
            "function f1 @invoke(v0 %value: int -> ssa) -> int [signature=int (int) linkage=External visibility=Default inline=false noreturn=false] {\n",
            "  b0(v0: int):\n",
            "    i0: v1: int = call.direct f0 (v0) signature=int (int) variadic-boundary=1 [read=true write=true unwind=true noreturn=false]\n",
            "    i1: v2: pointer to int = address.data d0\n",
            "    i2: v3: int = load v2 object=int [plain]\n",
            "    i3: v4: int = add v1, v3\n",
            "    return v4\n",
            "}\n",
        )
    );
}

#[test]
fn unprototyped_calls_record_default_promotions_and_a_zero_fixed_boundary_exactly() {
    let module = lower_source(
        "int legacy();\n\
         int invoke(float floating, signed char narrow) { return legacy(floating, narrow); }",
    );
    verify_frontend(&module).unwrap();
    assert_eq!(
        dump_frontend_ir(&module),
        concat!(
            "declare f0 @legacy : int () [linkage=External visibility=Default]\n",
            "function f1 @invoke(v0 %floating: float -> ssa, v1 %narrow: signed char -> ssa) -> int [signature=int (float, signed char) linkage=External visibility=Default inline=false noreturn=false] {\n",
            "  b0(v0: float, v1: signed char):\n",
            "    i0: v2: double = convert.floating-conversion v0 float -> double\n",
            "    i1: v3: int = convert.integer-promotion v1 signed char -> int\n",
            "    i2: v4: int = call.direct f0 (v2, v3) signature=int () variadic-boundary=0 [read=true write=true unwind=true noreturn=false]\n",
            "    return v4\n",
            "}\n",
        )
    );
}

#[test]
fn golden_covers_data_strings_places_and_cfg() {
    let module = lower_source(
        "char exact[2] = \"xy\";\n\
         int f(int x) { return x ? exact[0] : 2; }",
    );
    assert_eq!(
        dump_frontend_ir(&module),
        concat!(
            "data d0 @exact : array[2] of char [file:g0 linkage=External duration=Static visibility=Default definition=Definition]\n",
            "  initializer root=n0 {\n",
            "    n0: array[2] of char = string-data s0 units=2\n",
            "  }\n",
            "string s0 Ordinary : array[3] of char = [120, 121, 0]\n",
            "function f0 @f(v0 %x: int -> ssa) -> int [signature=int (int) linkage=External visibility=Default inline=false noreturn=false] {\n",
            "  b0(v0: int):\n",
            "    i0: v1: int = convert.to-boolean v0 int -> int\n",
            "    conditional v1 ? b1(v0) : b2(v0)\n",
            "  b1(v2: int):\n",
            "    i1: v3: pointer to array[2] of char = address.data d0\n",
            "    i2: v4: pointer to char = convert.array-to-pointer v3 array[2] of char -> pointer to char\n",
            "    i3: v5: int = const signed:0\n",
            "    i4: v6: pointer to char = pointer.offset v4, v5 element=char\n",
            "    i5: v7: char = load v6 object=char [plain]\n",
            "    i6: v8: int = convert.integer-promotion v7 char -> int\n",
            "    branch b3(v8, v2)\n",
            "  b2(v9: int):\n",
            "    i7: v10: int = const signed:2\n",
            "    branch b3(v10, v9)\n",
            "  b3(v11: int, v12: int):\n",
            "    return v11\n",
            "}\n",
        )
    );
}

#[test]
fn predefined_function_names_use_unique_string_storage_and_existing_relocations() {
    let module = lower_source(
        "const char *direct(void) { return __func__; }
         const char *saved(void) {
             static const char *pointer = __func__;
             return pointer;
         }",
    );
    verify_frontend(&module).unwrap();

    assert_eq!(module.strings.len(), 2);
    assert_eq!(
        module.strings[0].code_units,
        b"direct\0"
            .iter()
            .copied()
            .map(u32::from)
            .collect::<Vec<_>>()
    );
    assert_eq!(
        module.strings[1].code_units,
        b"saved\0"
            .iter()
            .copied()
            .map(u32::from)
            .collect::<Vec<_>>()
    );
    for string in &module.strings {
        let ccc_types::TypeKind::Array(array) = module.types.kind(string.ty.ty) else {
            panic!("predefined function name should have array type")
        };
        assert_eq!(array.element.ty, TypeId::CHAR);
        assert!(
            array
                .element
                .qualifiers
                .contains(ccc_types::TypeQualifiers::CONST)
        );
    }

    assert!(module.functions[0].blocks.iter().any(|block| {
        block.instructions.iter().any(|instruction| {
            matches!(
                instruction.kind,
                FullInstructionKind::AddressOfString { string } if string.0 == 0
            )
        })
    }));
    let initializer = module.globals[0].initializer.as_ref().unwrap();
    assert!(matches!(
        initializer.nodes[initializer.root.0 as usize].kind,
        InitializerNodeKind::Relocation {
            target: RelocationTarget::String(string),
            kind: RelocationKind::StringAddress,
            ..
        } if string.0 == 1
    ));
}

#[test]
fn aggregate_rvalues_are_owned_and_project_field_index_paths() {
    let module = lower_source(
        "struct Matrix { int items[2][3]; };\n\
         struct Matrix make(void);\n\
         int read(int choose, int row, int column) {\n\
             return (choose ? make() : make()).items[row][column];\n\
         }",
    );
    verify_frontend(&module).unwrap();
    let dump = dump_frontend_ir(&module);
    assert_eq!(
        dump,
        concat!(
            "declare f0 @make : struct Matrix () [linkage=External visibility=Default]\n",
            "function f1 @read(v0 %choose: int -> ssa, v1 %row: int -> ssa, v2 %column: int -> ssa) -> int [signature=int (int, int, int) linkage=External visibility=Default inline=false noreturn=false] {\n",
            "  b0(v0: int, v1: int, v2: int):\n",
            "    i0: v3: int = convert.to-boolean v0 int -> int\n",
            "    conditional v3 ? b1(v0, v1, v2) : b2(v0, v1, v2)\n",
            "  b1(v4: int, v5: int, v6: int):\n",
            "    i1: v7: struct Matrix = call.direct f0 () signature=struct Matrix () variadic-boundary=0 [read=true write=true unwind=true noreturn=false]\n",
            "    branch b3(v7, v4, v5, v6)\n",
            "  b2(v8: int, v9: int, v10: int):\n",
            "    i2: v11: struct Matrix = call.direct f0 () signature=struct Matrix () variadic-boundary=0 [read=true write=true unwind=true noreturn=false]\n",
            "    branch b3(v11, v8, v9, v10)\n",
            "  b3(v12: struct Matrix, v13: int, v14: int, v15: int):\n",
            "    i3: v16: pointer to int = aggregate.project v12 object=struct Matrix path=field#0:items/index:v14/index:v15\n",
            "    i4: v17: int = load v16 object=int [plain]\n",
            "    return v17\n",
            "}\n",
        )
    );
}

#[test]
fn aggregate_initializer_field_addresses_inherit_record_qualifiers() {
    let module = lower_source(
        "struct Item { const char *name; int value; };\n\
         static int consume(const struct Item *items) { return items[0].value; }\n\
         int const_init(void) {\n\
           const struct Item items[] = {{\"none\", 1}};\n\
           return consume(items);\n\
         }\n\
         int mutable_init(void) {\n\
           struct Item items[] = {{\"full\", 2}};\n\
           return consume(items);\n\
         }",
    );
    verify_frontend(&module).unwrap();

    let projections = module
        .functions
        .iter()
        .filter(|function| matches!(function.name.as_str(), "const_init" | "mutable_init"))
        .flat_map(|function| {
            function.blocks.iter().flat_map(|block| {
                block.instructions.iter().filter_map(|instruction| {
                    let FullInstructionKind::ProjectField {
                        record,
                        field_name: Some(field_name),
                        ..
                    } = &instruction.kind
                    else {
                        return None;
                    };
                    if field_name != "name" {
                        return None;
                    }
                    let result = instruction.result.unwrap();
                    Some((
                        function.name.as_str(),
                        module
                            .types
                            .display(function.value_types[result.0 as usize]),
                        module.types.display_qualified(*record),
                    ))
                })
            })
        })
        .collect::<Vec<_>>();

    assert_eq!(
        projections,
        vec![
            (
                "const_init",
                "pointer to const pointer to const char".to_owned(),
                "const struct Item".to_owned(),
            ),
            (
                "mutable_init",
                "pointer to pointer to const char".to_owned(),
                "struct Item".to_owned(),
            ),
        ]
    );
}

#[test]
fn bitfield_storage_offsets_are_relative_to_the_projected_field() {
    let module = lower_source(
        "struct Bits { unsigned prefix; unsigned value : 5; };\n\
         unsigned update(struct Bits *bits) {\n\
             bits->value = 7;\n\
             return bits->value;\n\
         }",
    );
    verify_frontend(&module).unwrap();
    let dump = dump_frontend_ir(&module);
    assert_eq!(dump.matches("storage=0:4").count(), 2, "{dump}");
    assert!(!dump.contains("storage=4:4"), "{dump}");
}

#[test]
fn aggregate_rvalue_bitfields_use_a_final_projection_anchor() {
    let module = lower_source(
        "struct Inner { unsigned prefix; signed value : 6; };\n\
         struct Outer { struct Inner inner; unsigned tail; };\n\
         struct Outer make(void);\n\
         int read(void) { return make().inner.value; }",
    );
    verify_frontend(&module).unwrap();
    let dump = dump_frontend_ir(&module);
    let relevant = dump
        .lines()
        .filter(|line| line.contains("aggregate.project") || line.contains("bitfield.load"))
        .collect::<Vec<_>>();
    assert_eq!(
        relevant,
        [
            "    i1: v1: pointer to int = aggregate.project v0 object=struct Outer path=field#0:inner/field#1:value:bits(0:0/6)",
            "    i2: v2: int = bitfield.load v1 field=1 storage=0:4 bit=0/6 signed=true [plain]",
        ],
        "{dump}"
    );
}

#[test]
fn verifier_rejects_escaped_and_mismatched_aggregate_bitfield_anchors() {
    fn module() -> FullModule {
        lower_source(
            "struct Bits { unsigned value : 6; };\n\
             struct Bits make(void);\n\
             unsigned read(void) { return make().value; }",
        )
    }

    let mut escaped = module();
    let function = &mut escaped.functions[1];
    let projection = function.blocks[0]
        .instructions
        .iter_mut()
        .find(|instruction| {
            matches!(
                instruction.kind,
                FullInstructionKind::AggregateProject { .. }
            )
        })
        .unwrap();
    let FullInstructionKind::AggregateProject {
        base, projections, ..
    } = &mut projection.kind
    else {
        unreachable!()
    };
    projections.push(AggregateProjection::Index { index: *base });
    let error = verify_frontend(&escaped).unwrap_err();
    assert!(
        error
            .message
            .contains("bitfield must be the final aggregate projection"),
        "{error}"
    );

    let mut mismatched = module();
    let load = mismatched.functions[1].blocks[0]
        .instructions
        .iter_mut()
        .find_map(|instruction| match &mut instruction.kind {
            FullInstructionKind::BitfieldLoad { descriptor, .. } => Some(descriptor),
            _ => None,
        })
        .unwrap();
    load.width -= 1;
    let error = verify_frontend(&mismatched).unwrap_err();
    assert!(
        error
            .message
            .contains("aggregate bitfield projection and load descriptors disagree"),
        "{error}"
    );
}

#[test]
fn verifier_rejects_bitfield_load_result_type_mismatches() {
    let mut module = lower_source(
        "struct Bits { unsigned value : 6; };\n\
         unsigned read(struct Bits *bits) { return bits->value; }",
    );
    let result = module.functions[0].blocks[0]
        .instructions
        .iter()
        .find_map(|instruction| {
            matches!(instruction.kind, FullInstructionKind::BitfieldLoad { .. })
                .then_some(instruction.result.unwrap())
        })
        .unwrap();
    module.functions[0].value_types[result.0 as usize] = TypeId::INT;
    let error = verify_frontend(&module).unwrap_err();
    assert!(
        error
            .message
            .contains("bitfield load result disagrees with its address pointee"),
        "{error}"
    );
}

#[test]
fn lowers_variadic_builtins_to_abi_neutral_effects() {
    let module = lower_source(
        "typedef __builtin_va_list va_list;\n\
         int read(int count, ...) {\n\
             va_list list, copy;\n\
             __builtin_va_start(list, count);\n\
             __builtin_va_copy(copy, list);\n\
             int value = __builtin_va_arg(copy, int);\n\
             __builtin_va_end(copy);\n\
             __builtin_va_end(list);\n\
             return value;\n\
         }",
    );
    verify_frontend(&module).unwrap();
    let dump = dump_frontend_ir(&module);
    assert_eq!(
        dump,
        concat!(
            "function f0 @read(v0 %count: int -> ssa) -> int [signature=int (int, ...) linkage=External visibility=Default inline=false noreturn=false] {\n",
            "  storage m0 l1 %list: array[1] of struct __ccc_sysv_va_list_tag [Automatic; AddressTaken,Aggregate]\n",
            "  storage m1 l2 %copy: array[1] of struct __ccc_sysv_va_list_tag [Automatic; AddressTaken,Aggregate]\n",
            "  b0(v0: int):\n",
            "    i0: v1: int = const signed:0\n",
            "    i1: v2: pointer to array[1] of struct __ccc_sysv_va_list_tag = address.storage m0\n",
            "    i2: va.start v2 last=l0\n",
            "    i3: v3: pointer to array[1] of struct __ccc_sysv_va_list_tag = address.storage m1\n",
            "    i4: v4: pointer to array[1] of struct __ccc_sysv_va_list_tag = address.storage m0\n",
            "    i5: va.copy v4 -> v3\n",
            "    i6: v5: pointer to array[1] of struct __ccc_sysv_va_list_tag = address.storage m1\n",
            "    i7: v6: int = va.arg v5 requested=int\n",
            "    i8: v7: pointer to array[1] of struct __ccc_sysv_va_list_tag = address.storage m1\n",
            "    i9: va.end v7\n",
            "    i10: v8: pointer to array[1] of struct __ccc_sysv_va_list_tag = address.storage m0\n",
            "    i11: va.end v8\n",
            "    return v6\n",
            "}\n",
        )
    );
}

#[test]
fn lowers_sync_synchronize_to_a_sequentially_consistent_memory_fence() {
    let module = lower_source("void synchronize(void) { __sync_synchronize(); }");
    verify_frontend(&module).unwrap();
    let function = module
        .functions
        .iter()
        .find(|function| function.name == "synchronize")
        .unwrap();
    let fences = function
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .filter_map(|instruction| match instruction.kind {
            FullInstructionKind::MemoryFence { order } => Some(order),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(fences, [MemoryOrder::SequentiallyConsistent]);
    let dump = dump_frontend_ir(&module);
    assert!(
        dump.contains("memory.fence order=SequentiallyConsistent"),
        "{dump}"
    );
    assert_eq!(
        module.functions.len(),
        1,
        "the builtin must not become a call"
    );
}

#[test]
fn lowers_legacy_sync_operations_to_explicit_sequentially_consistent_atomics() {
    let module = lower_source(
        "int value;\n\
         void *pointer;\n\
         int protected_side_effect(void);\n\
         int update(int delta) {\n\
             int old = __sync_fetch_and_add(&value, delta);\n\
             int now = __sync_add_and_fetch(&value, delta, protected_side_effect());\n\
             int after = __sync_sub_and_fetch(&value, delta, __sync_synchronize);\n\
             int changed = __sync_bool_compare_and_swap(&value, old, now);\n\
             int seen = __sync_val_compare_and_swap(&value, now, after);\n\
             pointer = __sync_lock_test_and_set(&pointer, (void *)0);\n\
             pointer = __sync_add_and_fetch(&pointer, 1);\n\
             return old + now + after + changed + seen;\n\
         }",
    );
    verify_frontend(&module).unwrap();
    let function = module
        .functions
        .iter()
        .find(|function| function.name == "update")
        .unwrap();
    let instructions = function
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .collect::<Vec<_>>();
    let rmw = instructions
        .iter()
        .filter_map(|instruction| match instruction.kind {
            FullInstructionKind::AtomicReadModifyWrite {
                operation,
                return_new,
                order,
                ..
            } => Some((operation, return_new, order)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        rmw,
        [
            (
                AtomicReadModifyWriteOperation::Add,
                false,
                MemoryOrder::SequentiallyConsistent,
            ),
            (
                AtomicReadModifyWriteOperation::Add,
                true,
                MemoryOrder::SequentiallyConsistent,
            ),
            (
                AtomicReadModifyWriteOperation::Subtract,
                true,
                MemoryOrder::SequentiallyConsistent,
            ),
            (
                AtomicReadModifyWriteOperation::Exchange,
                false,
                MemoryOrder::SequentiallyConsistent,
            ),
            (
                AtomicReadModifyWriteOperation::Add,
                true,
                MemoryOrder::SequentiallyConsistent,
            ),
        ]
    );
    let compare_exchange = instructions
        .iter()
        .filter_map(|instruction| match instruction.kind {
            FullInstructionKind::AtomicCompareExchange { order, .. } => Some(order),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        compare_exchange,
        [
            MemoryOrder::SequentiallyConsistent,
            MemoryOrder::SequentiallyConsistent,
        ]
    );
    assert!(instructions.iter().all(|instruction| !matches!(
        instruction.kind,
        FullInstructionKind::DirectCall { .. } | FullInstructionKind::IndirectCall { .. }
    )));
    let dump = dump_frontend_ir(&module);
    assert_eq!(dump.matches("atomic.rmw operation=").count(), 5, "{dump}");
    assert_eq!(dump.matches("return-new=true").count(), 3, "{dump}");
    assert_eq!(dump.matches("atomic.cmpxchg").count(), 2, "{dump}");
    assert_eq!(
        dump.matches("order=SequentiallyConsistent").count(),
        7,
        "{dump}"
    );
}

#[test]
fn verifier_rejects_inconsistent_atomic_rmw_value_types() {
    let mut module = lower_source(
        "int value; int update(int delta) { return __sync_fetch_and_add(&value, delta); }",
    );
    let mut changed = false;
    for block in &mut module.functions[0].blocks {
        for instruction in &mut block.instructions {
            if let FullInstructionKind::AtomicReadModifyWrite {
                address, operand, ..
            } = &mut instruction.kind
            {
                *operand = *address;
                changed = true;
                break;
            }
        }
    }
    assert!(changed);
    let error = verify_frontend(&module).unwrap_err();
    assert!(
        error
            .message
            .contains("atomic RMW operand type disagrees with its object"),
        "{error}"
    );
}

#[test]
fn verifier_rejects_return_new_atomic_exchange() {
    let mut module = lower_source(
        "void *pointer; void *exchange(void *value) {\n\
             return __sync_lock_test_and_set(&pointer, value);\n\
         }",
    );
    let changed = module.functions[0]
        .blocks
        .iter_mut()
        .flat_map(|block| &mut block.instructions)
        .find_map(|instruction| match &mut instruction.kind {
            FullInstructionKind::AtomicReadModifyWrite { return_new, .. } => Some(return_new),
            _ => None,
        })
        .unwrap();
    *changed = true;
    let error = verify_frontend(&module).unwrap_err();
    assert!(
        error
            .message
            .contains("atomic exchange cannot return a derived replacement value"),
        "{error}"
    );
}

#[test]
fn lowers_integer_intrinsics_and_nonfaulting_prefetch_effects_explicitly() {
    let module = lower_source(
        "void *next_address(void);\n\
         int protected_side_effect(void);\n\
         unsigned long swap(unsigned long value) { return __builtin_bswap64(value); }\n\
         int bits(unsigned int word, unsigned long wide, unsigned long long widest) {\n\
             return __builtin_clz(word) + __builtin_clzl(wide) +\n\
                 __builtin_clzll(widest) + __builtin_ctzll(widest) +\n\
                 __builtin_popcount(word) + __builtin_popcountll(widest);\n\
         }\n\
         void hints(void) {\n\
             __builtin_prefetch(\n\
                 next_address(),\n\
                 1 ? 0 : protected_side_effect(),\n\
                 1 ? 3 : protected_side_effect());\n\
             __builtin_prefetch((void *)1, 1, 0);\n\
         }",
    );
    verify_frontend(&module).unwrap();

    let operations = module
        .functions
        .iter()
        .flat_map(|function| &function.blocks)
        .flat_map(|block| &block.instructions)
        .filter_map(|instruction| match instruction.kind {
            FullInstructionKind::IntegerIntrinsic { operation, .. } => Some(operation),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        operations,
        [
            IntegerIntrinsicOperation::ByteSwap64,
            IntegerIntrinsicOperation::CountLeadingZerosInt,
            IntegerIntrinsicOperation::CountLeadingZerosLong,
            IntegerIntrinsicOperation::CountLeadingZerosLongLong,
            IntegerIntrinsicOperation::CountTrailingZerosLongLong,
            IntegerIntrinsicOperation::PopulationCountInt,
            IntegerIntrinsicOperation::PopulationCountLongLong,
        ]
    );

    let hints = module
        .functions
        .iter()
        .find(|function| function.name == "hints")
        .unwrap();
    let instructions = hints
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .collect::<Vec<_>>();
    let prefetches = instructions
        .iter()
        .filter_map(|instruction| match instruction.kind {
            FullInstructionKind::Prefetch {
                write, locality, ..
            } => Some((write, locality)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(prefetches, [(false, 3), (true, 0)]);
    assert_eq!(
        instructions
            .iter()
            .filter(|instruction| matches!(
                instruction.kind,
                FullInstructionKind::DirectCall { .. }
            ))
            .count(),
        1,
        "the prefetch address expression must be evaluated exactly once"
    );

    let dump = dump_frontend_ir(&module);
    for operation in [
        "ByteSwap64",
        "CountLeadingZerosInt",
        "CountLeadingZerosLong",
        "CountLeadingZerosLongLong",
        "CountTrailingZerosLongLong",
        "PopulationCountInt",
        "PopulationCountLongLong",
    ] {
        assert!(
            dump.contains(&format!("integer.intrinsic operation={operation}")),
            "{dump}"
        );
    }
    assert!(dump.contains("prefetch v"), "{dump}");
    assert!(dump.contains("write=false locality=3"), "{dump}");
    assert!(dump.contains("write=true locality=0"), "{dump}");
}

#[test]
fn verifier_rejects_corrupt_integer_intrinsic_and_prefetch_contracts() {
    let mut module = lower_source(
        "int count(unsigned int word, unsigned long long wide) {\n\
             return __builtin_clz(word) + (int)wide;\n\
         }",
    );
    let wrong_operand = module.functions[0].parameters[1].incoming.unwrap();
    let intrinsic = module.functions[0]
        .blocks
        .iter_mut()
        .flat_map(|block| &mut block.instructions)
        .find_map(|instruction| match &mut instruction.kind {
            FullInstructionKind::IntegerIntrinsic { operand, .. } => Some(operand),
            _ => None,
        })
        .unwrap();
    *intrinsic = wrong_operand;
    let error = verify_frontend(&module).unwrap_err();
    assert!(
        error
            .message
            .contains("integer intrinsic operand has the wrong exact type"),
        "{error}"
    );

    let mut module = lower_source("void hint(void *pointer) { __builtin_prefetch(pointer); }");
    let locality = module.functions[0]
        .blocks
        .iter_mut()
        .flat_map(|block| &mut block.instructions)
        .find_map(|instruction| match &mut instruction.kind {
            FullInstructionKind::Prefetch { locality, .. } => Some(locality),
            _ => None,
        })
        .unwrap();
    *locality = 4;
    let error = verify_frontend(&module).unwrap_err();
    assert!(
        error
            .message
            .contains("prefetch locality is outside the supported range"),
        "{error}"
    );
}

#[test]
fn lowers_scalar_builtins_to_value_preserving_ir_without_calls() {
    let module = lower_source(
        "long choose(int value) {\n\
             return __builtin_expect(value, 1 ? 1 : ++value);\n\
         }\n\
         double infinity(void) { return __builtin_huge_val(); }\n\
         float infinityf(void) { return __builtin_inff(); }\n\
         float not_a_number(void) { return __builtin_nanf(\"\"); }",
    );
    verify_frontend(&module).unwrap();
    assert_eq!(
        dump_frontend_ir(&module),
        concat!(
            "function f0 @choose(v0 %value: int -> ssa) -> long int [signature=long int (int) linkage=External visibility=Default inline=false noreturn=false] {\n",
            "  b0(v0: int):\n",
            "    i0: v1: long int = convert.integer-conversion v0 int -> long int\n",
            "    return v1\n",
            "}\n",
            "function f1 @infinity() -> double [signature=double () linkage=External visibility=Default inline=false noreturn=false] {\n",
            "  b0():\n",
            "    i0: v0: double = const float:0x7ff0000000000000\n",
            "    return v0\n",
            "}\n",
            "function f2 @infinityf() -> float [signature=float () linkage=External visibility=Default inline=false noreturn=false] {\n",
            "  b0():\n",
            "    i0: v0: float = const float:0x7ff0000000000000\n",
            "    return v0\n",
            "}\n",
            "function f3 @not_a_number() -> float [signature=float () linkage=External visibility=Default inline=false noreturn=false] {\n",
            "  b0():\n",
            "    i0: v0: float = const float:0x7ff8000000000000\n",
            "    return v0\n",
            "}\n",
        )
    );
}

#[test]
fn lowers_block_scope_compound_literals_at_their_evaluation_point() {
    let module = lower_source(
        "struct Pair { int left; int right; };
         int read(void) {
             struct Pair *pair = &(struct Pair){ .left = 19, .right = 23 };
             return pair->left + pair->right;
         }",
    );
    verify_frontend(&module).unwrap();
    assert_eq!(
        dump_frontend_ir(&module),
        concat!(
            "function f0 @read() -> int [signature=int () linkage=External visibility=Default inline=false noreturn=false] {\n",
            "  storage m0 l1 %<compound-literal-1>: struct Pair [Automatic; AddressTaken,Aggregate]\n",
            "  b0():\n",
            "    i0: v0: pointer to struct Pair = const null\n",
            "    i1: v1: pointer to struct Pair = address.storage m0\n",
            "    i2: initialize.zero v1 object=struct Pair\n",
            "    i3: v2: pointer to int = project.field v1 struct Pair .left#0\n",
            "    i4: v3: int = const signed:19\n",
            "    i5: store v3 -> v2 object=int [plain]\n",
            "    i6: v4: pointer to int = project.field v1 struct Pair .right#1\n",
            "    i7: v5: int = const signed:23\n",
            "    i8: store v5 -> v4 object=int [plain]\n",
            "    i9: v6: pointer to struct Pair = convert.pointer-conversion v1 pointer to struct Pair -> pointer to struct Pair\n",
            "    i10: v7: pointer to int = project.field v6 struct Pair .left#0\n",
            "    i11: v8: int = load v7 object=int [plain]\n",
            "    i12: v9: pointer to int = project.field v6 struct Pair .right#1\n",
            "    i13: v10: int = load v9 object=int [plain]\n",
            "    i14: v11: int = add v8, v10\n",
            "    return v11\n",
            "}\n",
        )
    );
}

#[test]
fn compound_literal_qualification_reaches_initializer_and_later_accesses() {
    let module = lower_source(
        "int read(void) {
             volatile int *value = &(volatile int){ 1 };
             *value = 2;
             return *value;
         }",
    );
    verify_frontend(&module).unwrap();
    let accesses = module.functions[0]
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .filter_map(|instruction| match instruction.kind {
            FullInstructionKind::Load { access, .. }
            | FullInstructionKind::Store { access, .. } => Some(access),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(accesses.len(), 3);
    assert!(accesses.iter().all(|access| access.volatile));
}

#[test]
fn lowers_promoted_anonymous_members_as_each_physical_projection() {
    let module = lower_source(
        "struct Usage {
             int prefix;
             union { struct { long minor; }; long alternate; };
         };
         long read(struct Usage *usage) { return usage->minor; }",
    );
    verify_frontend(&module).unwrap();
    let read = module
        .functions
        .iter()
        .find(|function| function.name == "read")
        .unwrap();
    let projected_names = read
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .filter_map(|instruction| match &instruction.kind {
            FullInstructionKind::ProjectField { field_name, .. } => Some(field_name.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(projected_names, [None, None, Some("minor".to_owned())]);
}

#[test]
fn aggregate_va_arg_results_are_owned_projection_roots() {
    let module = lower_source(
        "typedef __builtin_va_list va_list;\n\
         struct Pair { int values[2]; };\n\
         int read(int count, ...) {\n\
             va_list list;\n\
             __builtin_va_start(list, count);\n\
             return __builtin_va_arg(list, struct Pair).values[1];\n\
         }",
    );
    verify_frontend(&module).unwrap();
    let instructions = module.functions[0]
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .collect::<Vec<_>>();
    let va_arg = instructions
        .iter()
        .find_map(|instruction| {
            matches!(instruction.kind, FullInstructionKind::VaArg { .. })
                .then(|| instruction.result.expect("va_arg instruction result"))
        })
        .expect("aggregate va_arg result");
    assert!(instructions.iter().any(|instruction| matches!(
        &instruction.kind,
        FullInstructionKind::AggregateProject {
            base,
            projections,
            ..
        } if *base == va_arg
            && matches!(projections.as_slice(), [
                AggregateProjection::Field { index: 0, .. },
                AggregateProjection::Index { .. }
            ])
    )));
}

#[test]
fn verifier_rejects_invalid_va_arg_types_independently_of_sema() {
    fn module() -> FullModule {
        lower_source(
            "typedef __builtin_va_list va_list;\n\
             int read(int count, ...) {\n\
                 va_list list;\n\
                 __builtin_va_start(list, count);\n\
                 return __builtin_va_arg(list, int);\n\
             }",
        )
    }

    fn replace_requested(module: &mut FullModule, replacement: QualifiedType) {
        let mut result = None;
        for instruction in module.functions[0]
            .blocks
            .iter_mut()
            .flat_map(|block| &mut block.instructions)
        {
            if let FullInstructionKind::VaArg { requested, .. } = &mut instruction.kind {
                *requested = replacement;
                result = instruction.result;
                break;
            }
        }
        let result = result.expect("va_arg result");
        module.functions[0].value_types[result.0 as usize] = replacement.ty;
    }

    let mut promotion = module();
    replace_requested(&mut promotion, QualifiedType::unqualified(TypeId::FLOAT));
    assert!(
        verify_frontend(&promotion)
            .unwrap_err()
            .message
            .contains("default argument promotions")
    );

    let mut variable = module();
    let bound = variable.types.fresh_variable_length();
    let array = variable.types.array(ArrayType {
        element: QualifiedType::unqualified(TypeId::INT),
        length: ArrayLength::Variable(bound),
    });
    let pointer = variable.types.pointer(array);
    replace_requested(&mut variable, QualifiedType::unqualified(pointer));
    assert!(
        verify_frontend(&variable)
            .unwrap_err()
            .message
            .contains("variably modified")
    );

    let mut incomplete = module();
    let (_, record) = incomplete
        .types
        .declare_record(RecordKind::Struct, Some("Incomplete".to_owned()));
    replace_requested(&mut incomplete, QualifiedType::unqualified(record));
    assert!(
        verify_frontend(&incomplete)
            .unwrap_err()
            .message
            .contains("incomplete")
    );
}

#[test]
fn verifier_accepts_va_arg_with_an_int_compatible_enum() {
    let module = lower_source(
        "enum Choice { CHOICE = 3 };\n\
         typedef __builtin_va_list va_list;\n\
         enum Choice read(enum Choice final, ...) {\n\
             va_list list;\n\
             __builtin_va_start(list, final);\n\
             return __builtin_va_arg(list, enum Choice);\n\
         }",
    );
    verify_frontend(&module).unwrap();
    assert!(
        dump_frontend_ir(&module).contains("requested=enum Choice"),
        "{}",
        dump_frontend_ir(&module)
    );
}

#[test]
fn variadic_promotions_ir_dump_matches_the_committed_golden() {
    let module = lower_source(include_str!(
        "../../../../tests/frontend/typed-ast/variadic-promotions.c"
    ));
    verify_frontend(&module).unwrap();
    assert_eq!(
        dump_frontend_ir(&module),
        include_str!("../../../../tests/frontend/goldens/ir-variadic-promotions.out")
    );
}

#[test]
fn comma_values_ir_dump_matches_the_committed_golden() {
    let module = lower_source(include_str!(
        "../../../../tests/frontend/typed-ast/comma-values.c"
    ));
    verify_frontend(&module).unwrap();
    assert_eq!(
        dump_frontend_ir(&module),
        include_str!("../../../../tests/frontend/goldens/ir-comma-values.out")
    );
}

#[test]
fn lowers_consecutive_equal_array_elements_to_one_repeated_fragment() {
    let module = lower_source("int repeated[4] = {7, 7, 7};");
    verify_frontend(&module).unwrap();
    assert_eq!(
        dump_frontend_ir(&module),
        concat!(
            "data d0 @repeated : array[4] of int [file:g0 linkage=External duration=Static visibility=Default definition=Definition]\n",
            "  initializer root=n1 {\n",
            "    n0: int = const signed:7\n",
            "    n1: array[4] of int = repeat n0 count=3\n",
            "  }\n",
        )
    );

    let module = lower_source(
        "int target;\n\
         int *addresses[3] = {&target, &target, &target};",
    );
    verify_frontend(&module).unwrap();
    let graph = module.globals[1].initializer.as_ref().unwrap();
    assert_eq!(graph.nodes.len(), 2);
    assert!(matches!(
        graph.nodes[0].kind,
        InitializerNodeKind::Relocation {
            target: RelocationTarget::Object(DataId(0)),
            addend: 0,
            ..
        }
    ));
    assert_eq!(
        graph.nodes[1].kind,
        InitializerNodeKind::Repeat {
            element: InitializerNodeId(0),
            count: 3,
        }
    );

    let module = lower_source("int zeros[3] = {{}, {}, {}};");
    verify_frontend(&module).unwrap();
    let graph = module.globals[0].initializer.as_ref().unwrap();
    assert_eq!(graph.nodes.len(), 2);
    assert_eq!(graph.nodes[0].kind, InitializerNodeKind::Zero);
    assert_eq!(
        graph.nodes[1].kind,
        InitializerNodeKind::Repeat {
            element: InitializerNodeId(0),
            count: 3,
        }
    );

    let module = lower_source("double signed_zeros[2] = {0.0, -0.0};");
    let graph = module.globals[0].initializer.as_ref().unwrap();
    assert!(matches!(
        graph.nodes[graph.root.0 as usize].kind,
        InitializerNodeKind::Aggregate(_)
    ));
}

#[test]
fn lowers_static_array_addresses_decay_and_mixed_shift_constants() {
    let module = lower_source(
        "int values[4];\n\
         int *selected = &values[2];\n\
         const char text[] = \"named\";\n\
         const char *text_pointer = text;\n\
         unsigned long long high_bit = (unsigned long long)1 << 40;",
    );
    verify_frontend(&module).unwrap();

    let selected = module.globals[1].initializer.as_ref().unwrap();
    assert_eq!(
        selected.nodes[selected.root.0 as usize].kind,
        InitializerNodeKind::Relocation {
            target: RelocationTarget::Object(DataId(0)),
            addend: 8,
            one_past: false,
            kind: RelocationKind::ObjectAddress,
        }
    );
    let text_pointer = module.globals[3].initializer.as_ref().unwrap();
    assert_eq!(
        text_pointer.nodes[text_pointer.root.0 as usize].kind,
        InitializerNodeKind::Relocation {
            target: RelocationTarget::Object(DataId(2)),
            addend: 0,
            one_past: false,
            kind: RelocationKind::ObjectAddress,
        }
    );
    let high_bit = module.globals[4].initializer.as_ref().unwrap();
    assert_eq!(
        high_bit.nodes[high_bit.root.0 as usize].kind,
        InitializerNodeKind::Scalar(ScalarConstant::Unsigned(1_u128 << 40))
    );
}

#[test]
fn lowers_block_static_addresses_in_block_static_initializers() {
    let module = lower_source(
        "void *address(void) {\n\
             static int target;\n\
             static void *pointer = &target;\n\
             return pointer;\n\
         }",
    );
    verify_frontend(&module).unwrap();

    assert_eq!(module.globals.len(), 2);
    assert!(matches!(
        module.globals[0].source,
        DataOrigin::BlockStatic { .. }
    ));
    let pointer = module.globals[1].initializer.as_ref().unwrap();
    assert_eq!(
        pointer.nodes[pointer.root.0 as usize].kind,
        InitializerNodeKind::Relocation {
            target: RelocationTarget::Object(DataId(0)),
            addend: 0,
            one_past: false,
            kind: RelocationKind::ObjectAddress,
        }
    );
}

#[test]
fn retains_declaration_signatures_without_definition_parameters() {
    let module = lower_source("int abs(int); int main(void) { return abs(-1); }");
    verify_frontend(&module).unwrap();
    let declaration = &module.functions[0];
    assert!(declaration.entry.is_none());
    assert!(declaration.parameters.is_empty());
    assert!(
        module
            .types
            .function_signature(declaration.signature)
            .is_some()
    );
}

#[test]
fn keeps_qualifiers_on_places_and_uses_unqualified_ssa_value_types() {
    let module = lower_source("int f(const int value) { return value; }");
    let function = &module.functions[0];
    let parameter = &function.parameters[0];
    assert!(
        parameter
            .ty
            .qualifiers
            .contains(ccc_types::TypeQualifiers::CONST)
    );
    assert_eq!(
        function.value_types[parameter.incoming.unwrap().0 as usize],
        ccc_types::TypeId::INT
    );
    assert_eq!(parameter.storage, None);
    assert!(function.storage.is_empty());
}

#[test]
fn const_and_volatile_aggregate_sources_preserve_independent_copy_accesses() {
    let module = lower_source(
        "struct Pair { int left; int right; };\n\
         void copy(struct Pair *destination, const volatile struct Pair *source) {\n\
             *destination = *source;\n\
         }",
    );
    let instructions = module.functions[0]
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .collect::<Vec<_>>();
    let snapshot = instructions
        .iter()
        .find_map(|instruction| match &instruction.kind {
            FullInstructionKind::AggregateSnapshot { object, access, .. } => Some((object, access)),
            _ => None,
        })
        .expect("aggregate snapshot");
    let copy = instructions
        .iter()
        .find_map(|instruction| match &instruction.kind {
            FullInstructionKind::AggregateCopy {
                destination_object,
                source_object,
                destination_access,
                source_access,
                ..
            } => Some((
                destination_object,
                source_object,
                destination_access,
                source_access,
            )),
            _ => None,
        })
        .expect("aggregate copy");
    assert!(copy.0.qualifiers.is_empty());
    assert!(copy.1.qualifiers.is_empty());
    assert!(!copy.2.volatile);
    assert!(!copy.3.volatile);
    assert!(
        snapshot
            .0
            .qualifiers
            .contains(ccc_types::TypeQualifiers::CONST)
    );
    assert!(
        snapshot
            .0
            .qualifiers
            .contains(ccc_types::TypeQualifiers::VOLATILE)
    );
    assert!(snapshot.1.volatile);
}

#[test]
fn discarded_places_lower_and_keep_exact_volatile_read_effects() {
    let module = lower_source(
        "int plain; volatile int observed;\n\
         struct Pair { int left; int right; };\n\
         volatile struct Pair aggregate;\n\
         void consume(int *pointer) { plain; *pointer; observed; aggregate; }",
    );
    let instructions = module.functions[0]
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .collect::<Vec<_>>();
    assert_eq!(
        instructions
            .iter()
            .filter(|instruction| matches!(
                instruction.kind,
                FullInstructionKind::Load { access, .. } if access.volatile
            ))
            .count(),
        1
    );
    assert_eq!(
        instructions
            .iter()
            .filter(|instruction| matches!(
                instruction.kind,
                FullInstructionKind::AggregateSnapshot { access, .. } if access.volatile
            ))
            .count(),
        1
    );
}

#[test]
fn promotes_eligible_scalar_mutation_across_control_flow_to_block_parameters() {
    let module = lower_source(
        "int adjust(int condition) {\n\
             int value = 1;\n\
             if (condition) value += 2;\n\
             return value;\n\
         }",
    );
    let function = &module.functions[0];
    assert!(function.storage.is_empty());
    assert!(
        function
            .parameters
            .iter()
            .all(|parameter| parameter.storage.is_none())
    );
    assert!(
        function
            .blocks
            .iter()
            .any(|block| !block.parameters.is_empty())
    );
    assert!(function.blocks.iter().all(|block| {
        block.instructions.iter().all(|instruction| {
            !matches!(
                instruction.kind,
                FullInstructionKind::AddressOfStorage { .. }
            )
        })
    }));
}

#[test]
fn lowers_pointer_arrays_indirect_calls_and_mixed_shift_promotions() {
    for source in [
        include_str!("../../../../tests/execution/cases/pointers_and_arrays.c"),
        include_str!("../../../../tests/execution/cases/indirect_calls.c"),
        include_str!("../../../../tests/execution/cases/operators_and_conversions.c"),
    ] {
        let module = lower_source(source);
        verify_frontend(&module).unwrap();
    }
}

#[test]
fn emits_static_locals_as_data_and_limits_exact_bound_string_copies() {
    let module = lower_source(
        "char exact[2] = \"xy\";\n\
         int f(void) { static int saved = 3; return saved; }",
    );
    assert_eq!(module.globals.len(), 2);
    assert!(matches!(
        module.globals[1].source,
        DataOrigin::BlockStatic { .. }
    ));
    let graph = module.globals[0].initializer.as_ref().unwrap();
    assert!(matches!(
        graph.nodes[graph.root.0 as usize].kind,
        InitializerNodeKind::StringData {
            copy_code_units: 2,
            ..
        }
    ));
    assert!(module.functions[0].storage.is_empty());
    assert!(module.functions[0].blocks.iter().any(|block| {
        block.instructions.iter().any(|instruction| {
            matches!(
                instruction.kind,
                FullInstructionKind::AddressOfGlobal { global: DataId(1) }
            )
        })
    }));
}

#[test]
fn lowers_braced_string_array_initializers_as_string_copies() {
    let module = lower_source(
        "char output[] = { \"luac\" \".out\" };\n\
         int f(void) { char local[2] = { (\"xy\") }; return local[0]; }",
    );
    verify_frontend(&module).unwrap();

    let graph = module.globals[0].initializer.as_ref().unwrap();
    assert!(matches!(
        graph.nodes[graph.root.0 as usize].kind,
        InitializerNodeKind::StringData {
            copy_code_units: 9,
            ..
        }
    ));
    assert!(module.functions[0].blocks.iter().any(|block| {
        block.instructions.iter().any(|instruction| {
            matches!(
                instruction.kind,
                FullInstructionKind::StringInitialize {
                    copy_code_units: 2,
                    ..
                }
            )
        })
    }));
}

#[test]
fn lowers_every_control_flow_form_to_terminators() {
    let module = lower_source(
        "int f(int x) {\n\
           int total = 0;\n\
         again:\n\
           for (int i = 0; i < 2; i++) total += i;\n\
           do { total++; } while (total < 2);\n\
           while (x--) { if (x == 2) continue; if (x == 1) break; }\n\
           switch (total) { case 2: total += 3; break; default: total = 1; }\n\
           if (total == 0) goto again;\n\
           return total;\n\
         }",
    );
    verify_frontend(&module).unwrap();
    let function = &module.functions[0];
    assert!(
        function
            .blocks
            .iter()
            .any(|block| { matches!(block.terminator, Some(FullTerminator::Switch { .. })) })
    );
    assert!(
        function
            .blocks
            .iter()
            .all(|block| block.terminator.is_some())
    );
}

#[test]
fn verifier_rejects_use_dominance_block_arity_and_ordering_corruption() {
    let mut module = lower_source("int f(int x) { return x ? x + 1 : x + 2; }");
    let function = &mut module.functions[0];
    let instruction = function
        .blocks
        .iter_mut()
        .flat_map(|block| &mut block.instructions)
        .find(|instruction| matches!(instruction.kind, FullInstructionKind::Binary { .. }))
        .unwrap();
    let own_result = instruction.result.unwrap();
    let FullInstructionKind::Binary { left, .. } = &mut instruction.kind else {
        unreachable!()
    };
    *left = own_result;
    let error = verify_frontend(&module).unwrap_err();
    assert!(error.message.contains("does not dominate"), "{error}");

    let mut module = lower_source("int f(int x) { return x ? 1 : 2; }");
    let edge = module.functions[0]
        .blocks
        .iter_mut()
        .filter_map(|block| block.terminator.as_mut())
        .find_map(|terminator| match terminator {
            FullTerminator::Branch(edge) if !edge.arguments.is_empty() => Some(edge),
            _ => None,
        })
        .unwrap();
    edge.arguments.clear();
    let error = verify_frontend(&module).unwrap_err();
    assert!(error.message.contains("wrong arity"), "{error}");

    let mut module = lower_source("volatile int g; int f(void) { return g; }");
    let access = module.functions[0]
        .blocks
        .iter_mut()
        .flat_map(|block| &mut block.instructions)
        .find_map(|instruction| match &mut instruction.kind {
            FullInstructionKind::Load { access, .. } if access.volatile => Some(access),
            _ => None,
        })
        .unwrap();
    access.non_movable = false;
    let error = verify_frontend(&module).unwrap_err();
    assert!(
        error.message.contains("non-elidable/non-movable"),
        "{error}"
    );
}

#[test]
fn verifier_rejects_invalid_initializer_copy_extents() {
    let mut module = lower_source("char exact[2] = \"xy\";");
    let graph = module.globals[0].initializer.as_mut().unwrap();
    let InitializerNodeKind::StringData {
        copy_code_units, ..
    } = &mut graph.nodes[graph.root.0 as usize].kind
    else {
        panic!("expected string-data initializer")
    };
    *copy_code_units = 3;
    let error = verify_frontend(&module).unwrap_err();
    assert!(error.message.contains("copy count"), "{error}");
}

#[test]
fn verifier_rejects_malformed_repeated_initializer_graphs() {
    let repeated = || lower_source("int repeated[3] = {7, 7, 7};");

    let mut module = repeated();
    let graph = module.globals[0].initializer.as_mut().unwrap();
    let InitializerNodeKind::Repeat { count, .. } = &mut graph.nodes[1].kind else {
        panic!("expected repeated initializer")
    };
    *count = 0;
    let error = verify_frontend(&module).unwrap_err();
    assert!(error.message.contains("count is zero"), "{error}");

    let mut module = repeated();
    let graph = module.globals[0].initializer.as_mut().unwrap();
    let InitializerNodeKind::Repeat { count, .. } = &mut graph.nodes[1].kind else {
        panic!("expected repeated initializer")
    };
    *count = 4;
    let error = verify_frontend(&module).unwrap_err();
    assert!(error.message.contains("exceeds its array bound"), "{error}");

    let mut module = repeated();
    let pointer = module.types.pointer(ccc_types::TypeId::INT);
    module.globals[0].initializer.as_mut().unwrap().nodes[0].ty =
        ccc_types::QualifiedType::unqualified(pointer);
    let error = verify_frontend(&module).unwrap_err();
    assert!(error.message.contains("element type"), "{error}");

    let mut module = repeated();
    module.globals[0].ty = ccc_types::QualifiedType::unqualified(ccc_types::TypeId::INT);
    module.globals[0].initializer.as_mut().unwrap().nodes[1].ty =
        ccc_types::QualifiedType::unqualified(ccc_types::TypeId::INT);
    let error = verify_frontend(&module).unwrap_err();
    assert!(
        error.message.contains("does not have array type"),
        "{error}"
    );

    let mut module = repeated();
    let incomplete = module.types.array(ccc_types::ArrayType {
        element: ccc_types::QualifiedType::unqualified(ccc_types::TypeId::INT),
        length: ccc_types::ArrayLength::Incomplete,
    });
    module.globals[0].ty = ccc_types::QualifiedType::unqualified(incomplete);
    module.globals[0].initializer.as_mut().unwrap().nodes[1].ty =
        ccc_types::QualifiedType::unqualified(incomplete);
    let error = verify_frontend(&module).unwrap_err();
    assert!(error.message.contains("constant array bound"), "{error}");

    let mut module = repeated();
    let graph = module.globals[0].initializer.as_mut().unwrap();
    let InitializerNodeKind::Repeat { element, .. } = &mut graph.nodes[1].kind else {
        panic!("expected repeated initializer")
    };
    *element = InitializerNodeId(1);
    let error = verify_frontend(&module).unwrap_err();
    assert!(error.message.contains("child-before-parent"), "{error}");

    let mut module = repeated();
    let graph = module.globals[0].initializer.as_mut().unwrap();
    graph.nodes.push(InitializerNode {
        id: InitializerNodeId(2),
        ty: ccc_types::QualifiedType::unqualified(ccc_types::TypeId::INT),
        kind: InitializerNodeKind::Zero,
    });
    let error = verify_frontend(&module).unwrap_err();
    assert!(error.message.contains("unreachable"), "{error}");
}

#[test]
fn verifier_rejects_edge_types_storage_terminators_and_relocation_targets() {
    let mut module = lower_source("int f(int x) { return x ? 1 : 2; }");
    let pointer = module.types.pointer(ccc_types::TypeId::INT);
    let parameter = module.functions[0]
        .blocks
        .iter()
        .find_map(|block| {
            if block.parameters.is_empty() || block.id == BlockId(0) {
                None
            } else {
                Some(block.parameters[0])
            }
        })
        .unwrap();
    module.functions[0].value_types[parameter.0 as usize] = pointer;
    let error = verify_frontend(&module).unwrap_err();
    assert!(error.message.contains("argument type mismatch"), "{error}");

    let mut module = lower_source("int f(volatile int x) { return x; }");
    let storage = module.functions[0]
        .blocks
        .iter_mut()
        .flat_map(|block| &mut block.instructions)
        .find_map(|instruction| match &mut instruction.kind {
            FullInstructionKind::AddressOfStorage { storage } => Some(storage),
            _ => None,
        })
        .unwrap();
    *storage = StorageId(99);
    let error = verify_frontend(&module).unwrap_err();
    assert!(error.message.contains("unknown storage"), "{error}");

    let mut module = lower_source("int f(void) { return 1; }");
    module.functions[0].blocks[0].terminator = None;
    let error = verify_frontend(&module).unwrap_err();
    assert!(error.message.contains("has no terminator"), "{error}");

    let mut module = lower_source("int target; int *pointer = &target;");
    let graph = module.globals[1].initializer.as_mut().unwrap();
    let InitializerNodeKind::Relocation { target, .. } =
        &mut graph.nodes[graph.root.0 as usize].kind
    else {
        panic!("expected relocation initializer")
    };
    *target = RelocationTarget::Object(DataId(99));
    let error = verify_frontend(&module).unwrap_err();
    assert!(error.message.contains("unknown data"), "{error}");
}

#[test]
fn reports_unsupported_atomic_updates_and_accepts_aggregate_calls() {
    let atomic = typed_source("_Atomic int value; int f(void) { return value += 1; }");
    let error = lower_frontend(&atomic).unwrap_err();
    assert_eq!(error.code, "CCC3101");
    assert!(error.message.contains("atomic compound"), "{error}");

    let aggregate = lower_source(
        "struct Pair { int x; int y; };\n\
         int consume(struct Pair);\n\
         int f(void) { struct Pair value = {1, 2}; return consume(value); }",
    );
    verify_frontend(&aggregate).unwrap();
    assert!(aggregate.functions[1].blocks.iter().any(|block| {
        block
            .instructions
            .iter()
            .any(|instruction| matches!(instruction.kind, FullInstructionKind::DirectCall { .. }))
    }));
}
