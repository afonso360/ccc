use ccc_pp::{
    FsFileProvider, PpItem, PragmaEvent, PreprocessContext, PreprocessOptions, VecDiagnosticSink,
    lex, preprocess,
};
use ccc_session::{Session, SourceMap};
use ccc_syntax::frontend::{self as syntax, ExternalItem};
use ccc_target::{CapabilityKind, CapabilityState, EffectiveCompilationConfig, LanguageMode};
use ccc_types::{ArrayLength, TypeId, TypeKind, TypeQualifiers};

use super::*;

fn parse_source(source: &str) -> syntax::TranslationUnit {
    let mut sources = SourceMap::new();
    let file = sources.add_file("generic-sema-test.c", source);
    let tokens = lex(file, sources.source(file).unwrap()).unwrap();
    let items = syntax::convert_pp_items(tokens.into_iter().map(PpItem::Token)).unwrap();
    syntax::parse(&items).unwrap()
}

fn analyze_source(source: &str) -> Result<FullTypedTranslationUnit, Vec<ccc_diag::Diagnostic>> {
    analyze_frontend(
        &parse_source(source),
        &EffectiveCompilationConfig::default(),
    )
}

fn analyze_source_with_config(
    source: &str,
    config: &EffectiveCompilationConfig,
) -> Result<FullTypedTranslationUnit, Vec<ccc_diag::Diagnostic>> {
    analyze_frontend(&parse_source(source), config)
}

fn analyze_preprocessed_source(
    display_name: &str,
    source: &str,
) -> Result<FullTypedTranslationUnit, Vec<ccc_diag::Diagnostic>> {
    let mut session = Session::new(EffectiveCompilationConfig::default());
    let file = session.sources.add_file(display_name, source);
    let options = PreprocessOptions::default();
    let files = FsFileProvider;
    let mut diagnostics = VecDiagnosticSink::default();
    let output = preprocess(
        &mut PreprocessContext {
            session: &mut session,
            diagnostics: &mut diagnostics,
            options: &options,
            files: &files,
        },
        file,
    );
    assert!(!output.had_errors, "{:#?}", diagnostics.diagnostics);
    let items = syntax::convert_pp_items(output.items).unwrap();
    let parsed = syntax::parse(&items).unwrap();
    analyze_frontend(&parsed, &session.config)
}

fn analyze_resource_source(
    source: &str,
) -> Result<FullTypedTranslationUnit, Vec<ccc_diag::Diagnostic>> {
    let resource_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("resource-dir");
    let mut session =
        Session::new(EffectiveCompilationConfig::default().with_resource_dir(resource_dir));
    let file = session.sources.add_file("resource-test.c", source);
    let options = PreprocessOptions::default();
    let files = FsFileProvider;
    let mut diagnostics = VecDiagnosticSink::default();
    let output = preprocess(
        &mut PreprocessContext {
            session: &mut session,
            diagnostics: &mut diagnostics,
            options: &options,
            files: &files,
        },
        file,
    );
    assert!(!output.had_errors, "{:#?}", diagnostics.diagnostics);
    let items = syntax::convert_pp_items(output.items).unwrap();
    let parsed = syntax::parse(&items).unwrap();
    analyze_frontend(&parsed, &session.config)
}

#[test]
fn zero_length_arrays_are_limited_to_gnu_language_mode() {
    let unit = analyze_source("struct FileHandle { unsigned char bytes[0]; };").unwrap();
    let FullTypedExternalItem::TypeDeclaration { ty, .. } = unit.external_items[0] else {
        panic!("expected a type declaration");
    };
    let TypeKind::Record(record) = unit.types.kind(ty) else {
        panic!("expected a record declaration");
    };
    let definition = unit.types.record(*record).unwrap();
    let fields = definition.fields.as_ref().unwrap();
    let TypeKind::Array(array) = unit.types.kind(fields[0].ty.ty) else {
        panic!("expected an array field");
    };
    assert_eq!(array.length, ArrayLength::Constant(0));

    let diagnostics = analyze_source_with_config(
        "struct FileHandle { unsigned char bytes[0]; };",
        &EffectiveCompilationConfig::default().with_language_mode(LanguageMode::C11),
    )
    .unwrap_err();
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "CCC2223")
    );
}

#[test]
fn types_block_scope_compound_literals_as_initialized_addressable_lvalues() {
    let unit = analyze_source(
        "struct Pair { int left; int right; };
         int read(void) {
             struct Pair pair = (struct Pair){ .left = 19, .right = 23 };
             return pair.left + pair.right;
         }
         enum State { __attribute__((deprecated)) old_state = 1, current_state = 2 };",
    )
    .unwrap();
    let function = unit
        .functions
        .iter()
        .find(|function| function.name == "read")
        .unwrap();
    let FullTypedStatementKind::Compound(items) = &function.body.as_ref().unwrap().kind else {
        panic!("read has a compound body")
    };
    let FullTypedBlockItem::Declaration(declaration) = &items[0] else {
        panic!("read starts with a declaration")
    };
    let FullTypedInitializerKind::Scalar(expression) =
        &declaration.initializer.as_ref().unwrap().kind
    else {
        panic!("pair has a scalar aggregate initializer")
    };
    let FullTypedExpressionKind::Conversion {
        kind: ConversionKind::LvalueToValue { .. },
        expression,
    } = &expression.kind
    else {
        panic!("compound literal is converted for aggregate initialization")
    };
    let FullTypedExpressionKind::CompoundLiteral { local, initializer } = &expression.kind else {
        panic!("pair is initialized from a compound literal")
    };
    assert_eq!(expression.category, ValueCategory::Lvalue);
    assert!(
        expression.place.as_ref().is_some_and(|place| place.base
            == PlaceBase::CompoundLiteral(*local)
            && place.addressable)
    );
    assert!(matches!(
        initializer.kind,
        FullTypedInitializerKind::Aggregate(ref entries) if entries.len() == 2
    ));

    assert_eq!(
        diagnostic_codes(
            "struct Pair { int left; int right; };
             struct Pair pair = (struct Pair){ .left = 1, .right = 2 };"
        ),
        vec!["CCC2430"]
    );
}

#[test]
fn label_tokens_and_computed_goto_have_exact_typed_semantics() {
    let source = "int dispatch(int opcode) {\n\
         static const void *const table[2] = {&&zero, &&one};\n\
         goto *table[opcode];\n\
     zero: return 10;\n\
     one: return 20;\n\
     }";
    let unit = analyze_source(source).unwrap();
    assert_eq!(
        dump_frontend_typed_ast(&unit),
        concat!(
            "translation-unit full-typed\n",
            "function @0 dispatch : int (int) storage=Extern linkage=External visibility=Default inline=false noreturn=false definition\n",
            "  parameter %0 opcode : int\n",
            "  compound\n",
            "    local %1 table : array[2] of const pointer to const void storage=Static duration=Static\n",
            "      data symbol=__ccc_block_static.dispatch.0.1.table visibility=Internal section=None align=None tls=None\n",
            "      initializer : array[2] of const pointer to const void\n",
            "        subobject [Index(0)]\n",
            "          initializer : const pointer to const void\n",
            "            convert PointerConversion : pointer to const void Value\n",
            "              constant Address(RelocatableAddress { base: Label { function: FullFunctionId(0), label: LabelId(0) }, addend: 0, one_past: false }) : pointer to void Value\n",
            "        subobject [Index(1)]\n",
            "          initializer : const pointer to const void\n",
            "            convert PointerConversion : pointer to const void Value\n",
            "              constant Address(RelocatableAddress { base: Label { function: FullFunctionId(0), label: LabelId(1) }, addend: 0, one_past: false }) : pointer to void Value\n",
            "    computed-goto\n",
            "      convert LvalueToValue { access: AccessSemantics { volatile: false, atomic: false } } : pointer to const void Value\n",
            "        subscript : const pointer to const void Lvalue\n",
            "          convert ArrayToPointer : pointer to const pointer to const void Value\n",
            "            decl-ref Local(FullLocalId(1)) : array[2] of const pointer to const void Lvalue\n",
            "          convert LvalueToValue { access: AccessSemantics { volatile: false, atomic: false } } : int Value\n",
            "            decl-ref Local(FullLocalId(0)) : int Lvalue\n",
            "    label ^0 zero\n",
            "      return\n",
            "        constant Signed(10) : int Value\n",
            "    label ^1 one\n",
            "      return\n",
            "        constant Signed(20) : int Value\n",
        )
    );
}

#[test]
fn computed_goto_rejects_nonpointers_label_arithmetic_and_missing_labels() {
    analyze_source(
        "int f(void) { void *token = &&done; void *copy = token; goto *copy; done: return 1; }",
    )
    .unwrap();
    analyze_source(
        "int f(void) { return &&left == &&left && &&left != &&right && !((long)&&left == 0); left: return 1; right: return 2; }",
    )
    .unwrap();
    assert_eq!(
        diagnostic_codes("int f(void) { goto *1; }"),
        vec!["CCC2424"]
    );
    assert_eq!(
        diagnostic_codes("void *target = &&missing;"),
        vec!["CCC2427"]
    );
    for source in [
        "int f(int offset) { goto *(&&base + offset); base: return 0; }",
        "int f(void) { return (int)(&&right - &&left); left: return 1; right: return 2; }",
        "int f(void) { return (long)&&left * 2; left: return 1; }",
        "int f(void) { return (long)&&left << 1; left: return 1; }",
        "int f(void) { return (long)&&left | 2; left: return 1; }",
        "int f(void) { return -(long)&&left; left: return 1; }",
        "int f(void) { return ~(long)&&left; left: return 1; }",
        "int f(void) { return &&left < &&right; left: return 1; right: return 2; }",
        "int f(int choose) { return (long)(choose ? &&left : &&right) * 2; left: return 1; right: return 2; }",
        "int f(void) { long value = 1; value *= (long)&&left; return value; left: return 1; }",
        "int f(void) { int values[3] = {0}; return values[(long)&&left]; left: return 1; }",
        "int f(void) { return ((char *)&&left)[0]; left: return 1; }",
    ] {
        assert_eq!(diagnostic_codes(source), vec!["CCC2425"], "{source}");
    }
    assert_eq!(
        diagnostic_codes("int f(void) { void *target = &&missing; return target != 0; }"),
        vec!["CCC2363"]
    );
}

#[test]
fn function_visibility_attributes_survive_redeclarations() {
    let unit = analyze_source(
        "int hidden(int) __attribute__((visibility(\"hidden\")));\n\
         int hidden(int value) { return value; }\n\
         int protected_fn(void) __attribute__((visibility(\"protected\")));\n\
         int protected_fn(void) { return 1; }",
    )
    .unwrap();
    let hidden = unit
        .functions
        .iter()
        .find(|function| function.name == "hidden")
        .unwrap();
    assert_eq!(hidden.visibility, SymbolVisibility::Hidden);
    let protected = unit
        .functions
        .iter()
        .find(|function| function.name == "protected_fn")
        .unwrap();
    assert_eq!(protected.visibility, SymbolVisibility::Protected);
}

#[test]
fn weak_symbol_binding_is_sticky_across_redeclarations() {
    let unit = analyze_source(
        "extern int weak_function(void);\n\
         extern int weak_function(void) __attribute__((__weak__));\n\
         int weak_function(void) { return 7; }\n\
         extern int weak_object __attribute__((weak));\n\
         int weak_object;",
    )
    .unwrap();

    let function = unit
        .functions
        .iter()
        .find(|function| function.name == "weak_function")
        .unwrap();
    assert_eq!(function.binding, SymbolBinding::Weak);
    assert!(function.body.is_some());

    let object = unit
        .globals
        .iter()
        .find(|global| global.name == "weak_object")
        .unwrap();
    assert_eq!(object.emission.binding, SymbolBinding::Weak);
    assert_eq!(
        object.emission.definition,
        ObjectDefinitionPolicy::Definition
    );
    assert!(!object.tentative);
    let dump = dump_frontend_typed_ast(&unit);
    assert!(
        dump.contains("weak_function") && dump.contains("binding=Weak"),
        "{dump}"
    );
}

#[test]
fn weak_attribute_rejects_non_symbol_and_internal_linkage_placements() {
    for source in [
        "static int object __attribute__((weak));",
        "static int function(void) __attribute__((__weak__));",
        "typedef int alias __attribute__((weak));",
        "int function(void) { int local __attribute__((weak)); return local; }",
        "int function(int value __attribute__((weak))) { return value; }",
        "int function(void) { static int local __attribute__((weak)); return local; }",
        "int function(int marker, ...) __attribute__((weak)); int function(int marker, ...) { return marker; }",
        "extern int object __attribute__((weak(1)));",
    ] {
        assert_eq!(diagnostic_codes(source), vec!["CCC2423"], "{source}");
    }
}

