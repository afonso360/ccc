use ccc_pp::{PpItem, lex};
use ccc_sema::generic::analyze_frontend;
use ccc_session::SourceMap;
use ccc_syntax::frontend as syntax;
use ccc_target::EffectiveCompilationConfig;

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
fn dumps_explicit_places_compound_updates_and_volatile_effects_exactly() {
    let module = lower_source(
        "volatile int g;\n\
         int f(int *p) { *p += 2; g = *p; return g; }",
    );
    assert_eq!(
        dump_frontend_ir(&module),
        concat!(
            "data d0 @g : volatile int [file:g0 linkage=External duration=Static visibility=Default definition=TentativeCommon]\n",
            "function f0 @f(v0 %p: pointer to int -> ssa) -> int [signature=int (pointer to int) linkage=External inline=false noreturn=false] {\n",
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
            "function f0 @f(v0 %x: int -> ssa) -> int [signature=int (int) linkage=External inline=false noreturn=false] {\n",
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
    let copy = module.functions[0]
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
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
    assert!(copy.1.qualifiers.contains(ccc_types::TypeQualifiers::CONST));
    assert!(
        copy.1
            .qualifiers
            .contains(ccc_types::TypeQualifiers::VOLATILE)
    );
    assert!(!copy.2.volatile);
    assert!(copy.3.volatile);
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
                FullInstructionKind::AggregateValue { access, .. } if access.volatile
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
fn reports_unsupported_atomic_updates_and_aggregate_calls_explicitly() {
    let atomic = typed_source("_Atomic int value; int f(void) { return value += 1; }");
    let error = lower_frontend(&atomic).unwrap_err();
    assert_eq!(error.code, "CCC3101");
    assert!(error.message.contains("atomic compound"), "{error}");

    let aggregate = typed_source(
        "struct Pair { int x; int y; };\n\
         int consume(struct Pair);\n\
         int f(void) { struct Pair value = {1, 2}; return consume(value); }",
    );
    let error = lower_frontend(&aggregate).unwrap_err();
    assert_eq!(error.code, "CCC3101");
    assert!(error.message.contains("aggregate call"), "{error}");
}
