use ccc_pp::{
    FsFileProvider, PpItem, PragmaEvent, PreprocessContext, PreprocessOptions, VecDiagnosticSink,
    lex, preprocess,
};
use ccc_session::{Session, SourceMap};
use ccc_syntax::frontend::{self as syntax, ExternalItem};
use ccc_target::{CapabilityKind, CapabilityState, EffectiveCompilationConfig};
use ccc_types::{ArrayLength, TypeKind, TypeQualifiers};

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
    assert!(diagnostic_codes("int value __asm__(\"renamed\");").contains(&"CCC2346".to_owned()));
    assert!(diagnostic_codes("__thread int value;").contains(&"CCC2374".to_owned()));
    assert!(
        diagnostic_codes("struct Packet { unsigned long length; char bytes[]; };")
            .contains(&"CCC2370".to_owned())
    );
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