#[test]
fn assembly_labels_preserve_source_names_and_merge_redeclarations() {
    let unit = analyze_source(
        "extern int source_object __asm__(\"linked_object\");\n\
         extern int source_object;\n\
         extern int source_function(int) asm(\"linked_function\");\n\
         int source_function(int value) { return source_object + value; }\n\
         int use_block_declarations(void) {\n\
             extern int block_object asm(\"linked_block_object\");\n\
             extern int block_function(void) asm(\"linked_block_function\");\n\
             return block_object + block_function();\n\
         }",
    )
    .unwrap();

    let source_object = unit
        .globals
        .iter()
        .find(|global| global.name == "source_object")
        .unwrap();
    assert_eq!(source_object.emission.symbol_name, "linked_object");
    assert_eq!(
        source_object.asm_label.as_ref().unwrap().symbol,
        "linked_object"
    );
    assert_eq!(
        unit.globals
            .iter()
            .find(|global| global.name == "block_object")
            .unwrap()
            .emission
            .symbol_name,
        "linked_block_object"
    );

    let source_function = unit
        .functions
        .iter()
        .find(|function| function.name == "source_function")
        .unwrap();
    assert_eq!(source_function.name, "source_function");
    assert_eq!(
        source_function.asm_label.as_ref().unwrap().symbol,
        "linked_function"
    );
    assert!(source_function.body.is_some());
    assert_eq!(
        unit.functions
            .iter()
            .find(|function| function.name == "block_function")
            .unwrap()
            .asm_label
            .as_ref()
            .unwrap()
            .symbol,
        "linked_block_function"
    );
}

#[test]
fn assembly_labels_reject_conflicts_and_unsupported_storage() {
    for source in [
        "extern int function(void) asm(\"first\"); extern int function(void) asm(\"second\");",
        "extern int object asm(\"first\"); extern int object asm(\"second\");",
    ] {
        assert!(diagnostic_codes(source).contains(&"CCC2419".to_owned()));
    }
    assert!(
        diagnostic_codes("int f(void) { int local asm(\"linked\"); return local; }")
            .contains(&"CCC2257".to_owned())
    );
    assert!(
        diagnostic_codes("int f(void) { static int local asm(\"linked\"); return local; }")
            .contains(&"CCC2257".to_owned())
    );
    assert!(diagnostic_codes("extern int object asm(\"\");").contains(&"CCC2349".to_owned()));
}

#[test]
fn accepts_function_inlining_attributes_as_behavior_compatible_no_ops() {
    let unit = analyze_source(
        "static __attribute__((always_inline)) inline int fast(int value) { return value + 1; }\n\
         static int __attribute__((__always_inline__)) fast_alias(int value) { return value + 2; }\n\
         static __attribute__((noinline)) int slow(int value) { return value - 1; }\n\
         static int __attribute__((__noinline__)) slow_alias(int value) { return value - 2; }",
    )
    .unwrap();

    for (name, attribute_name) in [
        ("fast", "always_inline"),
        ("fast_alias", "__always_inline__"),
        ("slow", "noinline"),
        ("slow_alias", "__noinline__"),
    ] {
        let function = unit
            .functions
            .iter()
            .find(|function| function.name == name)
            .unwrap();
        let attribute = function
            .attributes
            .iter()
            .find(|attribute| attribute.name == attribute_name)
            .unwrap();
        assert_eq!(
            attribute.capability,
            CapabilityState::BehaviorCompatibleNoOp
        );
    }
}

#[test]
fn accepts_hosted_header_diagnostic_and_optimization_attributes_as_no_ops() {
    let unit = analyze_source(
        "extern void *allocate(const char *format, ...)\n\
         __attribute__((__const__, __malloc__, __format__(__printf__, 1, 2),\n\
                        __nonnull__(1), __warn_unused_result__, unused, __unused__,\n\
                        __deprecated__));",
    )
    .unwrap();
    let function = unit
        .functions
        .iter()
        .find(|function| function.name == "allocate")
        .unwrap();
    for name in [
        "__const__",
        "__malloc__",
        "__format__",
        "__nonnull__",
        "__warn_unused_result__",
        "unused",
        "__unused__",
        "__deprecated__",
    ] {
        let attribute = function
            .attributes
            .iter()
            .find(|attribute| attribute.name == name)
            .unwrap();
        assert_eq!(
            attribute.capability,
            CapabilityState::BehaviorCompatibleNoOp,
            "unexpected state for {name}"
        );
    }
}

#[test]
fn gnu_noreturn_attribute_updates_function_control_flow_properties() {
    let unit = analyze_source(
        "extern void stop(void) __attribute__((__noreturn__));\n\
         void stop(void) { for (;;) {} }",
    )
    .unwrap();
    let function = unit
        .functions
        .iter()
        .find(|function| function.name == "stop")
        .unwrap();
    assert!(function.properties.no_return);
    assert_eq!(
        function
            .attributes
            .iter()
            .find(|attribute| attribute.name == "__noreturn__")
            .unwrap()
            .capability,
        CapabilityState::Implemented
    );
}

#[test]
fn word_mode_uses_the_target_pointer_width_and_preserves_signedness() {
    let unit = analyze_source(
        "typedef int register_t __attribute__((__mode__(__word__)));\n\
         typedef unsigned int unsigned_register_t __attribute__((mode(word)));\n\
         register_t signed_value;\n\
         unsigned_register_t unsigned_value;",
    )
    .unwrap();
    let signed_value = unit
        .globals
        .iter()
        .find(|global| global.name == "signed_value")
        .unwrap();
    let unsigned_value = unit
        .globals
        .iter()
        .find(|global| global.name == "unsigned_value")
        .unwrap();
    assert_eq!(signed_value.ty.ty, ccc_types::TypeId::LONG);
    assert_eq!(unsigned_value.ty.ty, ccc_types::TypeId::UNSIGNED_LONG);
    assert!(
        diagnostic_codes("typedef int unsupported_mode __attribute__((mode(__QI__)));")
            .contains(&"CCC2421".to_owned())
    );
}

#[test]
fn aligned_attribute_preserves_object_and_private_typedef_alignment() {
    let unit = analyze_source(
        "int object __attribute__((__aligned__(32)));\n\
         int maximum_aligned __attribute__((__aligned__));\n\
         typedef struct { char byte; } aligned_record_t __attribute__((__aligned__));\n\
         aligned_record_t record;",
    )
    .unwrap();
    let object = unit
        .globals
        .iter()
        .find(|global| global.name == "object")
        .unwrap();
    assert_eq!(object.emission.requested_alignment, Some(32));
    let maximum_aligned = unit
        .globals
        .iter()
        .find(|global| global.name == "maximum_aligned")
        .unwrap();
    assert_eq!(maximum_aligned.emission.requested_alignment, Some(16));
    let record = unit
        .globals
        .iter()
        .find(|global| global.name == "record")
        .unwrap();
    let layout = unit
        .types
        .layout_of(record.ty.ty, &EffectiveCompilationConfig::default())
        .unwrap();
    assert_eq!(layout.align, 16);
    assert_eq!(layout.size, 16);

    assert!(
        diagnostic_codes(
            "typedef struct { char byte; } base_t;\n\
         typedef base_t unsafe_alias_t __attribute__((aligned));"
        )
        .contains(&"CCC2422".to_owned())
    );
}

fn diagnostic_codes(source: &str) -> Vec<String> {
    analyze_source(source)
        .unwrap_err()
        .into_iter()
        .map(|diagnostic| diagnostic.code)
        .collect()
}

fn first_local(function: &FullTypedFunction) -> &FullTypedLocalDeclaration {
    let body = function.body.as_ref().unwrap();
    let FullTypedStatementKind::Compound(items) = &body.kind else {
        panic!("function body is compound")
    };
    items
        .iter()
        .find_map(|item| match item {
            FullTypedBlockItem::Declaration(declaration) => Some(declaration.as_ref()),
            _ => None,
        })
        .expect("function has a local declaration")
}

#[test]
fn types_target_va_list_and_variadic_builtins_without_exposing_layout_in_syntax() {
    let unit = analyze_source(
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
    )
    .unwrap();
    let va_list = unit
        .types
        .target_builtin_id(ccc_types::TargetBuiltinType::VaList)
        .expect("target va_list type");
    let layout = unit
        .types
        .layout_of(va_list, &EffectiveCompilationConfig::default())
        .unwrap();
    assert_eq!((layout.size, layout.align), (24, 8));
    assert!(matches!(unit.types.kind(va_list), TypeKind::Array(_)));
    let dump = dump_frontend_typed_ast(&unit);
    assert!(dump.contains("va-start"), "{dump}");
    assert!(dump.contains("va-arg requested=int"), "{dump}");
    assert!(dump.contains("va-copy"), "{dump}");
    assert_eq!(dump.matches("va-end").count(), 2, "{dump}");
}

#[test]
fn types_sync_synchronize_as_a_registry_gated_sequentially_consistent_fence() {
    let unit = analyze_preprocessed_source(
        "sync-synchronize.c",
        "#if !__has_builtin(__sync_synchronize)\n\
         #error missing synchronization builtin\n\
         #endif\n\
         void synchronize(void) { __sync_synchronize(); }",
    )
    .unwrap();
    let function = unit
        .functions
        .iter()
        .find(|function| function.name == "synchronize")
        .unwrap();
    let FullTypedStatementKind::Compound(items) = &function.body.as_ref().unwrap().kind else {
        panic!("synchronize has a compound body")
    };
    let FullTypedBlockItem::Statement(statement) = &items[0] else {
        panic!("synchronize contains an expression statement")
    };
    let FullTypedStatementKind::Expression(Some(expression)) = &statement.kind else {
        panic!("synchronize contains a fence expression")
    };
    assert_eq!(expression.ty.ty, ccc_types::TypeId::VOID);
    assert_eq!(
        expression.kind,
        FullTypedExpressionKind::MemoryFence {
            order: MemoryOrder::SequentiallyConsistent,
        }
    );

    let mut config = EffectiveCompilationConfig::default();
    config.capabilities.insert(
        CapabilityKind::Builtin,
        "__sync_synchronize",
        CapabilityState::ParseOnly,
    );
    let diagnostics =
        analyze_source_with_config("void synchronize(void) { __sync_synchronize(); }", &config)
            .unwrap_err();
    assert_eq!(
        diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code.as_str())
            .collect::<Vec<_>>(),
        ["CCC2407"]
    );
}

