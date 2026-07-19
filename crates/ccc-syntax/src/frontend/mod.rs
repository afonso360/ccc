//! C syntax tree and parser over the ordered preprocessing output.
//!
//! Parsing preserves declaration ambiguity until semantic analysis has the
//! symbol-table context needed to resolve it.

use ccc_session::Span;

mod ast;
mod dump;
mod names;
mod parser;
mod token;

pub use ast::*;
pub use dump::*;
pub use names::*;
pub use parser::*;
pub use token::*;

fn span_through(start: Span, end: Span) -> Span {
    if start.file == end.file {
        Span::with_origin(start.file, start.start, end.end, start.origin)
    } else {
        start
    }
}

#[cfg(test)]
mod tests {
    use ccc_pp::{LanguageMode, PpItem, PpTokenKind, PragmaEvent, lex};
    use ccc_session::SourceMap;

    use super::*;

    fn converted(source: &str) -> Vec<FrontendItem> {
        let mut sources = SourceMap::new();
        let file = sources.add_file("frontend-test.c", source);
        let tokens = lex(file, sources.source(file).unwrap()).unwrap();
        convert_pp_items(tokens.into_iter().map(PpItem::Token)).unwrap()
    }

    fn parse_source(source: &str) -> Result<TranslationUnit, ParseError> {
        parse(&converted(source))
    }

    fn parse_source_with_mode(
        source: &str,
        mode: LanguageMode,
    ) -> Result<TranslationUnit, ParseError> {
        parse_with_mode(&converted(source), mode)
    }

