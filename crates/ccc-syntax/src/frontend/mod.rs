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