#[test]
fn types_legacy_sync_operations_with_native_scalar_and_pointer_contracts() {
    let unit = analyze_preprocessed_source(
        "sync-operations.c",
        "#if !__has_builtin(__sync_add_and_fetch) || \
             !__has_builtin(__sync_fetch_and_add) || \
             !__has_builtin(__sync_sub_and_fetch) || \
             !__has_builtin(__sync_bool_compare_and_swap) || \
             !__has_builtin(__sync_val_compare_and_swap) || \
             !__has_builtin(__sync_lock_test_and_set)\n\
         #error missing atomic builtin\n\
         #endif\n\
         int value;\n\
         void *pointer;\n\
         int protected_side_effect(void);\n\
         int update(int delta) {\n\
             int old = __sync_fetch_and_add(&value, delta);\n\
             int now = __sync_add_and_fetch(&value, delta, protected_side_effect());\n\
             int after = __sync_sub_and_fetch(&value, delta, __sync_synchronize);\n\
             int changed = __sync_bool_compare_and_swap(&value, old, now);\n\
             int seen = __sync_val_compare_and_swap(&value, now, after);\n\
             pointer = __sync_lock_test_and_set(&pointer, (void *)0);\n\
             pointer = __sync_add_and_fetch(&pointer, (void *)1);\n\
             return old + now + after + changed + seen;\n\
         }",
    )
    .unwrap();
    let dump = dump_frontend_typed_ast(&unit);
    assert_eq!(dump.matches("atomic-rmw Add").count(), 3, "{dump}");
    assert_eq!(dump.matches("atomic-rmw Subtract").count(), 1, "{dump}");
    assert_eq!(dump.matches("atomic-rmw Exchange").count(), 1, "{dump}");
    assert_eq!(dump.matches("atomic-cmpxchg").count(), 2, "{dump}");
    assert!(
        dump.contains("atomic-cmpxchg object=int return-boolean=true"),
        "{dump}"
    );
    assert!(
        dump.contains("atomic-cmpxchg object=int return-boolean=true order=SequentiallyConsistent : _Bool Value"),
        "{dump}"
    );
    assert!(
        dump.contains("atomic-cmpxchg object=int return-boolean=false"),
        "{dump}"
    );
    assert_eq!(dump.matches("order=SequentiallyConsistent").count(), 7);
    assert!(
        !dump.contains("call function=Some(FullFunctionId(0))"),
        "the protected operand must not become an evaluated call: {dump}"
    );

    for (source, code) in [
        (
            "int f(void) { return __sync_fetch_and_add(1, 2); }",
            "CCC2433",
        ),
        (
            "const int x = 0; int f(void) { return __sync_fetch_and_add(&x, 1); }",
            "CCC2433",
        ),
        (
            "float x; int f(void) { return __sync_fetch_and_add(&x, 1); }",
            "CCC2434",
        ),
        (
            "_Bool x; int f(void) { return __sync_fetch_and_add(&x, 1); }",
            "CCC2434",
        ),
        (
            "struct Pair { int x; }; struct Pair value; int f(void) { return __sync_bool_compare_and_swap(&value, value, value); }",
            "CCC2434",
        ),
    ] {
        assert_eq!(diagnostic_codes(source), vec![code], "{source}");
    }

    let raw_conversions = analyze_source(
        "typedef void (*callback)(void);\n\
         callback g_callback;\n\
         void *pointer;\n\
         int raw;\n\
         _Bool atomic_set(void) {\n\
             return __sync_bool_compare_and_swap(\n\
                 &g_callback, ((void *)0), ((void *)0));\n\
         }\n\
         void *exchange_integer(void) {\n\
             return __sync_lock_test_and_set(&pointer, 1);\n\
         }\n\
         int exchange_pointer(void) {\n\
             return __sync_lock_test_and_set(&raw, (void *)1);\n\
         }",
    )
    .unwrap();
    let dump = dump_frontend_typed_ast(&raw_conversions);
    assert_eq!(
        dump.matches("convert PointerConversion").count(),
        8,
        "{dump}"
    );
}

#[test]
fn types_the_exact_integer_intrinsic_and_prefetch_contracts() {
    let unit = analyze_preprocessed_source(
        "integer-intrinsics.c",
        "#if !__has_builtin(__builtin_bswap64) || \
             !__has_builtin(__builtin_clz) || \
             !__has_builtin(__builtin_clzl) || \
             !__has_builtin(__builtin_clzll) || \
             !__has_builtin(__builtin_ctzll) || \
             !__has_builtin(__builtin_popcount) || \
             !__has_builtin(__builtin_popcountll) || \
             !__has_builtin(__builtin_prefetch)\n\
         #error missing selected builtin\n\
         #endif\n\
         #if __has_builtin(__builtin_bswap32) || \
             __has_builtin(__builtin_ctz) || \
             __has_builtin(__builtin_ctzl) || \
             __has_builtin(__builtin_popcountl)\n\
         #error unselected builtin was advertised\n\
         #endif\n\
         unsigned long swap(int value) { return __builtin_bswap64(value); }\n\
         int clz_int(int value) { return __builtin_clz(value); }\n\
         int clz_long(long value) { return __builtin_clzl(value); }\n\
         int clz_long_long(long long value) { return __builtin_clzll(value); }\n\
         int ctz_long_long(long long value) { return __builtin_ctzll(value); }\n\
         int popcount_int(int value) { return __builtin_popcount(value); }\n\
         int popcount_long_long(long long value) { return __builtin_popcountll(value); }\n\
         void *next_address(void);\n\
         int side_effect(void);\n\
         void hints(void) {\n\
             __builtin_prefetch(next_address());\n\
             __builtin_prefetch(next_address(), 1, 0);\n\
             __builtin_prefetch(\n\
                 next_address(), 1 ? 0 : side_effect(), 1 ? 3 : side_effect());\n\
             __builtin_prefetch(0);\n\
         }",
    )
    .unwrap();
    let dump = dump_frontend_typed_ast(&unit);
    for (operation, result) in [
        ("ByteSwap64", "unsigned long int"),
        ("CountLeadingZerosInt", "int"),
        ("CountLeadingZerosLong", "int"),
        ("CountLeadingZerosLongLong", "int"),
        ("CountTrailingZerosLongLong", "int"),
        ("PopulationCountInt", "int"),
        ("PopulationCountLongLong", "int"),
    ] {
        assert!(
            dump.contains(&format!("integer-intrinsic {operation} : {result} Value")),
            "{dump}"
        );
    }
    for input in [
        "unsigned int",
        "unsigned long int",
        "unsigned long long int",
    ] {
        assert!(
            dump.contains(&format!("convert IntegerConversion : {input} Value")),
            "{dump}"
        );
    }
    assert_eq!(
        dump.matches("prefetch write=false locality=3").count(),
        3,
        "{dump}"
    );
    assert_eq!(
        dump.matches("prefetch write=true locality=0").count(),
        1,
        "{dump}"
    );

    for source in [
        "void f(void) { __builtin_prefetch(1); }",
        "void f(volatile int *p) { __builtin_prefetch(p); }",
    ] {
        assert_eq!(diagnostic_codes(source), vec!["CCC2335"], "{source}");
    }
    for source in [
        "void f(void *p) { __builtin_prefetch(p, 2); }",
        "void f(void *p) { __builtin_prefetch(p, 0, 4); }",
        "void f(void *p, int write) { __builtin_prefetch(p, write); }",
        "void f(void *p) { __builtin_prefetch(p, 0.0); }",
        "void f(void *p) { __builtin_prefetch(p, 0, 3.0); }",
    ] {
        assert_eq!(diagnostic_codes(source), vec!["CCC2436"], "{source}");
    }
    analyze_source(
        "void f(void *p) {\n\
             __builtin_prefetch(p, (unsigned char)256, (unsigned char)259);\n\
             __builtin_prefetch(p, 4294967296ULL, 4294967299ULL);\n\
         }",
    )
    .unwrap();

    for (name, source) in [
        (
            "__builtin_bswap64",
            "unsigned long f(unsigned long value) { return __builtin_bswap64(value); }",
        ),
        (
            "__builtin_prefetch",
            "void f(void *pointer) { __builtin_prefetch(pointer); }",
        ),
    ] {
        let mut config = EffectiveCompilationConfig::default();
        config
            .capabilities
            .insert(CapabilityKind::Builtin, name, CapabilityState::ParseOnly);
        let diagnostics = analyze_source_with_config(source, &config).unwrap_err();
        assert_eq!(
            diagnostics
                .iter()
                .map(|diagnostic| diagnostic.code.as_str())
                .collect::<Vec<_>>(),
            ["CCC2407"],
            "{name}"
        );
    }
}

#[test]
fn folds_integer_intrinsics_in_integer_constant_expression_contexts() {
    analyze_source(
        "enum folded {\n\
             swapped = __builtin_bswap64(1UL),\n\
             leading_int = __builtin_clz(1U),\n\
             leading_long = __builtin_clzl(1UL),\n\
             leading_long_long = __builtin_clzll(1ULL),\n\
             trailing_long_long = __builtin_ctzll(0x100ULL),\n\
             population_int = __builtin_popcount(0xf0U),\n\
             population_long_long = __builtin_popcountll(0xf00000000000000fULL)\n\
         };\n\
         _Static_assert(swapped == 0x0100000000000000UL, \"bswap64\");\n\
         _Static_assert(leading_int == 31, \"clz\");\n\
         _Static_assert(leading_long == 63, \"clzl\");\n\
         _Static_assert(leading_long_long == 63, \"clzll\");\n\
         _Static_assert(trailing_long_long == 8, \"ctzll\");\n\
         _Static_assert(population_int == 4, \"popcount\");\n\
         _Static_assert(population_long_long == 8, \"popcountll\");\n\
         int folded_array[(swapped == 0x0100000000000000UL &&\n\
             leading_int + leading_long + leading_long_long +\n\
             trailing_long_long + population_int + population_long_long == 177) ? 7 : -1];",
    )
    .unwrap();

    assert!(
        analyze_source("enum invalid { value = __builtin_clz(0U) };").is_err(),
        "zero-input clz must remain outside constant folding"
    );
    analyze_source("int runtime(unsigned value) { return __builtin_clz(value); }").unwrap();
}

#[test]
fn types_scalar_constants_and_expect_from_the_builtin_registry() {
    let unit = analyze_preprocessed_source(
        "scalar-builtins.c",
        "#if !__has_builtin(__builtin_expect)\n\
         #error missing expect builtin\n\
         #endif\n\
         #if !__has_builtin(__builtin_huge_val)\n\
         #error missing huge-value builtin\n\
         #endif\n\
         #if !__has_builtin(__builtin_inff)\n\
         #error missing float-infinity builtin\n\
         #endif\n\
         #if !__has_builtin(__builtin_nanf)\n\
         #error missing float-NaN builtin\n\
         #endif\n\
         enum { expectation = 1, folded = __builtin_expect(7, expectation) };\n\
         _Static_assert(__builtin_expect(1, expectation), \"folded expectation\");\n\
         long choose(signed char value) {\n\
             return __builtin_expect(value, (0, 1));\n\
         }\n\
         double infinity(void) { return __builtin_huge_val(); }\n\
         float infinityf(void) { return __builtin_inff(); }\n\
         float not_a_number(void) { return __builtin_nanf(\"\"); }\n\
         float utf8_not_a_number(void) { return __builtin_nanf(u8\"\"); }",
    )
    .unwrap();

    let choose = unit
        .functions
        .iter()
        .find(|function| function.name == "choose")
        .unwrap();
    let FullTypedStatementKind::Compound(items) = &choose.body.as_ref().unwrap().kind else {
        panic!("choose has a compound body")
    };
    let FullTypedBlockItem::Statement(statement) = &items[0] else {
        panic!("choose has a return statement")
    };
    let FullTypedStatementKind::Return(Some(expression)) = &statement.kind else {
        panic!("choose returns an expression")
    };
    let FullTypedExpressionKind::BuiltinExpect { value, expected } = &expression.kind else {
        panic!("choose returns builtin expect")
    };
    assert_eq!(expression.ty.ty, TypeId::LONG);
    assert_eq!(value.ty.ty, TypeId::LONG);
    assert_eq!(expected.ty.ty, TypeId::LONG);
    assert_eq!(expected.constant, Some(ConstantValue::Signed(1)));

    let infinity = unit
        .functions
        .iter()
        .find(|function| function.name == "infinity")
        .unwrap();
    let FullTypedStatementKind::Compound(items) = &infinity.body.as_ref().unwrap().kind else {
        panic!("infinity has a compound body")
    };
    let FullTypedBlockItem::Statement(statement) = &items[0] else {
        panic!("infinity has a return statement")
    };
    let FullTypedStatementKind::Return(Some(expression)) = &statement.kind else {
        panic!("infinity returns an expression")
    };
    assert_eq!(expression.ty.ty, TypeId::DOUBLE);
    assert!(matches!(
        expression.constant,
        Some(ConstantValue::Floating(value)) if value.is_infinite() && value.is_sign_positive()
    ));

    let infinityf = unit
        .functions
        .iter()
        .find(|function| function.name == "infinityf")
        .unwrap();
    let FullTypedStatementKind::Compound(items) = &infinityf.body.as_ref().unwrap().kind else {
        panic!("infinityf has a compound body")
    };
    let FullTypedBlockItem::Statement(statement) = &items[0] else {
        panic!("infinityf has a return statement")
    };
    let FullTypedStatementKind::Return(Some(expression)) = &statement.kind else {
        panic!("infinityf returns an expression")
    };
    assert_eq!(expression.ty.ty, TypeId::FLOAT);
    assert!(matches!(
        expression.constant,
        Some(ConstantValue::Floating(value))
            if (value as f32).to_bits() == f32::INFINITY.to_bits()
    ));

    for function_name in ["not_a_number", "utf8_not_a_number"] {
        let function = unit
            .functions
            .iter()
            .find(|function| function.name == function_name)
            .unwrap();
        let FullTypedStatementKind::Compound(items) = &function.body.as_ref().unwrap().kind else {
            panic!("{function_name} has a compound body")
        };
        let FullTypedBlockItem::Statement(statement) = &items[0] else {
            panic!("{function_name} has a return statement")
        };
        let FullTypedStatementKind::Return(Some(expression)) = &statement.kind else {
            panic!("{function_name} returns an expression")
        };
        assert_eq!(expression.ty.ty, TypeId::FLOAT);
        assert!(matches!(
            expression.constant,
            Some(ConstantValue::Floating(value)) if (value as f32).to_bits() == 0x7fc0_0000
        ));
    }

    assert_eq!(
        diagnostic_codes("long f(void) { return __builtin_expect(1, missing); }"),
        vec!["CCC2274"]
    );
    assert_eq!(
        diagnostic_codes(
            "struct Pair { int value; }; long f(struct Pair pair) { return __builtin_expect(1, pair); }"
        ),
        vec!["CCC2336"]
    );
    for source in [
        "long f(long expected) { return __builtin_expect(1, expected); }",
        "long side_effect(void); long f(void) { return __builtin_expect(1, side_effect()); }",
        "long side_effect(void); long f(void) { return __builtin_expect(1, (side_effect(), 1)); }",
    ] {
        assert_eq!(diagnostic_codes(source), vec!["CCC2428"], "{source}");
    }

    for (name, source) in [
        (
            "__builtin_expect",
            "long choose(void) { return __builtin_expect(1, 1); }",
        ),
        (
            "__builtin_huge_val",
            "double infinity(void) { return __builtin_huge_val(); }",
        ),
        (
            "__builtin_inff",
            "float infinityf(void) { return __builtin_inff(); }",
        ),
        (
            "__builtin_nanf",
            "float not_a_number(void) { return __builtin_nanf(\"\"); }",
        ),
    ] {
        let mut config = EffectiveCompilationConfig::default();
        config
            .capabilities
            .insert(CapabilityKind::Builtin, name, CapabilityState::ParseOnly);
        let diagnostics = analyze_source_with_config(source, &config).unwrap_err();
        assert_eq!(
            diagnostics
                .iter()
                .map(|diagnostic| diagnostic.code.as_str())
                .collect::<Vec<_>>(),
            ["CCC2407"],
            "{name}"
        );
    }

    for source in [
        "float not_a_number(char *payload) { return __builtin_nanf(payload); }",
        "float not_a_number(void) { return __builtin_nanf(L\"\"); }",
        "float not_a_number(void) { return __builtin_nanf(\"payload\"); }",
    ] {
        assert_eq!(diagnostic_codes(source), vec!["CCC2429"], "{source}");
    }
}