    #[test]
    fn conversion_decodes_numbers_and_concatenates_strings() {
        let items = converted("int *s = \"left\" u\" right\"; double x = 0x1.8p+2;");
        let strings = items
            .iter()
            .filter_map(|item| match item {
                FrontendItem::Token(Token {
                    kind: TokenKind::String(literal),
                    ..
                }) => Some(literal),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(strings.len(), 1);
        assert_eq!(strings[0].prefix, ccc_pp::StringLiteralPrefix::Utf16);
        assert!(items.iter().any(|item| matches!(
            item,
            FrontendItem::Token(Token {
                kind: TokenKind::Floating(value),
                ..
            }) if value.number == "0x1.8p+2"
        )));
    }

    #[test]
    fn preserves_pragma_events_in_translation_unit_order() {
        let mut sources = SourceMap::new();
        let file = sources.add_file("pragma.c", "int before; int after;");
        let tokens = lex(file, sources.source(file).unwrap()).unwrap();
        let split = tokens
            .iter()
            .position(|token| token.spelling == ";")
            .unwrap()
            + 1;
        let pack_tokens = lex(file, "(push, 1)").unwrap();
        let mut items = tokens[..split]
            .iter()
            .cloned()
            .map(PpItem::Token)
            .collect::<Vec<_>>();
        items.push(PpItem::Pragma(PragmaEvent::Pack {
            payload: pack_tokens,
            span: Span::new(file, 4, 10),
        }));
        items.extend(tokens[split..].iter().cloned().map(PpItem::Token));
        let unit = parse(&convert_pp_items(items).unwrap()).unwrap();
        assert!(matches!(unit.items[0], ExternalItem::Declaration(_)));
        assert!(matches!(
            unit.items[1],
            ExternalItem::Pragma(PragmaEvent::Pack { .. })
        ));
        assert!(matches!(unit.items[2], ExternalItem::Declaration(_)));
    }

    #[test]
    fn optimization_pragma_may_separate_declaration_tokens() {
        let mut sources = SourceMap::new();
        let file = sources.add_file("pragma.c", "int function(void) { return 0; }");
        let tokens = lex(file, sources.source(file).unwrap()).unwrap();
        let mut items = vec![PpItem::Token(tokens[0].clone())];
        items.push(PpItem::Pragma(PragmaEvent::GccOptimize {
            payload: Vec::new(),
            span: tokens[0].span,
        }));
        items.extend(tokens[1..].iter().cloned().map(PpItem::Token));

        let unit = parse(&convert_pp_items(items).unwrap()).unwrap();
        assert!(matches!(
            unit.items.as_slice(),
            [ExternalItem::FunctionDefinition(_)]
        ));
    }

    #[test]
    fn parses_recursive_declarators_records_and_initializers() {
        let unit = parse_source(
            "typedef unsigned long size_t;\n\
             struct Node { volatile unsigned bits:3; struct Node *next; };\n\
             int (*handler)(const char *, ...);\n\
             struct Node nodes[2] = {[1].bits = 3, [0].next = &nodes[1]};",
        )
        .unwrap();
        assert_eq!(unit.items.len(), 4);
        let ExternalItem::Declaration(handler) = &unit.items[2] else {
            panic!("handler should be a declaration");
        };
        assert_eq!(
            handler.declarators[0].declarator.identifier().unwrap().name,
            "handler"
        );
    }

    #[test]
    fn distinguishes_unspecified_parameters_from_an_empty_prototype() {
        let unit = parse_source("int unspecified(); int prototype(void);").unwrap();
        let function_declarator = |index| {
            let ExternalItem::Declaration(declaration) = &unit.items[index] else {
                panic!("expected a declaration");
            };
            let DirectDeclarator::Function {
                parameters,
                has_parameter_type_list,
                ..
            } = &declaration.declarators[0].declarator.direct
            else {
                panic!("expected a function declarator");
            };
            (parameters, has_parameter_type_list)
        };

        let (unspecified_parameters, unspecified_has_type_list) = function_declarator(0);
        assert!(unspecified_parameters.is_empty());
        assert!(!unspecified_has_type_list);

        let (prototype_parameters, prototype_has_type_list) = function_declarator(1);
        assert_eq!(prototype_parameters.len(), 1);
        assert!(prototype_has_type_list);

        let dump = dump_ast(&unit);
        assert!(
            dump.contains("declarator unspecified(unspecified)"),
            "{dump}"
        );
        assert!(dump.contains("declarator prototype(1)"), "{dump}");
    }

    #[test]
    fn accepts_empty_file_scope_declarations_only_in_gnu_mode() {
        let unit = parse_source_with_mode("; int value; ;", LanguageMode::Gnu11).unwrap();
        assert_eq!(unit.items.len(), 1);
        assert!(matches!(unit.items[0], ExternalItem::Declaration(_)));

        let error = parse_source_with_mode(";", LanguageMode::C11).unwrap_err();
        assert_eq!(error.code, "CCC1020");
    }

    #[test]
    fn parses_unnamed_array_parameters() {
        let unit = parse_source("extern char *tmpnam(char[20]);").unwrap();
        let ExternalItem::Declaration(declaration) = &unit.items[0] else {
            panic!("expected a declaration");
        };
        let DirectDeclarator::Function { parameters, .. } =
            &declaration.declarators[0].declarator.direct
        else {
            panic!("expected a function declarator");
        };
        let DirectDeclarator::Array { inner, .. } =
            &parameters[0].declarator.as_ref().unwrap().direct
        else {
            panic!("expected an unnamed array declarator");
        };
        assert!(matches!(inner.as_ref(), DirectDeclarator::Abstract(_)));
    }

    #[test]
    fn parses_remaining_statements_and_expression_operators() {
        parse_source(
            "int f(int n) {\n\
                 int x = 0;\n\
                 for (int i = 0; i < n; i++) {\n\
                     switch (i) { case 1: continue; default: x += (i << 1) | 1; }\n\
                 }\n\
                 do { x--; if (!x) break; } while (x);\n\
             again: if (x ? x : n) goto again;\n\
                 return x;\n\
             }",
        )
        .unwrap();
    }

    #[test]
    fn parses_postfix_operators_after_compound_literals() {
        let unit = parse_source(
            "struct Pair { int left; int right; };\n\
             int read(void) {\n\
                 return (struct Pair){.left = 1, .right = 2}.right\n\
                     + (int[]){3, 4}[1];\n\
             }",
        )
        .unwrap();
        let dump = dump_ast(&unit);
        assert_eq!(dump.matches("compound-literal").count(), 2, "{dump}");
        assert!(dump.contains("member right"), "{dump}");
        assert!(dump.contains("subscript"), "{dump}");
    }

    #[test]
    fn parses_computed_goto_and_label_addresses_exactly_in_gnu_mode() {
        let source = "int dispatch(int opcode) {\n\
             static const void *const table[2] = {&&zero, &&one};\n\
             goto *table[opcode];\n\
         zero: return 10;\n\
         one: return 20;\n\
         }";
        let unit = parse_source_with_mode(source, LanguageMode::Gnu11).unwrap();
        assert_eq!(
            dump_ast(&unit),
            concat!(
                "translation-unit\n",
                "  function-definition dispatch\n",
                "    type Int\n",
                "    declarator dispatch(1)\n",
                "    compound\n",
                "      declaration\n",
                "        storage Static\n",
                "        qualifier Const\n",
                "        type Void\n",
                "        declarator *table[]\n",
                "          initializer-list\n",
                "            initializer-entry\n",
                "              label-address zero\n",
                "            initializer-entry\n",
                "              label-address one\n",
                "      computed-goto\n",
                "        subscript\n",
                "          name table\n",
                "          name opcode\n",
                "      label\n",
                "        return\n",
                "          integer 10\n",
                "      label\n",
                "        return\n",
                "          integer 20\n",
            )
        );

        assert!(parse_source_with_mode(source, LanguageMode::C11).is_err());
        assert!(
            parse_source_with_mode(
                "int f(void) { return &&done != 0; done: return 0; }",
                LanguageMode::C11
            )
            .is_err()
        );
    }

    #[test]
    fn parses_standard_digraph_punctuators() {
        let unit = parse_source(
            "int values<:2:> = <% 3, 4 %>;\n\
             int read(void) <% return values<:1:>; %>",
        )
        .unwrap();
        assert_eq!(unit.items.len(), 2);
        let dump = dump_ast(&unit);
        assert!(dump.contains("declarator values[]"), "{dump}");
        assert!(dump.contains("subscript"), "{dump}");
    }

    #[test]
    fn point_of_declaration_hides_a_typedef_before_its_initializer() {
        let unit =
            parse_source("typedef int T; int f(void) { int T = sizeof(T); return T; }").unwrap();
        let ExternalItem::FunctionDefinition(function) = &unit.items[1] else {
            panic!("expected a function definition");
        };
        let StatementKind::Compound(items) = &function.body.kind else {
            panic!("expected a compound body");
        };
        let BlockItem::Declaration(declaration) = &items[0] else {
            panic!("expected a declaration");
        };
        let Some(Initializer::Expression(initializer)) = &declaration.declarators[0].initializer
        else {
            panic!("expected an initializer");
        };
        assert!(matches!(
            initializer.kind,
            ExpressionKind::SizeofExpression(_)
        ));
    }

    #[test]
    fn predefined_function_name_hides_a_file_scope_typedef() {
        let unit =
            parse_source("typedef int __func__; int named(void) { return sizeof(__func__); }")
                .unwrap();
        let ExternalItem::FunctionDefinition(function) = &unit.items[1] else {
            panic!("expected a function definition");
        };
        let StatementKind::Compound(items) = &function.body.kind else {
            panic!("expected a compound body");
        };
        let BlockItem::Statement(statement) = &items[0] else {
            panic!("expected a return statement");
        };
        let StatementKind::Return(Some(expression)) = &statement.kind else {
            panic!("expected a return expression");
        };
        assert!(matches!(
            expression.kind,
            ExpressionKind::SizeofExpression(_)
        ));
        assert!(unit.scope_events.iter().any(|event| {
            event.depth == 1
                && matches!(
                    &event.kind,
                    ScopeEventKind::Bind {
                        name,
                        class: NameClass::Ordinary,
                    } if name == "__func__"
                )
        }));
    }

    #[test]
    fn name_class_transactions_restore_bindings_and_events() {
        let mut sources = SourceMap::new();
        let file = sources.add_file("scope.c", "T");
        let span = Span::new(file, 0, 1);
        let mut names = NameClassEnv::new();
        names.bind("T", NameClass::TypedefName, span);
        let checkpoint = names.checkpoint();
        names.enter_scope(ScopeKind::Block, span);
        names.bind("T", NameClass::Ordinary, span);
        assert_eq!(names.lookup("T"), Some(NameClass::Ordinary));
        names.rollback(checkpoint);
        assert_eq!(names.lookup("T"), Some(NameClass::TypedefName));
        assert_eq!(names.events().len(), 1);
    }

    #[test]
    fn parses_gnu_declaration_surface_and_preserves_spellings() {
        let unit = parse_source(
            "__extension__ extern __typeof__(target) *f(int *__restrict__)\n\
             __asm__(\"f_impl\") __attribute__((__nothrow__, aligned(8)));",
        )
        .unwrap();
        let ExternalItem::Declaration(declaration) = &unit.items[0] else {
            panic!("expected a declaration");
        };
        assert!(declaration.specifiers.extension);
        let init = &declaration.declarators[0];
        assert_eq!(init.asm_label.as_ref().unwrap().keyword_spelling, "__asm__");
        assert_eq!(
            init.asm_label.as_ref().unwrap().literal_spelling,
            "\"f_impl\""
        );
        assert_eq!(init.attributes[0].introducer, "__attribute__");
        assert_eq!(init.attributes[0].name.name, "__nothrow__");
    }

    #[test]
    fn parses_and_preserves_complete_gnu_asm_statements() {
        let unit = parse_source(
            "int f(int index, int limit, int backup) {\n\
                 int candidate = 1;\n\
                 __asm__ __volatile__ __inline__ goto (\"cmp %[limit], %[index]\\n\\t\" \"cmova %[backup], %[candidate]\"\n\
                     : [candidate] \"+r\" (candidate)\n\
                     : [index] \"r\" (index), [limit] \"r\" (limit), [backup] \"r\" (backup)\n\
                     : \"cc\", \"memory\" : done);\n\
             done: return candidate;\n\
             }",
        )
        .unwrap();
        let ExternalItem::FunctionDefinition(function) = &unit.items[0] else {
            panic!("expected function definition");
        };
        let StatementKind::Compound(items) = &function.body.kind else {
            panic!("expected compound statement");
        };
        let BlockItem::Statement(statement) = &items[1] else {
            panic!("expected asm statement");
        };
        let StatementKind::Asm(asm) = &statement.kind else {
            panic!("expected asm statement");
        };
        assert_eq!(asm.keyword_spelling, "__asm__");
        assert_eq!(asm.qualifiers.len(), 3);
        assert_eq!(asm.outputs.len(), 1);
        assert_eq!(asm.inputs.len(), 3);
        assert_eq!(asm.clobbers.len(), 2);
        assert_eq!(asm.goto_labels[0].name, "done");
        assert_eq!(asm.colon_group_count, 4);
        assert_eq!(asm.outputs[0].constraint.spelling, "\"+r\"");
        assert_eq!(
            asm.outputs[0].symbolic_name.as_ref().unwrap().name,
            "candidate"
        );
        assert!(dump_ast(&unit).contains("asm-statement"));
    }

    #[test]
    fn parses_basic_and_empty_group_asm_statements() {
        let unit = parse_source(
            "void f(long *field, long value) {\n\
                 asm(\"nop\");\n\
                 asm volatile (\"\" ::: \"memory\");\n\
                 __asm__(\"lock; xchgq %0, %1\" : \"+q\"(value), \"+m\"(*field));\n\
             }",
        )
        .unwrap();
        let dump = dump_ast(&unit);
        assert_eq!(dump.matches("asm-statement").count(), 3, "{dump}");
        assert!(dump.contains("colon-groups=3"), "{dump}");
    }

    #[test]
    fn rejects_malformed_gnu_asm_operands() {
        for source in [
            "void f(void) { asm(); }",
            "void f(int x) { asm(\"\" : +r(x)); }",
            "void f(int x) { asm(\"\" : \"+r\" x); }",
            "void f(void) { asm(\"\" ::: memory); }",
            "void f(void) { asm goto (\"\" :::: 1); }",
        ] {
            assert!(parse_source(source).is_err(), "accepted {source}");
        }
    }

    #[test]
    fn parses_enumerator_attributes_before_and_after_the_enumerator() {
        let unit = parse_source(
            "enum State {
                 __attribute__((deprecated)) old_state = 1,
                 current_state __attribute__((deprecated)) = 2
             };",
        )
        .unwrap();
        let dump = dump_ast(&unit);
        assert_eq!(
            dump.matches("attribute __attribute__ deprecated").count(),
            2
        );
        assert!(dump.contains("enumerator old_state"), "{dump}");
        assert!(dump.contains("enumerator current_state"), "{dump}");

        let ExternalItem::Declaration(declaration) = &unit.items[0] else {
            panic!("expected an enum declaration")
        };
        let enumeration = declaration
            .specifiers
            .items
            .iter()
            .find_map(|specifier| match specifier {
                DeclarationSpecifier::Type(TypeSpecifier::Enum(enumeration)) => Some(enumeration),
                _ => None,
            })
            .unwrap();
        let enumerators = enumeration.enumerators.as_ref().unwrap();
        assert_eq!(
            enumerators[0].span.start,
            enumerators[0].attributes[0].span.start
        );
        assert_eq!(
            enumerators[0].span.end,
            enumerators[0].value.as_ref().unwrap().span.end
        );
        assert_eq!(
            enumerators[1].span.end,
            enumerators[1].value.as_ref().unwrap().span.end
        );
    }

    #[test]
    fn parses_attributes_before_later_init_declarators() {
        let unit = parse_source(
            "static const int first = 1,
                 __attribute__((deprecated)) second = 2,
                 third = 3;",
        )
        .unwrap();
        let dump = dump_ast(&unit);
        assert!(dump.contains("declarator first"), "{dump}");
        assert!(dump.contains("declarator third"), "{dump}");

        let ExternalItem::Declaration(declaration) = &unit.items[0] else {
            panic!("expected a declaration")
        };
        let second = &declaration.declarators[1];
        assert_eq!(second.declarator.attributes.len(), 1);
        assert_eq!(second.declarator.attributes[0].name.name, "deprecated");
        assert!(second.attributes.is_empty());
        assert_eq!(
            second.declarator.span.start,
            second.declarator.attributes[0].span.start
        );
    }

    #[test]
    fn plain_gnu_keywords_are_mode_dependent_but_reserved_alternatives_are_not() {
        let plain = "extern typeof(target) f(void) asm(\"f_impl\");";
        parse_source_with_mode(plain, LanguageMode::Gnu11).unwrap();
        assert!(parse_source_with_mode(plain, LanguageMode::C11).is_err());

        let reserved = "extern __typeof__(target) f(void) __asm__(\"f_impl\");";
        parse_source_with_mode(reserved, LanguageMode::Gnu11).unwrap();
        parse_source_with_mode(reserved, LanguageMode::C11).unwrap();
    }

    #[test]
    fn parses_builtin_offsetof_member_designator() {
        let unit = parse_source(
            "struct Outer { struct { int b; } a[3]; };\n\
             unsigned long offset = __builtin_offsetof(struct Outer, a[2].b);",
        )
        .unwrap();
        let ExternalItem::Declaration(declaration) = &unit.items[1] else {
            panic!("expected offset declaration");
        };
        let Some(Initializer::Expression(expression)) = &declaration.declarators[0].initializer
        else {
            panic!("expected offset initializer");
        };
        let ExpressionKind::BuiltinOffsetof { designator, .. } = &expression.kind else {
            panic!("expected dedicated offsetof syntax");
        };
        assert_eq!(designator.len(), 3);
        assert!(matches!(designator[0], OffsetDesignator::Member(_)));
        assert!(matches!(designator[1], OffsetDesignator::Index(_)));
        assert!(matches!(designator[2], OffsetDesignator::Member(_)));
    }

    #[test]
    fn parses_target_va_list_and_typed_variadic_builtins() {
        let unit = parse_source(
            "typedef __builtin_va_list va_list;\n\
             struct Pair { int left; int right; };\n\
             int read(int count, ...) {\n\
                 va_list list, copy;\n\
                 __builtin_va_start(list, count);\n\
                 __builtin_va_copy(copy, list);\n\
                 struct Pair pair = __builtin_va_arg(copy, struct Pair);\n\
                 __builtin_va_end(copy);\n\
                 __builtin_va_end(list);\n\
                 return pair.left;\n\
             }",
        )
        .unwrap();
        let dump = dump_ast(&unit);
        assert!(dump.contains("type __builtin_va_list"), "{dump}");
        assert!(dump.contains("builtin-va-start"), "{dump}");
        assert!(dump.contains("builtin-va-arg"), "{dump}");
        assert!(dump.contains("builtin-va-copy"), "{dump}");
        assert_eq!(dump.matches("builtin-va-end").count(), 2, "{dump}");
    }

    #[test]
    fn parses_compiler_128_bit_integer_spellings_as_types() {
        let unit = parse_source(
            "__int128 signed_value;\n\
             signed __int128 explicit_signed;\n\
             unsigned __int128 unsigned_value;\n\
             __int128_t signed_alias;\n\
             __uint128_t unsigned_alias;\n\
             struct Wide { __uint128_t words[4]; };",
        )
        .unwrap();
        let dump = dump_ast(&unit);
        assert_eq!(dump.matches("type __int128\n").count(), 3, "{dump}");
        assert!(dump.contains("type __int128_t"), "{dump}");
        assert_eq!(dump.matches("type __uint128_t").count(), 2, "{dump}");
    }

    #[test]
    fn parses_float16_as_a_reserved_arithmetic_type() {
        let unit = parse_source(
            "_Float16 object;\n\
             extern _Float16 operation(_Float16);\n\
             struct Pair { _Float16 first; _Float16 second; };",
        )
        .unwrap();
        let dump = dump_ast(&unit);
        assert_eq!(dump.matches("type _Float16").count(), 4, "{dump}");
    }

    #[test]
    fn parses_sync_synchronize_only_with_exact_zero_argument_syntax() {
        let unit = parse_source("void synchronize(void) { __sync_synchronize(); }").unwrap();
        let dump = dump_ast(&unit);
        assert!(dump.contains("builtin-sync-synchronize"), "{dump}");

        for source in [
            "void synchronize(void) { __sync_synchronize(1); }",
            "void synchronize(void) { __sync_synchronize; }",
        ] {
            let error = parse_source(source).unwrap_err();
            assert!(
                error.message.contains("__sync_synchronize"),
                "{source}: {error}"
            );
        }
    }

    #[test]
    fn parses_legacy_sync_operations_and_optional_protected_operands() {
        let unit = parse_source(
            "int value; void *pointer;\n\
             int update(int delta) {\n\
                 int old = __sync_fetch_and_add(&value, delta);\n\
                 int now = __sync_add_and_fetch(&value, delta, __sync_synchronize);\n\
                 int after = __sync_sub_and_fetch(&value, delta, &value);\n\
                 int changed = __sync_bool_compare_and_swap(&value, old, now);\n\
                 int seen = __sync_val_compare_and_swap(&value, now, after);\n\
                 pointer = __sync_lock_test_and_set(&pointer, (void *)0);\n\
                 return old + changed + seen;\n\
             }",
        )
        .unwrap();
        let dump = dump_ast(&unit);
        for spelling in [
            "__sync_fetch_and_add",
            "__sync_add_and_fetch",
            "__sync_sub_and_fetch",
            "__sync_bool_compare_and_swap",
            "__sync_val_compare_and_swap",
            "__sync_lock_test_and_set",
        ] {
            assert!(dump.contains(spelling), "{dump}");
        }
        assert!(dump.contains("name __sync_synchronize"), "{dump}");

        for source in [
            "int x; int f(void) { return __sync_fetch_and_add(&x); }",
            "int x; int f(void) { return __sync_bool_compare_and_swap(&x, 1); }",
            "int x; int f(void) { return __sync_lock_test_and_set(); }",
        ] {
            let error = parse_source(source).unwrap_err();
            assert!(
                error.message.contains("requires at least"),
                "{source}: {error}"
            );
        }
    }

    #[test]
    fn parses_the_exact_scalar_atomic_builtin_surface() {
        let unit = parse_source(
            "int value; int expected;\n\
             int update(int operand) {\n\
                 int result = __atomic_load_n(&value, 0);\n\
                 __atomic_store_n(&value, operand, 3);\n\
                 result ^= __atomic_exchange_n(&value, operand, 5);\n\
                 result ^= __atomic_fetch_add(&value, operand, 0);\n\
                 result ^= __atomic_fetch_sub(&value, operand, 1);\n\
                 result ^= __atomic_fetch_and(&value, operand, 2);\n\
                 result ^= __atomic_fetch_or(&value, operand, 3);\n\
                 result ^= __atomic_fetch_xor(&value, operand, 4);\n\
                 result ^= __atomic_add_fetch(&value, operand, 5);\n\
                 result ^= __atomic_sub_fetch(&value, operand, 0);\n\
                 result ^= __atomic_and_fetch(&value, operand, 1);\n\
                 result ^= __atomic_or_fetch(&value, operand, 2);\n\
                 result ^= __atomic_xor_fetch(&value, operand, 3);\n\
                 result ^= __atomic_compare_exchange_n(\n\
                     &value, &expected, operand, 1, 4, 2);\n\
                 __atomic_thread_fence(5);\n\
                 __atomic_signal_fence(5);\n\
                 return result;\n\
             }",
        )
        .unwrap();
        let dump = dump_ast(&unit);
        for spelling in [
            "__atomic_load_n",
            "__atomic_store_n",
            "__atomic_exchange_n",
            "__atomic_fetch_add",
            "__atomic_fetch_sub",
            "__atomic_fetch_and",
            "__atomic_fetch_or",
            "__atomic_fetch_xor",
            "__atomic_add_fetch",
            "__atomic_sub_fetch",
            "__atomic_and_fetch",
            "__atomic_or_fetch",
            "__atomic_xor_fetch",
            "__atomic_compare_exchange_n",
            "__atomic_thread_fence",
            "__atomic_signal_fence",
        ] {
            assert!(dump.contains(spelling), "{dump}");
        }

        for source in [
            "int x; int f(void) { return __atomic_load_n(&x); }",
            "int x; void f(void) { __atomic_store_n(&x, 1); }",
            "int x, expected; int f(void) { return __atomic_compare_exchange_n(&x, &expected, 1, 0, 5); }",
            "void f(void) { __atomic_thread_fence(); }",
        ] {
            let error = parse_source(source).unwrap_err();
            assert!(
                error.message.contains("requires exactly"),
                "{source}: {error}"
            );
        }
    }

    #[test]
    fn parses_the_exact_integer_intrinsic_and_prefetch_surface() {
        let unit = parse_source(
            "unsigned long long swap(unsigned long long value) {\n\
                 return __builtin_bswap64(value);\n\
             }\n\
             int bits(unsigned int word, unsigned long wide, unsigned long long widest) {\n\
                 return __builtin_clz(word) + __builtin_clzl(wide) +\n\
                     __builtin_clzll(widest) + __builtin_ctz(word) +\n\
                     __builtin_ctzll(widest) +\n\
                     __builtin_popcount(word) + __builtin_popcountll(widest);\n\
             }\n\
             void hints(void *address) {\n\
                 __builtin_prefetch(address);\n\
                 __builtin_prefetch(address, 1);\n\
                 __builtin_prefetch(address, 0, 3);\n\
             }",
        )
        .unwrap();
        let dump = dump_ast(&unit);
        for spelling in [
            "__builtin_bswap64",
            "__builtin_clz",
            "__builtin_clzl",
            "__builtin_clzll",
            "__builtin_ctz",
            "__builtin_ctzll",
            "__builtin_popcount",
            "__builtin_popcountll",
        ] {
            assert!(dump.contains(spelling), "{dump}");
        }
        assert_eq!(dump.matches("builtin-prefetch").count(), 3, "{dump}");

        for source in [
            "int f(void) { return __builtin_clz(); }",
            "int f(void) { return __builtin_clz(1, 2); }",
        ] {
            let error = parse_source(source).unwrap_err();
            assert!(error.message.contains("exactly one argument"), "{error}");
        }
        for source in [
            "void f(void) { __builtin_prefetch(); }",
            "void f(void *p) { __builtin_prefetch(p, 0, 1, 2); }",
            "void f(void *p) { __builtin_prefetch(p,); }",
        ] {
            let error = parse_source(source).unwrap_err();
            assert!(error.message.contains("__builtin_prefetch"), "{error}");
        }
    }

    #[test]
    fn parses_gnu_statement_expressions_and_memory_builtins() {
        let source = "void *copy(void *to, const void *from, unsigned long count) {\n\
                 return ({ __builtin_memcpy(to, from, count); });\n\
             }\n\
             void *move(void *to, const void *from, unsigned long count) {\n\
                 return __builtin_memmove(to, from, count);\n\
             }\n\
             void *fill(void *to, int value, unsigned long count) {\n\
                 return __builtin_memset(to, value, count);\n\
             }";
        let unit = parse_source_with_mode(source, LanguageMode::Gnu11).unwrap();
        let dump = dump_ast(&unit);
        assert!(dump.contains("statement-expression"), "{dump}");
        for spelling in ["__builtin_memcpy", "__builtin_memmove", "__builtin_memset"] {
            assert!(dump.contains(spelling), "{dump}");
        }
        assert!(parse_source_with_mode(source, LanguageMode::C11).is_err());

        for source in [
            "void *f(void *p) { return __builtin_memcpy(p, p); }",
            "void *f(void *p) { return __builtin_memmove(p, p, 1, 2); }",
            "void *f(void *p) { return __builtin_memset(p, 0); }",
        ] {
            let error = parse_source(source).unwrap_err();
            assert!(error.message.contains("exactly three arguments"), "{error}");
        }
    }

    #[test]
    fn parses_scalar_builtins_with_their_exact_arity() {
        let unit = parse_source(
            "long choose(int value, int expected) {\n\
                 return __builtin_expect(value, expected);\n\
             }\n\
             double infinity(void) { return __builtin_huge_val(); }\n\
             float infinityf(void) { return __builtin_inff(); }\n\
             float not_a_number(void) { return __builtin_nanf(\"\"); }",
        )
        .unwrap();
        let dump = dump_ast(&unit);
        assert!(dump.contains("builtin-expect"), "{dump}");
        assert!(dump.contains("builtin-huge-val"), "{dump}");
        assert!(dump.contains("builtin-inff"), "{dump}");
        assert!(dump.contains("builtin-nanf"), "{dump}");

        for source in [
            "long choose(void) { return __builtin_expect(1); }",
            "long choose(void) { return __builtin_expect(1, 0, 2); }",
            "double infinity(void) { return __builtin_huge_val(1); }",
            "float infinityf(void) { return __builtin_inff(1); }",
            "float not_a_number(void) { return __builtin_nanf(); }",
            "float not_a_number(void) { return __builtin_nanf(\"\", \"x\"); }",
        ] {
            assert!(parse_source(source).is_err(), "{source}");
        }
    }

    #[test]
    fn rejects_a_missing_va_arg_type_name_during_parsing() {
        let error = parse_source(
            "typedef __builtin_va_list va_list;\n\
             int read(int count, ...) {\n\
                 va_list list; return __builtin_va_arg(list, );\n\
             }",
        )
        .unwrap_err();
        assert!(
            error.message.contains("declaration specifiers") || error.message.contains("type"),
            "{error}"
        );
    }

    #[test]
    fn parses_the_curated_hosted_header_output() {
        let source = include_str!("../../../../tests/preprocessing/goldens/hosted-header.out");
        let unit = parse_source(source).unwrap();
        assert!(unit.items.iter().any(|item| matches!(
            item,
            ExternalItem::FunctionDefinition(function)
                if function.declarator.identifier().is_some_and(|name| name.name == "fixture_identity")
        )));
    }

    #[test]
    fn dump_is_deterministic_and_numbers_anonymous_records_in_source_order() {
        let unit = parse_source("struct { int x; } a; struct { int y; } b;").unwrap();
        let first = dump_ast(&unit);
        let second = dump_ast(&unit);
        assert_eq!(first, second);
        assert!(first.contains("struct anonymous-0"));
        assert!(first.contains("struct anonymous-1"));
        assert!(!first.contains("frontend-test.c"));
    }

    #[test]
    fn rejects_invalid_numeric_tokens_during_conversion() {
        let mut sources = SourceMap::new();
        let file = sources.add_file("number.c", "1z");
        let tokens = lex(file, sources.source(file).unwrap()).unwrap();
        assert_eq!(tokens[0].kind, PpTokenKind::PpNumber);
        assert!(convert_pp_items(tokens.into_iter().map(PpItem::Token)).is_err());
    }
}
