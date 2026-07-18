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
            }) if value.value == 6.0
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