#[test]
fn builtin_expect_uses_folded_constant_semantics() {
    for source in [
        "long f(void) { return __builtin_expect(1, (0, 1)); }",
        "long f(void) { return __builtin_expect(1, 1.0); }",
        "long side_effect(void); long f(void) { return __builtin_expect(1, 1 ? 1 : side_effect()); }",
        "long side_effect(void); long f(void) { return __builtin_expect(1, 0 ? side_effect() : 1); }",
        "long side_effect(void); long f(void) { return __builtin_expect(1, 0 && side_effect()); }",
        "long side_effect(void); long f(void) { return __builtin_expect(1, 1 || side_effect()); }",
    ] {
        analyze_source(source).unwrap_or_else(|diagnostics| panic!("{source}: {diagnostics:#?}"));
    }

    for source in [
        "long f(long expected) { return __builtin_expect(1, expected); }",
        "long side_effect(void); long f(void) { return __builtin_expect(1, side_effect()); }",
        "long side_effect(void); long f(void) { return __builtin_expect(1, (side_effect(), 1)); }",
        "long side_effect(void); long f(void) { return __builtin_expect(1, 1 ? side_effect() : 1); }",
        "long side_effect(void); long f(void) { return __builtin_expect(1, 0 ? 1 : side_effect()); }",
        "long side_effect(void); long f(void) { return __builtin_expect(1, 1 && side_effect()); }",
        "long side_effect(void); long f(void) { return __builtin_expect(1, 0 || side_effect()); }",
    ] {
        let diagnostics = analyze_source(source).unwrap_err();
        assert_eq!(
            diagnostics
                .iter()
                .map(|diagnostic| diagnostic.code.as_str())
                .collect::<Vec<_>>(),
            ["CCC2428"],
            "{source}: {diagnostics:#?}"
        );
        assert_eq!(
            diagnostics[0].message,
            "the second argument to `__builtin_expect` must be a compile-time constant",
            "{source}"
        );
    }
}

#[test]
fn diagnoses_invalid_variadic_builtin_uses_even_when_unreachable() {
    assert_eq!(
        diagnostic_codes(
            "typedef __builtin_va_list va_list;\n\
             int fixed(int count) { va_list list; __builtin_va_start(list, count); return 0; }"
        ),
        vec!["CCC2400"]
    );
    assert_eq!(
        diagnostic_codes(
            "typedef __builtin_va_list va_list;\n\
             int bad(int first, int last, ...) {\n\
                 va_list list; __builtin_va_start(list, first); return 0;\n\
             }"
        ),
        vec!["CCC2402"]
    );
    assert_eq!(
        diagnostic_codes(
            "typedef __builtin_va_list va_list;\n\
             int bad(int count, ...) {\n\
                 va_list list; __builtin_va_start(list, count);\n\
                 if (0) __builtin_va_arg(list, float);\n\
                 return 0;\n\
             }"
        ),
        vec!["CCC2403"]
    );
    assert_eq!(
        diagnostic_codes(
            "int bad(int count, ...) {\n\
                 int list; __builtin_va_start(list, count); return 0;\n\
             }"
        ),
        vec!["CCC2411"]
    );
    assert_eq!(
        diagnostic_codes(
            "typedef __builtin_va_list va_list;\n\
             int bad(int count, ...) {\n\
                 const va_list list; __builtin_va_end(list); return 0;\n\
             }"
        ),
        vec!["CCC2410"]
    );
    assert_eq!(
        diagnostic_codes(
            "typedef __builtin_va_list va_list;\n\
             int bad(int count, ...) {\n\
                 va_list list; return __builtin_va_arg(list, int[2])[0];\n\
             }"
        ),
        vec!["CCC2405"]
    );
    assert_eq!(
        diagnostic_codes(
            "typedef __builtin_va_list va_list;\n\
             int bad(int count, ...) {\n\
                 va_list list;\n\
                 return __builtin_va_arg(list, int (*)[count]) != 0;\n\
             }"
        ),
        vec!["CCC2414"]
    );
    assert_eq!(
        diagnostic_codes(
            "typedef __builtin_va_list va_list;\n\
             int bad(int count, ...) {\n\
                 va_list list;\n\
                 return __builtin_va_arg(list, int (*(*)(void))[count++]) != 0;\n\
             }"
        ),
        vec!["CCC2414"]
    );
    for parameter in ["float count", "register int count", "int count[1]"] {
        let source = format!(
            "typedef __builtin_va_list va_list;\n\
             int bad({parameter}, ...) {{\n\
                 va_list list; __builtin_va_start(list, count); return 0;\n\
             }}"
        );
        assert_eq!(diagnostic_codes(&source), vec!["CCC2413"], "{parameter}");
    }
}

#[test]
fn accepts_compatible_enum_types_at_variadic_boundaries() {
    let unit = analyze_source(
        "enum Choice { CHOICE = 3 };\n\
         typedef __builtin_va_list va_list;\n\
         enum Choice read(enum Choice final, ...) {\n\
             va_list list;\n\
             __builtin_va_start(list, final);\n\
             enum Choice value = __builtin_va_arg(list, enum Choice);\n\
             __builtin_va_end(list);\n\
             return value;\n\
         }",
    )
    .unwrap();
    let dump = dump_frontend_typed_ast(&unit);
    assert!(dump.contains("va-start"), "{dump}");
    assert!(dump.contains("va-arg requested=enum Choice"), "{dump}");
}

#[test]
fn promotes_narrow_unsigned_int_bitfields_through_an_ellipsis() {
    let unit = analyze_source(
        "struct Bits { unsigned narrow : 5; unsigned wide : 32; };\n\
         int sink(int marker, ...);\n\
         int pass(struct Bits *bits) {\n\
             return sink(0, bits->narrow, bits->wide);\n\
         }",
    )
    .unwrap();
    let dump = dump_frontend_typed_ast(&unit);
    let promotions = dump
        .lines()
        .filter(|line| line.contains("convert IntegerPromotion"))
        .collect::<Vec<_>>();
    assert_eq!(promotions, ["        convert IntegerPromotion : int Value"]);
    assert_eq!(
        dump.matches("convert LvalueToValue { access: AccessSemantics { volatile: false, atomic: false } } : unsigned int Value")
            .count(),
        2,
        "{dump}"
    );
}

#[test]
fn adjusts_va_list_parameters_to_the_public_record_pointer() {
    let unit = analyze_source(
        "typedef __builtin_va_list va_list;\n\
         int next(va_list list) { return __builtin_va_arg(list, int); }",
    )
    .unwrap();
    let function = unit
        .functions
        .iter()
        .find(|function| function.name == "next")
        .unwrap();
    let TypeKind::Pointer(pointer) = unit.types.kind(function.parameters[0].ty.ty) else {
        panic!("va_list parameter must be adjusted to a pointer")
    };
    assert!(matches!(
        unit.types.kind(pointer.pointee.ty),
        TypeKind::Record(_)
    ));
    assert!(dump_frontend_typed_ast(&unit).contains("va-arg requested=int"));
}

#[test]
fn resource_stdarg_supports_partial_and_repeated_inclusion() {
    let unit = analyze_resource_source(
        "#if !__has_builtin(__builtin_va_start) || !__has_builtin(__builtin_va_arg) || \
             !__has_builtin(__builtin_va_copy) || !__has_builtin(__builtin_va_end)\n\
         #error missing variadic builtins\n\
         #endif\n\
         #define __need___va_list\n\
         #include <stdarg.h>\n\
         __gnuc_va_list first;\n\
         #define __need_va_list\n\
         #include <stdarg.h>\n\
         va_list second;\n\
         #define __need_va_arg\n\
         #include <stdarg.h>\n\
         #ifndef va_arg\n\
         #error va_arg partial include failed\n\
         #endif\n\
         #define __need___va_copy\n\
         #include <stdarg.h>\n\
         #ifndef __va_copy\n\
         #error __va_copy partial include failed\n\
         #endif\n\
         #define __need_va_copy\n\
         #include <stdarg.h>\n\
         #ifndef va_copy\n\
         #error va_copy partial include failed\n\
         #endif\n\
         #include <stdarg.h>\n\
         #include <stdarg.h>\n\
         int next(va_list list) { return va_arg(list, int); }",
    )
    .unwrap();
    assert_eq!(
        unit.globals
            .iter()
            .map(|global| global.name.as_str())
            .collect::<Vec<_>>(),
        vec!["first", "second"]
    );
    let next = unit
        .functions
        .iter()
        .find(|function| function.name == "next")
        .unwrap();
    assert!(matches!(
        unit.types.kind(next.parameters[0].ty.ty),
        TypeKind::Pointer(_)
    ));
}

#[test]
fn makes_loads_promotions_and_volatile_accesses_explicit() {
    let unit = analyze_source(
        "volatile int g;\n\
         int f(short x) { g = x + 1; return g; }",
    )
    .unwrap();
    let dump = dump_frontend_typed_ast(&unit);
    assert!(dump.contains("IntegerPromotion"), "{dump}");
    assert!(
        dump.contains(
            "LvalueToValue { access: AccessSemantics { volatile: true, atomic: false } }"
        ),
        "{dump}"
    );
    assert!(
        dump.contains("store=AccessSemantics { volatile: true, atomic: false }"),
        "{dump}"
    );
}

#[test]
fn retains_typedef_and_parameter_qualifiers_in_the_correct_layer() {
    let unit = analyze_source(
        "typedef const int CI; CI g;\n\
         int f(volatile int x, int a[const 3]) { return x + a[0]; }",
    )
    .unwrap();
    let global = unit
        .globals
        .iter()
        .find(|global| global.name == "g")
        .unwrap();
    assert!(global.ty.qualifiers.contains(TypeQualifiers::CONST));

    let function = unit
        .functions
        .iter()
        .find(|function| function.name == "f")
        .unwrap();
    assert!(
        function.parameters[0]
            .ty
            .qualifiers
            .contains(TypeQualifiers::VOLATILE)
    );
    assert!(
        function.parameters[1]
            .ty
            .qualifiers
            .contains(TypeQualifiers::CONST)
    );
    let TypeKind::Pointer(pointer) = unit.types.kind(function.parameters[1].ty.ty) else {
        panic!("adjusted array parameter is a pointer")
    };
    assert!(pointer.pointee.qualifiers.is_empty());

    let signature = unit.types.function_signature(function.signature).unwrap();
    let ccc_types::FunctionParameters::Prototype(parameters) = signature.parameters else {
        panic!("function has a prototype")
    };
    assert!(
        parameters
            .iter()
            .all(|parameter| parameter.qualifiers.is_empty())
    );
}

#[test]
fn preserves_unspecified_parameters_and_empty_prototypes() {
    let unit = analyze_source("int unspecified(); int prototype(void);").unwrap();
    let signature = |name| {
        let function = unit
            .functions
            .iter()
            .find(|function| function.name == name)
            .unwrap();
        unit.types.function_signature(function.signature).unwrap()
    };

    assert!(matches!(
        signature("unspecified").parameters,
        ccc_types::FunctionParameters::Unspecified
    ));
    assert!(matches!(
        signature("prototype").parameters,
        ccc_types::FunctionParameters::Prototype(ref parameters) if parameters.is_empty()
    ));
}

#[test]
fn empty_identifier_list_definition_has_a_fixed_zero_parameter_boundary() {
    let unit = analyze_source("int answer() { return 42; }").unwrap();
    let function = unit
        .functions
        .iter()
        .find(|function| function.name == "answer")
        .unwrap();
    let signature = unit.types.function_signature(function.signature).unwrap();
    assert!(matches!(
        signature.parameters,
        ccc_types::FunctionParameters::Prototype(ref parameters) if parameters.is_empty()
    ));
}

#[test]
fn distinguishes_unspecified_variable_bounds_at_prototype_scope() {
    let unit = analyze_source(
        "int inspect(int (*matrix)[*]);\n\
         int inspect(int (*matrix)[*]);",
    )
    .unwrap();
    let function = unit
        .functions
        .iter()
        .find(|function| function.name == "inspect")
        .unwrap();
    let signature = unit.types.function_signature(function.signature).unwrap();
    let ccc_types::FunctionParameters::Prototype(parameters) = signature.parameters else {
        panic!("inspect has a prototype")
    };
    let TypeKind::Pointer(pointer) = unit.types.kind(parameters[0].ty) else {
        panic!("matrix is a pointer parameter")
    };
    let TypeKind::Array(array) = unit.types.kind(pointer.pointee.ty) else {
        panic!("matrix points to an array")
    };
    assert!(matches!(array.length, ArrayLength::UnspecifiedVariable(_)));

    let expression_prototype = analyze_source("int sized(int n, int (*matrix)[n]);").unwrap();
    let sized = expression_prototype
        .functions
        .iter()
        .find(|function| function.name == "sized")
        .unwrap();
    let signature = expression_prototype
        .types
        .function_signature(sized.signature)
        .unwrap();
    let ccc_types::FunctionParameters::Prototype(parameters) = signature.parameters else {
        panic!("sized has a prototype")
    };
    let TypeKind::Pointer(pointer) = expression_prototype.types.kind(parameters[1].ty) else {
        panic!("matrix is a pointer")
    };
    let TypeKind::Array(array) = expression_prototype.types.kind(pointer.pointee.ty) else {
        panic!("matrix points to an array")
    };
    assert!(matches!(array.length, ArrayLength::UnspecifiedVariable(_)));

    for source in [
        "int inspect(int (*matrix)[*]) { return 0; }",
        "int inspect(void) { int (*matrix)[*]; return 0; }",
    ] {
        assert_eq!(diagnostic_codes(source), vec!["CCC2223"]);
    }
}

#[test]
fn retains_parameter_and_local_variable_length_bounds_without_requiring_vla_storage() {
    assert!(analyze_source("int accepted(int n, int values[n]) { return values[0]; }").is_ok());

    let unit = analyze_source(
        "int inspect(int n, int (*matrix)[n]) {\n\
             int (*row)[n++];\n\
             static int (*saved)[n];\n\
             return 0;\n\
         }",
    )
    .unwrap();
    let function = unit
        .functions
        .iter()
        .find(|function| function.name == "inspect")
        .unwrap();

    let parameter_bound = &function.parameters[1].variable_length_bounds[0];
    let FullTypedExpressionKind::Conversion { expression, .. } = &parameter_bound.expression.kind
    else {
        panic!("parameter bound performs the parameter lvalue conversion")
    };
    assert!(matches!(
        expression.kind,
        FullTypedExpressionKind::DeclRef(SymbolReference::Local(FullLocalId(0)))
    ));
    let TypeKind::Pointer(pointer) = unit.types.kind(function.parameters[1].ty.ty) else {
        panic!("matrix is a pointer")
    };
    let TypeKind::Array(array) = unit.types.kind(pointer.pointee.ty) else {
        panic!("matrix points to an array")
    };
    assert_eq!(array.length, ArrayLength::Variable(parameter_bound.id));

    let FullTypedStatementKind::Compound(items) = &function.body.as_ref().unwrap().kind else {
        panic!("inspect has a compound body")
    };
    let declarations = items
        .iter()
        .filter_map(|item| match item {
            FullTypedBlockItem::Declaration(declaration) => Some(declaration.as_ref()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(declarations.len(), 2);
    assert!(matches!(
        declarations[0].variable_length_bounds[0].expression.kind,
        FullTypedExpressionKind::Increment { .. }
    ));
    assert_eq!(declarations[0].duration, StorageDuration::Automatic);
    assert_eq!(declarations[1].duration, StorageDuration::Static);
    assert_eq!(declarations[1].variable_length_bounds.len(), 1);

    assert_eq!(
        diagnostic_codes("int rejected(int n) { int values[2][n]; return 0; }"),
        vec!["CCC2258"]
    );
    assert_eq!(
        diagnostic_codes("int rejected(int n) { extern int (*value)[n]; return 0; }"),
        vec!["CCC2415"]
    );
    assert_eq!(
        diagnostic_codes("int rejected(int n) { extern int (*(*value)(void))[n++]; return 0; }"),
        vec!["CCC2415"]
    );
    assert_eq!(
        diagnostic_codes("int rejected(int n) { typedef int Row[n]; return 0; }"),
        vec!["CCC2416"]
    );
    assert_eq!(
        diagnostic_codes("void *rejected(int n, void *value) { return (int (*)[n++])value; }"),
        vec!["CCC2417"]
    );
}

#[test]
fn separates_integer_constant_expression_rules_from_value_folding() {
    assert!(
        analyze_source(
            "_Static_assert((int)4.0 == 4, \"direct floating cast\");\n\
             _Static_assert(1 ? 4 : (2, 3), \"unselected comma\");\n\
             _Static_assert(1 || (2, 3), \"short-circuited comma\");\n\
             _Static_assert(2 || 1 / 0, \"short-circuited division\");\n\
             _Static_assert(!(0 && 1 / 0), \"short-circuited division\");"
        )
        .is_ok()
    );

    for source in [
        "_Static_assert((1, 4), \"evaluated comma\");",
        "_Static_assert((int)(double)4.0 == 4, \"nested floating cast\");",
        "_Static_assert(0 ? 4 : (2, 3), \"selected comma\");",
        "_Static_assert(0 || (2, 3), \"evaluated comma\");",
        "int f(int n) { _Static_assert(1 ? 4 : n++, \"invalid operand\"); return 0; }",
        "int f(int n) { _Static_assert(1 || n++, \"invalid operand\"); return 0; }",
        "_Static_assert(1 / 0, \"evaluated division by zero\");",
    ] {
        assert_eq!(diagnostic_codes(source), vec!["CCC2338"]);
    }
    assert_eq!(
        diagnostic_codes("extern int (*value)[(1, 4)];"),
        vec!["CCC2223"]
    );

    let unit = analyze_source("int f(int n) { int (*value)[(n++, 4)]; return n; }").unwrap();
    let local = first_local(&unit.functions[0]);
    assert_eq!(local.variable_length_bounds.len(), 1);
    let bound = &local.variable_length_bounds[0].expression;
    assert!(matches!(bound.kind, FullTypedExpressionKind::Comma(_)));
    assert_eq!(bound.constant, Some(ConstantValue::Signed(4)));
    assert_eq!(
        bound.constant_expression_kind,
        ConstantExpressionKind::Invalid
    );
    let dump = dump_frontend_typed_ast(&unit);
    assert!(dump.contains("variable-length-bound vla"), "{dump}");
    assert!(dump.contains("comma : int Value"), "{dump}");
}

#[test]
fn rejects_incompatible_constant_array_composites() {
    assert_eq!(
        diagnostic_codes("extern int values[2]; extern int values[3];"),
        vec!["CCC2251"]
    );
    assert_eq!(
        diagnostic_codes("int apply(int (*values)[2]); int apply(int (*values)[3]);"),
        vec!["CCC2248"]
    );
}

#[test]
fn rejects_variably_modified_members_and_block_function_types() {
    for source in [
        "int f(int n) { struct S { int values[n]; }; return 0; }",
        "int f(int n) { struct S { int (*values)[n++]; }; return 0; }",
        "int f(int n) { struct S { int (*(*value)(void))[n++]; }; return 0; }",
    ] {
        assert_eq!(diagnostic_codes(source), vec!["CCC2235"]);
    }

    assert!(analyze_source("int f(int n) { int nested(int (*values)[n]); return 0; }").is_ok());
    assert_eq!(
        diagnostic_codes("int f(int n) { int (*nested(void))[n]; return 0; }"),
        vec!["CCC2418"]
    );
    assert_eq!(
        diagnostic_codes("void f(int n, struct S { int (*(*value)(void))[n++]; } argument);"),
        vec!["CCC2235"]
    );
    assert!(
        analyze_source("void f(int n, struct S { int (*callback)(int values[n]); } argument);")
            .is_ok()
    );
    assert!(
        analyze_source("void f(struct S { int (*callback)(int values[*]); } argument);").is_ok()
    );
}

#[test]
fn prototype_array_context_is_local_to_parameter_declarators() {
    for source in [
        "void atomic_star(_Atomic(int (*)[*]) value);",
        "void enum_star(enum E { X = sizeof(int (*)[*]) } value);",
        "void nested_size(int values[sizeof(int (*)[*])]);",
        "void definition(int (*(*callback)(void))[*]) {}",
    ] {
        assert_eq!(diagnostic_codes(source), vec!["CCC2223"], "{source}");
    }
    assert!(analyze_source("void prototype(int (*(*callback)(void))[*]);").is_ok());
}

#[test]
fn function_parameters_share_the_outer_body_scope() {
    assert_eq!(
        diagnostic_codes("int f(int value) { int value; return value; }"),
        vec!["CCC2364"]
    );
    assert!(
        analyze_source("int f(int value) { { int value = 1; (void)value; } return value; }")
            .is_ok()
    );
}

#[test]
fn definition_parameter_tags_and_enumerators_reach_only_the_function_body() {
    assert!(
        analyze_source(
            "int tagged(struct Tag { int value; } input) {\n\
                 struct Tag *copy = &input;\n\
                 return copy->value;\n\
             }\n\
             int enumerated(enum Choice { CHOSEN = 7 } input) { return CHOSEN + input; }"
        )
        .is_ok()
    );
    assert_eq!(
        diagnostic_codes(
            "int f(enum Choice { CHOSEN = 7 } input) { return CHOSEN; }\n\
             int g(void) { return CHOSEN; }"
        ),
        vec!["CCC2274"]
    );
    assert_eq!(
        diagnostic_codes(
            "int f(struct Tag { int value; } input) { return input.value; }\n\
             struct Tag outside;"
        ),
        vec!["CCC2342"]
    );
}

#[test]
fn register_parameters_remain_unaddressable_in_later_bounds() {
    for source in [
        "int f(register int n, int (*values)[sizeof &n]);",
        "int f(register int n, int (*values)[sizeof &n]) { return 0; }",
    ] {
        assert_eq!(diagnostic_codes(source), vec!["CCC2277"]);
    }
}

#[test]
fn definition_parameters_come_from_the_function_nearest_the_identifier() {
    let no_parameters = analyze_source("int (*factory(void))(double value) { return 0; }").unwrap();
    assert!(no_parameters.functions[0].parameters.is_empty());

    let unit =
        analyze_source("int (*factory(int n, int (*matrix)[n]))(double value) { return 0; }")
            .unwrap();
    let function = &unit.functions[0];
    assert_eq!(
        function
            .parameters
            .iter()
            .map(|parameter| parameter.name.as_str())
            .collect::<Vec<_>>(),
        ["n", "matrix"]
    );
    assert_eq!(function.parameters[1].variable_length_bounds.len(), 1);
}

#[test]
fn forms_order_independent_composite_array_and_function_types() {
    for source in [
        "extern const int values[]; extern const int values[10];\n\
         int apply(); int apply(int value); int use(void) { return apply(sizeof values); }",
        "extern const int values[10]; extern const int values[];\n\
         int apply(int value); int apply(); int use(void) { return apply(sizeof values); }",
    ] {
        let unit = analyze_source(source).unwrap();
        let values = unit
            .globals
            .iter()
            .find(|global| global.name == "values")
            .unwrap();
        let TypeKind::Array(array) = unit.types.kind(values.ty.ty) else {
            panic!("values has array type")
        };
        assert_eq!(array.length, ArrayLength::Constant(10));
        assert!(array.element.qualifiers.contains(TypeQualifiers::CONST));

        let apply = unit
            .functions
            .iter()
            .find(|function| function.name == "apply")
            .unwrap();
        let signature = unit.types.function_signature(apply.signature).unwrap();
        assert!(matches!(
            signature.parameters,
            ccc_types::FunctionParameters::Prototype(ref parameters)
                if parameters == &[ccc_types::QualifiedType::unqualified(ccc_types::TypeId::INT)]
        ));
    }

    assert!(
        diagnostic_codes("int incompatible(); int incompatible(float value);")
            .contains(&"CCC2248".to_owned())
    );
}

#[test]
fn rejects_static_redeclarations_after_external_linkage_at_the_new_declaration() {
    for source in [
        "extern int object;\nstatic int object;",
        "extern int function(void);\nstatic int function(void);",
    ] {
        let diagnostics = analyze_source(source).unwrap_err();
        let diagnostic = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == "CCC2372")
            .expect("linkage conflict diagnostic");
        let primary = diagnostic.primary.as_ref().expect("primary source span");
        let second_line = source.find('\n').unwrap() + 1;
        assert!(primary.span.start >= second_line, "{diagnostic:?}");
        assert!(
            diagnostic
                .message
                .contains("follows a non-static declaration")
        );
    }
}

#[test]
fn selects_target_integer_representations_for_enumerations() {
    let unit = analyze_source(
        "enum Big { HUGE = 3000000000, ALSO_HUGE = HUGE };\n\
         enum Big value = HUGE;\n\
         enum Negative { LOW = -3000000000 };",
    )
    .unwrap();

    let underlying = |name: &str| {
        let global = unit
            .globals
            .iter()
            .find(|global| global.name == name)
            .unwrap();
        let TypeKind::Enum(id) = unit.types.kind(global.ty.ty) else {
            panic!("global has enum type")
        };
        unit.types
            .enumeration(*id)
            .unwrap()
            .body
            .as_ref()
            .unwrap()
            .underlying
    };
    assert_eq!(underlying("value"), ccc_types::TypeId::UNSIGNED_INT);
    let value = unit
        .globals
        .iter()
        .find(|global| global.name == "value")
        .unwrap();
    let FullTypedInitializerKind::Scalar(initializer) = &value.initializer.as_ref().unwrap().kind
    else {
        panic!("enum initializer is scalar")
    };
    assert_eq!(
        initializer.constant,
        Some(ConstantValue::Unsigned(3_000_000_000))
    );

    let negative = unit
        .types
        .enumeration(ccc_types::EnumId(1))
        .unwrap()
        .body
        .as_ref()
        .unwrap();
    assert_eq!(negative.underlying, ccc_types::TypeId::LONG);
}

#[test]
fn completes_string_initialized_arrays_before_requiring_layout() {
    let unit =
        analyze_source("int f(void) { char mutable_copy[] = \"xy\"; return sizeof mutable_copy; }")
            .unwrap();
    let local = first_local(
        unit.functions
            .iter()
            .find(|function| function.name == "f")
            .unwrap(),
    );
    let TypeKind::Array(array) = unit.types.kind(local.ty.ty) else {
        panic!("local has array type")
    };
    assert_eq!(array.length, ArrayLength::Constant(3));
    assert!(matches!(
        local
            .initializer
            .as_ref()
            .map(|initializer| &initializer.kind),
        Some(FullTypedInitializerKind::String(_))
    ));
}

#[test]
fn types_predefined_function_names_as_unique_static_const_arrays() {
    let unused = analyze_source("int unused(void) { return 0; }").unwrap();
    assert!(unused.strings.is_empty());

    let unit = analyze_source(
        "typedef int __func__;
         const char *capture(void) {
             _Static_assert(sizeof __func__ == sizeof \"capture\", \"wrong function name\");
             static const char *saved = __func__;
             return saved == __func__ ? __func__ : 0;
         }",
    )
    .unwrap();

    assert_eq!(unit.strings.len(), 2);
    let predefined = &unit.strings[0];
    assert_eq!(
        predefined.code_units,
        b"capture\0"
            .iter()
            .copied()
            .map(u32::from)
            .collect::<Vec<_>>()
    );
    let TypeKind::Array(array) = unit.types.kind(predefined.ty.ty) else {
        panic!("the predefined function name should have array type")
    };
    assert_eq!(array.length, ArrayLength::Constant(8));
    assert_eq!(array.element.ty, TypeId::CHAR);
    assert_eq!(array.element.qualifiers, TypeQualifiers::CONST);
    assert!(unit.strings[1].code_units == predefined.code_units);
    assert!(unit.strings[1].ty != predefined.ty);

    let dump = dump_frontend_typed_ast(&unit);
    assert_eq!(
        dump.matches("PredefinedFunctionName(StringId(0))").count(),
        3,
        "{dump}"
    );
    assert!(dump.contains("array[8] of const char Lvalue"), "{dump}");
}

#[test]
fn predefined_function_names_preserve_identifier_encoding_and_scope_rules() {
    let unit = analyze_source("const char *caf\\u00e9(void) { return __func__; }").unwrap();
    assert_eq!(
        unit.strings[0].code_units,
        vec![
            u32::from(b'c'),
            u32::from(b'a'),
            u32::from(b'f'),
            0xc3,
            0xa9,
            0
        ]
    );

    assert!(analyze_source("int f(void) { __func__[0] = 'x'; return 0; }").is_err());
    assert!(analyze_source("int f(void) { char copy[] = __func__; return 0; }").is_err());
    assert!(analyze_source("int f(void) { int __func__; return 0; }").is_err());
    assert!(analyze_source("int f(void) { { int __func__ = 3; return __func__; } }").is_ok());
}

#[test]
fn ordinary_strings_initialize_all_character_array_types() {
    let unit =
        analyze_source("unsigned char bytes[] = \"xy\"; signed char signed_bytes[] = \"z\";")
            .unwrap();
    for (name, expected_element, expected_length) in [
        ("bytes", TypeId::UNSIGNED_CHAR, 3),
        ("signed_bytes", TypeId::SIGNED_CHAR, 2),
    ] {
        let global = unit
            .globals
            .iter()
            .find(|global| global.name == name)
            .unwrap();
        let TypeKind::Array(array) = unit.types.kind(global.ty.ty) else {
            panic!("{name} should have array type")
        };
        assert_eq!(array.element.ty, expected_element);
        assert_eq!(array.length, ArrayLength::Constant(expected_length));
        assert!(matches!(
            global
                .initializer
                .as_ref()
                .map(|initializer| &initializer.kind),
            Some(FullTypedInitializerKind::String(_))
        ));
    }

    assert!(diagnostic_codes("unsigned short words[] = \"xy\";").contains(&"CCC2312".to_owned()));
}

#[test]
fn string_array_initializers_may_be_enclosed_in_braces() {
    let unit = analyze_source(
        "static char output[] = { \"luac\" \".out\" };\n\
         static char extension[] = { __extension__ \"xy\" };\n\
         int f(void) { char local[2] = { (\"xy\") }; return local[0]; }",
    )
    .unwrap();

    let output = unit
        .globals
        .iter()
        .find(|global| global.name == "output")
        .unwrap();
    let TypeKind::Array(output_array) = unit.types.kind(output.ty.ty) else {
        panic!("output should have array type")
    };
    assert_eq!(output_array.length, ArrayLength::Constant(9));
    assert!(matches!(
        output
            .initializer
            .as_ref()
            .map(|initializer| &initializer.kind),
        Some(FullTypedInitializerKind::String(_))
    ));

    let extension = unit
        .globals
        .iter()
        .find(|global| global.name == "extension")
        .unwrap();
    let TypeKind::Array(extension_array) = unit.types.kind(extension.ty.ty) else {
        panic!("extension should have array type")
    };
    assert_eq!(extension_array.length, ArrayLength::Constant(3));
    assert!(matches!(
        extension
            .initializer
            .as_ref()
            .map(|initializer| &initializer.kind),
        Some(FullTypedInitializerKind::String(_))
    ));

    let local = first_local(
        unit.functions
            .iter()
            .find(|function| function.name == "f")
            .unwrap(),
    );
    let TypeKind::Array(local_array) = unit.types.kind(local.ty.ty) else {
        panic!("local should have array type")
    };
    assert_eq!(local_array.length, ArrayLength::Constant(2));
    assert!(matches!(
        local
            .initializer
            .as_ref()
            .map(|initializer| &initializer.kind),
        Some(FullTypedInitializerKind::String(_))
    ));

    let pointer_array = analyze_source("char *strings[] = { \"x\" };").unwrap();
    assert!(matches!(
        pointer_array.globals[0]
            .initializer
            .as_ref()
            .map(|initializer| &initializer.kind),
        Some(FullTypedInitializerKind::Aggregate(_))
    ));
}

#[test]
fn static_block_objects_are_data_and_require_constant_initializers() {
    let unit = analyze_source("int f(int x) { static int saved = 3; return saved + x; }").unwrap();
    let local = first_local(&unit.functions[0]);
    assert_eq!(local.duration, StorageDuration::Static);
    let emission = local
        .emission
        .as_ref()
        .expect("static local has data metadata");
    assert_eq!(emission.visibility, SymbolVisibility::Internal);
    assert_eq!(emission.definition, ObjectDefinitionPolicy::Definition);
    assert!(emission.symbol_name.contains("saved"));

    assert!(
        diagnostic_codes("int f(int x) { static int bad = x; return bad; }")
            .contains(&"CCC2367".to_owned())
    );

    let unit = analyze_source(
        "void *f(void) { static int target; static void *pointer = &target; return pointer; }",
    )
    .unwrap();
    let body = unit.functions[0].body.as_ref().unwrap();
    let FullTypedStatementKind::Compound(items) = &body.kind else {
        panic!("function body is compound")
    };
    let pointer = items
        .iter()
        .filter_map(|item| match item {
            FullTypedBlockItem::Declaration(declaration) => Some(declaration.as_ref()),
            _ => None,
        })
        .find(|declaration| declaration.name == "pointer")
        .unwrap();
    let FullTypedInitializerKind::Scalar(initializer) = &pointer.initializer.as_ref().unwrap().kind
    else {
        panic!("pointer initializer is scalar")
    };
    assert_eq!(
        initializer.constant,
        Some(ConstantValue::Address(RelocatableAddress {
            base: RelocatableBase::BlockStatic {
                function: unit.functions[0].id,
                local: FullLocalId(0),
            },
            addend: 0,
            one_past: false,
        }))
    );
}

#[test]
fn tagged_definitions_shadow_outer_tags_without_permitting_same_scope_redefinition() {
    analyze_source(
        "struct Item { int outer; };\n\
         int use(void) {\n\
             struct Item { char *inner; };\n\
             struct Item item;\n\
             item.inner = 0;\n\
             return item.inner == 0;\n\
         }\n\
         struct Item outside;",
    )
    .unwrap();

    assert!(
        diagnostic_codes(
            "int use(void) { struct Item { int first; }; struct Item { int second; }; }"
        )
        .contains(&"CCC2231".to_owned())
    );
}

#[test]
fn aggregate_initializer_paths_carry_bitfield_layout() {
    let unit = analyze_source(
        "struct Bits { unsigned prefix; unsigned value : 5; };\n\
         struct Bits bits = { .value = 7 };\n\
         unsigned offset = __builtin_offsetof(struct Bits, prefix);",
    )
    .unwrap();
    let bits = unit
        .globals
        .iter()
        .find(|global| global.name == "bits")
        .unwrap();
    let initializer = bits.initializer.as_ref().unwrap();
    let FullTypedInitializerKind::Aggregate(entries) = &initializer.kind else {
        panic!("record initializer is aggregate")
    };
    let InitializerPathElement::Field {
        bitfield: Some(bitfield),
        ..
    } = &entries[0].path[0]
    else {
        panic!("bitfield initializer has its storage descriptor")
    };
    assert_eq!(bitfield.width, 5);
    assert_eq!(bitfield.storage_offset, 0);
    assert!(bitfield.storage_size >= 1);

    let offset = unit
        .globals
        .iter()
        .find(|global| global.name == "offset")
        .unwrap();
    let FullTypedInitializerKind::Scalar(expression) = &offset.initializer.as_ref().unwrap().kind
    else {
        panic!("offset initializer is scalar")
    };
    assert_eq!(expression.constant, Some(ConstantValue::Unsigned(0)));
}

#[test]
fn aggregate_rvalue_members_retain_bitfield_layout() {
    let unit = analyze_source(
        "struct Bits { unsigned prefix; unsigned value : 6; };\n\
         struct Bits make(void);\n\
         unsigned read(void) { return make().value; }",
    )
    .unwrap();
    let read = unit
        .functions
        .iter()
        .find(|function| function.name == "read")
        .unwrap();
    let FullTypedStatementKind::Compound(items) = &read.body.as_ref().unwrap().kind else {
        panic!("function body is compound")
    };
    let FullTypedBlockItem::Statement(statement) = &items[0] else {
        panic!("function body contains a return")
    };
    let FullTypedStatementKind::Return(Some(expression)) = &statement.kind else {
        panic!("statement returns a value")
    };
    let FullTypedExpressionKind::Member { base, bitfield, .. } = &expression.kind else {
        panic!("return value is a member access")
    };
    assert_eq!(expression.category, ValueCategory::Value);
    assert!(expression.place.is_none());
    assert!(base.place.is_none());
    let bitfield = bitfield
        .as_deref()
        .expect("rvalue member retains its bitfield descriptor");
    assert_eq!(bitfield.field_index, 1);
    assert_eq!(bitfield.storage_offset, 0);
    assert_eq!(bitfield.storage_size, 4);
    assert_eq!(bitfield.width, 6);
    assert!(!bitfield.signed);
    assert!(
        dump_frontend_typed_ast(&unit)
            .contains("member #1 value indirect=false bitfield=0:4:0/6 : unsigned int Value")
    );
}

#[test]
fn resolves_members_promoted_through_nested_anonymous_records() {
    let unit = analyze_source(
        "struct Usage {
             int prefix;
             union { struct { volatile long minor; }; long alternate; };
         };
         long read(struct Usage *usage) { return usage->minor; }",
    )
    .unwrap();
    let dump = dump_frontend_typed_ast(&unit);
    assert!(
        dump.contains("member #0 minor indirect=false : volatile long int Lvalue"),
        "{dump}"
    );
    assert!(
        dump.contains("member #0 <anonymous> indirect=false"),
        "{dump}"
    );
    assert!(
        dump.contains("member #1 <anonymous> indirect=true"),
        "{dump}"
    );

    assert!(
        diagnostic_codes(
            "struct Ambiguous { struct { int value; }; union { int value; }; };
             int read(struct Ambiguous *object) { return object->value; }",
        )
        .contains(&"CCC2296".to_owned())
    );
}

#[test]
fn compound_assignment_has_a_single_evaluation_plan() {
    let unit = analyze_source("int f(int *p) { return *p += 2; }").unwrap();
    let body = unit.functions[0].body.as_ref().unwrap();
    let FullTypedStatementKind::Compound(items) = &body.kind else {
        panic!("body is compound")
    };
    let FullTypedBlockItem::Statement(statement) = &items[0] else {
        panic!("body contains return")
    };
    let FullTypedStatementKind::Return(Some(expression)) = &statement.kind else {
        panic!("statement returns an expression")
    };
    let FullTypedExpressionKind::Assignment {
        compound: Some(plan),
        value,
        ..
    } = &expression.kind
    else {
        panic!("compound assignment carries a plan")
    };
    assert_eq!(plan.operator, syntax::BinaryOperator::Add);
    assert!(!matches!(
        value.kind,
        FullTypedExpressionKind::Binary { .. }
    ));
}

#[test]
fn global_addresses_are_relocation_bearing_constants() {
    let unit = analyze_source("int target; int *pointer = &target;").unwrap();
    let target = unit
        .globals
        .iter()
        .find(|global| global.name == "target")
        .unwrap();
    let pointer = unit
        .globals
        .iter()
        .find(|global| global.name == "pointer")
        .unwrap();
    let FullTypedInitializerKind::Scalar(expression) = &pointer.initializer.as_ref().unwrap().kind
    else {
        panic!("pointer initializer is scalar")
    };
    assert_eq!(
        expression.constant,
        Some(ConstantValue::Address(RelocatableAddress {
            base: RelocatableBase::Global(target.id),
            addend: 0,
            one_past: false,
        }))
    );
}

#[test]
fn static_initializers_fold_array_addresses_decay_and_mixed_shift_counts() {
    let unit = analyze_source(
        "int values[4];\n\
         int *selected = &values[2];\n\
         const char text[] = \"named\";\n\
         const char *text_pointer = text;\n\
         unsigned long long high_bit = (unsigned long long)1 << 40;",
    )
    .unwrap();
    let global = |name: &str| {
        unit.globals
            .iter()
            .find(|global| global.name == name)
            .unwrap()
    };
    let scalar_constant = |name: &str| {
        let FullTypedInitializerKind::Scalar(expression) =
            &global(name).initializer.as_ref().unwrap().kind
        else {
            panic!("{name} has a scalar initializer")
        };
        expression.constant
    };

    assert_eq!(
        scalar_constant("selected"),
        Some(ConstantValue::Address(RelocatableAddress {
            base: RelocatableBase::Global(global("values").id),
            addend: 8,
            one_past: false,
        }))
    );
    assert_eq!(
        scalar_constant("text_pointer"),
        Some(ConstantValue::Address(RelocatableAddress {
            base: RelocatableBase::Global(global("text").id),
            addend: 0,
            one_past: false,
        }))
    );
    assert_eq!(
        scalar_constant("high_bit"),
        Some(ConstantValue::Unsigned(1_u128 << 40))
    );
}

#[test]
fn pointer_comparison_and_conditionals_merge_pointed_to_qualifiers() {
    let unit = analyze_source(
        "int compare(char *plain, const char *qualified) {\n\
             return plain == qualified && qualified == plain;\n\
         }\n\
         const char *select(int choose, char *plain, const char *qualified) {\n\
             return choose ? plain : qualified;\n\
         }\n\
         const char *reverse(int choose, char *plain, const char *qualified) {\n\
             return choose ? qualified : plain;\n\
         }",
    )
    .unwrap();

    for name in ["select", "reverse"] {
        let function = unit
            .functions
            .iter()
            .find(|function| function.name == name)
            .unwrap();
        let FullTypedStatementKind::Compound(items) = &function.body.as_ref().unwrap().kind else {
            panic!("{name} has a compound body")
        };
        let FullTypedBlockItem::Statement(statement) = &items[0] else {
            panic!("{name} contains a return statement")
        };
        let FullTypedStatementKind::Return(Some(expression)) = &statement.kind else {
            panic!("{name} returns a value")
        };
        let TypeKind::Pointer(pointer) = unit.types.kind(expression.ty.ty) else {
            panic!("{name} returns a pointer")
        };
        assert!(pointer.pointee.qualifiers.contains(TypeQualifiers::CONST));
    }
}

#[test]
fn pointer_subtraction_accepts_different_pointee_qualifiers() {
    let unit = analyze_source(
        "long difference(const char *cursor, char *start) { return cursor - start; }",
    )
    .unwrap();
    let body = unit.functions[0].body.as_ref().unwrap();
    let FullTypedStatementKind::Compound(items) = &body.kind else {
        panic!("body is compound")
    };
    let FullTypedBlockItem::Statement(statement) = &items[0] else {
        panic!("body contains a return statement")
    };
    let FullTypedStatementKind::Return(Some(expression)) = &statement.kind else {
        panic!("statement returns a value")
    };
    let FullTypedExpressionKind::Binary {
        operator: syntax::BinaryOperator::Subtract,
        left,
        right,
    } = &expression.kind
    else {
        panic!("return expression is pointer subtraction")
    };
    assert_eq!(expression.ty.ty, TypeId::LONG);
    let TypeKind::Pointer(left) = unit.types.kind(left.ty.ty) else {
        panic!("left operand is a pointer")
    };
    let TypeKind::Pointer(right) = unit.types.kind(right.ty.ty) else {
        panic!("right operand is a pointer")
    };
    assert!(left.pointee.qualifiers.contains(TypeQualifiers::CONST));
    assert!(right.pointee.qualifiers.is_empty());
    assert_eq!(left.pointee.ty, right.pointee.ty);
}

#[test]
fn pools_identical_string_literals_by_encoding_and_representation() {
    let unit = analyze_source(
        "const char *a = \"same\";\n\
         const char *b = \"same\";\n\
         const char *c = u8\"same\";\n\
         const char *d = \"different\";",
    )
    .unwrap();
    assert_eq!(unit.strings.len(), 3);
    let string_ids = unit
        .globals
        .iter()
        .map(|global| {
            let FullTypedInitializerKind::Scalar(expression) =
                &global.initializer.as_ref().unwrap().kind
            else {
                panic!("pointer initializer is scalar")
            };
            let Some(ConstantValue::Address(RelocatableAddress {
                base: RelocatableBase::String(id),
                ..
            })) = expression.constant
            else {
                panic!("pointer initializer refers to pooled string data")
            };
            id
        })
        .collect::<Vec<_>>();
    assert_eq!(string_ids[0], string_ids[1]);
    assert_ne!(string_ids[0], string_ids[2]);
    assert_ne!(string_ids[0], string_ids[3]);
}

#[test]
fn boolean_constant_conversions_normalize_scalar_values() {
    let unit = analyze_source(
        "int object;\n\
         _Bool false_value = (_Bool)0;\n\
         _Bool true_value = (_Bool)7;\n\
         _Bool null_value = (int *)0;\n\
         _Bool address_value = &object;\n\
         _Static_assert((_Bool)7 == 1, \"nonzero converts to true\");",
    )
    .unwrap();
    let constant = |name| {
        let global = unit
            .globals
            .iter()
            .find(|global| global.name == name)
            .unwrap();
        let FullTypedInitializerKind::Scalar(expression) =
            &global.initializer.as_ref().unwrap().kind
        else {
            panic!("expected a scalar initializer");
        };
        expression.constant
    };

    assert_eq!(constant("false_value"), Some(ConstantValue::Unsigned(0)));
    assert_eq!(constant("true_value"), Some(ConstantValue::Unsigned(1)));
    assert_eq!(constant("null_value"), Some(ConstantValue::Unsigned(0)));
    assert_eq!(constant("address_value"), Some(ConstantValue::Unsigned(1)));
}

#[test]
fn packing_stack_is_ordered_and_named() {
    let mut parsed = parse_source(
        "struct Packed { char c; int value; };\n\
         struct Native { char c; int value; };",
    );
    let mut sources = SourceMap::new();
    let file = sources.add_file("pack.c", "(push,wire,1) (pop,wire)");
    let tokens = lex(file, sources.source(file).unwrap()).unwrap();
    let split = tokens
        .iter()
        .position(|token| token.spelling == "(" && token.span.start > 0)
        .unwrap();
    let push = tokens[..split].to_vec();
    let pop = tokens[split..].to_vec();
    let span = tokens[0].span;
    let declarations = std::mem::take(&mut parsed.items);
    parsed.items = vec![
        ExternalItem::Pragma(PragmaEvent::Pack {
            payload: push,
            span,
        }),
        declarations[0].clone(),
        ExternalItem::Pragma(PragmaEvent::Pack { payload: pop, span }),
        declarations[1].clone(),
    ];
    let unit = analyze_frontend(&parsed, &EffectiveCompilationConfig::default()).unwrap();
    let record_types = unit
        .external_items
        .iter()
        .filter_map(|item| match item {
            FullTypedExternalItem::TypeDeclaration { ty, .. } => Some(*ty),
            _ => None,
        })
        .collect::<Vec<_>>();
    let packed = unit
        .types
        .layout_of(record_types[0], &EffectiveCompilationConfig::default())
        .unwrap();
    let native = unit
        .types
        .layout_of(record_types[1], &EffectiveCompilationConfig::default())
        .unwrap();
    let ccc_types::LayoutShape::Record(packed) = packed.shape else {
        panic!("packed type is a record")
    };
    let ccc_types::LayoutShape::Record(native) = native.shape else {
        panic!("native type is a record")
    };
    assert_eq!(packed.fields[1].offset, 1);
    assert_eq!(native.fields[1].offset, 4);
}

#[test]
fn pack_zero_restores_native_alignment() {
    let unit = analyze_preprocessed_source(
        "pack-zero.c",
        "#pragma pack(1)\n\
         struct Packed { char c; int value; };\n\
         #pragma pack(0)\n\
         struct Native { char c; int value; };",
    )
    .unwrap();
    let record_types = unit
        .external_items
        .iter()
        .filter_map(|item| match item {
            FullTypedExternalItem::TypeDeclaration { ty, .. } => Some(*ty),
            _ => None,
        })
        .collect::<Vec<_>>();
    let config = EffectiveCompilationConfig::default();
    let packed = unit.types.layout_of(record_types[0], &config).unwrap();
    let native = unit.types.layout_of(record_types[1], &config).unwrap();
    let ccc_types::LayoutShape::Record(packed) = packed.shape else {
        panic!("packed type is a record")
    };
    let ccc_types::LayoutShape::Record(native) = native.shape else {
        panic!("native type is a record")
    };
    assert_eq!(packed.fields[1].offset, 1);
    assert_eq!(native.fields[1].offset, 4);
}

#[test]
fn packed_attributes_and_flexible_array_members_use_the_target_layout() {
    let unit = analyze_source(
        "struct __attribute__((packed)) Packed { char tag; int value; };
         struct __attribute__((__packed__)) PackedAlias { char tag; int value; };
         struct Packet { unsigned length; int values[]; };
         static const int first = 1, __attribute__((deprecated)) second = 2;
         int read(struct Packet *packet, int index) { return packet->values[index]; }",
    )
    .unwrap();
    let config = EffectiveCompilationConfig::default();
    let record_types = unit
        .external_items
        .iter()
        .filter_map(|item| match item {
            FullTypedExternalItem::TypeDeclaration { ty, .. } => Some(*ty),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(record_types.len(), 3);
    for ty in &record_types[..2] {
        let layout = unit.types.layout_of(*ty, &config).unwrap();
        assert_eq!(layout.size, 5);
        let ccc_types::LayoutShape::Record(layout) = layout.shape else {
            panic!("packed type is a record")
        };
        assert_eq!(layout.fields[1].offset, 1);
    }
    let flexible = unit.types.layout_of(record_types[2], &config).unwrap();
    assert_eq!(flexible.size, 4);
    let ccc_types::LayoutShape::Record(flexible) = flexible.shape else {
        panic!("flexible type is a record")
    };
    assert_eq!(flexible.fields[1].offset, 4);
    assert_eq!(flexible.fields[1].size, 0);

    for source in [
        "union Invalid { int tag; int values[]; };",
        "struct Invalid { int values[]; int tail; };",
        "struct Invalid { int values[]; };",
    ] {
        assert_eq!(diagnostic_codes(source), vec!["CCC2370"]);
    }
    assert_eq!(
        diagnostic_codes(
            "struct Packet { int length; int values[]; };
             struct Packet packet = { 1, { 2 } };"
        ),
        vec!["CCC2431"]
    );
    for source in [
        "struct Native { char tag; int value __attribute__((packed)); };",
        "int object __attribute__((packed));",
        "enum __attribute__((packed)) Invalid { value };",
    ] {
        assert_eq!(diagnostic_codes(source), vec!["CCC2432"]);
    }
}

#[test]
fn flexible_array_members_follow_containment_and_anonymous_member_constraints() {
    analyze_source(
        "struct WithAnonymousStruct {
             struct { int promoted; };
             int values[];
         };
         struct WithAnonymousUnion {
             union { int first; long second; };
             int values[];
         };
         struct Packet { int length; int values[]; };
         union PacketHolder { struct Packet packet; long alignment; };",
    )
    .unwrap();

    for source in [
        "struct Packet { int length; int values[]; };
         struct Invalid { struct Packet packet; };",
        "struct Packet { int length; int values[]; };
         union PacketHolder { struct Packet packet; long alignment; };
         struct Invalid { union PacketHolder holder; };",
        "struct Packet { int length; int values[]; };
         union PacketHolder { struct Packet packet; long alignment; };
         union NestedHolder { union PacketHolder holder; long alignment; };
         struct Invalid { union NestedHolder holder; };",
        "struct Packet { int length; int values[]; };
         struct Packet packets[2];",
        "struct Packet { int length; int values[]; };
         void consume(struct Packet packets[2]);",
        "struct Packet { int length; int values[]; };
         typedef struct Packet PacketArray[2];",
    ] {
        assert_eq!(diagnostic_codes(source), vec!["CCC2370"], "{source}");
    }
}

#[test]
fn leading_attributes_on_later_declarators_affect_their_types() {
    analyze_source(
        "int first, __attribute__((mode(word))) second;
         _Static_assert(sizeof second == sizeof(void *), \"file mode\");
         int check(void) {
             int first, __attribute__((mode(word))) second;
             _Static_assert(sizeof second == sizeof(void *), \"block mode\");
             return sizeof first == sizeof second;
         }",
    )
    .unwrap();
}

#[test]
fn no_argument_attributes_reject_argument_lists() {
    for source in [
        "struct __attribute__((packed(8))) Invalid { char tag; int value; };",
        "struct __attribute__((__packed__(8))) Invalid { char tag; int value; };",
        "int value __attribute__((unused(1)));",
        "int value __attribute__((__unused__(1)));",
    ] {
        assert_eq!(diagnostic_codes(source), vec!["CCC2435"], "{source}");
    }
}

#[test]
fn rejects_unsupported_semantics_and_storage_but_allows_long_double_layout() {
    assert!(
        diagnostic_codes("int f(int n) { int values[n]; return 0; }")
            .contains(&"CCC2258".to_owned())
    );
    assert!(analyze_source("unsigned long size = sizeof(long double);").is_ok());
    assert!(analyze_source("unsigned long alignment = __alignof__(long double);").is_ok());
    assert!(
        diagnostic_codes("long double f(long double x) { return x + 1.0L; }")
            .contains(&"CCC2343".to_owned())
    );

    let parsed = parse_source("int value __attribute__((aligned(8))); ");
    let mut config = EffectiveCompilationConfig::default();
    config.capabilities.insert(
        CapabilityKind::Attribute,
        "aligned",
        CapabilityState::ParseOnly,
    );
    let diagnostics = analyze_frontend(&parsed, &config).unwrap_err();
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "CCC2345")
    );
    let parsed = parse_source("extern int value __asm__(\"renamed\");");
    let mut config = EffectiveCompilationConfig::default();
    config.capabilities.insert(
        CapabilityKind::Extension,
        "gnu-declaration-asm-labels",
        CapabilityState::ParseOnly,
    );
    let diagnostics = analyze_frontend(&parsed, &config).unwrap_err();
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "CCC2346")
    );
    assert!(diagnostic_codes("__thread int value;").contains(&"CCC2374".to_owned()));
}

#[test]
fn full_typed_dump_is_independent_of_source_offsets() {
    let first = analyze_source(
        "struct { int member; } object = { .member = 1 };\n\
         int read(void) { return object.member; }",
    )
    .unwrap();
    let second = analyze_source(
        "\n\n  struct { int member; } object = { .member = 1 };\n\n\
         int read(void)\n{ return object.member; }",
    )
    .unwrap();
    assert_eq!(
        dump_frontend_typed_ast(&first),
        dump_frontend_typed_ast(&second)
    );
}

#[test]
fn complex_full_typed_dump_matches_the_committed_golden() {
    let unit = analyze_preprocessed_source(
        "complex-typed-ast.c",
        include_str!("../../../../tests/frontend/typed-ast/complex.c"),
    )
    .unwrap();
    let dump = dump_frontend_typed_ast(&unit);
    assert!(!dump.contains("complex-typed-ast.c"), "{dump}");
    assert!(!dump.contains("Span {"), "{dump}");
    assert!(!dump.contains("offset:"), "{dump}");
    assert_eq!(
        dump,
        include_str!("../../../../tests/frontend/goldens/typed-ast-complex.out")
    );
}

#[test]
fn linkage_and_control_flow_typed_dump_matches_the_committed_golden() {
    let unit = analyze_preprocessed_source(
        "linkage-control-typed-ast.c",
        include_str!("../../../../tests/frontend/typed-ast/linkage-control.c"),
    )
    .unwrap();
    let dump = dump_frontend_typed_ast(&unit);
    assert!(!dump.contains("linkage-control-typed-ast.c"), "{dump}");
    assert!(!dump.contains("Span {"), "{dump}");
    assert!(!dump.contains("offset:"), "{dump}");
    assert_eq!(
        dump,
        include_str!("../../../../tests/frontend/goldens/typed-ast-linkage-control.out")
    );
}

#[test]
fn variadic_promotions_typed_dump_matches_the_committed_golden() {
    let unit = analyze_preprocessed_source(
        "variadic-promotions.c",
        include_str!("../../../../tests/frontend/typed-ast/variadic-promotions.c"),
    )
    .unwrap();
    assert_eq!(
        dump_frontend_typed_ast(&unit),
        include_str!("../../../../tests/frontend/goldens/typed-ast-variadic-promotions.out")
    );
}

#[test]
fn comma_values_typed_dump_matches_the_committed_golden() {
    let unit = analyze_preprocessed_source(
        "comma-values.c",
        include_str!("../../../../tests/frontend/typed-ast/comma-values.c"),
    )
    .unwrap();
    assert_eq!(
        dump_frontend_typed_ast(&unit),
        include_str!("../../../../tests/frontend/goldens/typed-ast-comma-values.out")
    );
    assert_eq!(
        diagnostic_codes(
            "struct Pair { int value; };\n\
             int invalid(struct Pair first, struct Pair second) {\n\
                 (first, second) = first;\n\
                 return 0;\n\
             }"
        ),
        vec!["CCC2283"]
    );
}

#[test]
fn target_char_signedness_reaches_bitfield_metadata() {
    let parsed = parse_source(
        "struct CharacterBits { char value : 3; };\n\
         struct CharacterBits bits = { .value = 1 };\n\
         char converted = (char)255;",
    );
    let mut config = EffectiveCompilationConfig::default();
    config.target.data_layout.char_is_signed = false;
    let unit = analyze_frontend(&parsed, &config).unwrap();
    let initializer = unit.globals[0].initializer.as_ref().unwrap();
    let FullTypedInitializerKind::Aggregate(entries) = &initializer.kind else {
        panic!("initializer is aggregate")
    };
    let InitializerPathElement::Field {
        bitfield: Some(bitfield),
        ..
    } = &entries[0].path[0]
    else {
        panic!("initializer selects a bitfield")
    };
    assert!(!bitfield.signed);

    let converted = unit
        .globals
        .iter()
        .find(|global| global.name == "converted")
        .unwrap();
    let FullTypedInitializerKind::Scalar(expression) =
        &converted.initializer.as_ref().unwrap().kind
    else {
        panic!("converted char has a scalar initializer")
    };
    assert_eq!(expression.constant, Some(ConstantValue::Unsigned(255)));
}

#[test]
fn ordinary_character_constants_follow_target_char_signedness() {
    let source = "int value = '\\xff';";
    let signed = analyze_source(source).unwrap();
    let signed_value = signed.globals[0].initializer.as_ref().unwrap();
    let FullTypedInitializerKind::Scalar(signed_expression) = &signed_value.kind else {
        panic!("character initializer is scalar")
    };
    assert_eq!(signed_expression.constant, Some(ConstantValue::Signed(-1)));

    let mut config = EffectiveCompilationConfig::default();
    config.target.data_layout.char_is_signed = false;
    let unsigned = analyze_source_with_config(source, &config).unwrap();
    let unsigned_value = unsigned.globals[0].initializer.as_ref().unwrap();
    let FullTypedInitializerKind::Scalar(unsigned_expression) = &unsigned_value.kind else {
        panic!("character initializer is scalar")
    };
    assert_eq!(
        unsigned_expression.constant,
        Some(ConstantValue::Unsigned(255))
    );
}
