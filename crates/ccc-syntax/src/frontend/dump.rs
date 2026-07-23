//! Deterministic, path-independent syntax tree snapshots.

use ccc_pp::PragmaEvent;

use super::ast::*;

/// Produces a path- and offset-independent syntax snapshot.
pub fn dump_ast(unit: &TranslationUnit) -> String {
    let mut dumper = AstDumper {
        output: String::new(),
        next_anonymous: 0,
    };
    dumper.line(0, "translation-unit");
    for item in &unit.items {
        dumper.external_item(item, 1);
    }
    dumper.output
}

struct AstDumper {
    output: String,
    next_anonymous: usize,
}

impl AstDumper {
    fn external_item(&mut self, item: &ExternalItem, indent: usize) {
        match item {
            ExternalItem::Pragma(pragma) => self.pragma(pragma, indent),
            ExternalItem::Declaration(declaration) => self.declaration(declaration, indent),
            ExternalItem::FunctionDefinition(function) => {
                let name = function
                    .declarator
                    .identifier()
                    .map_or("_", |identifier| identifier.name.as_str());
                self.line(indent, &format!("function-definition {name}"));
                self.specifiers(&function.specifiers, indent + 1);
                self.line(
                    indent + 1,
                    &format!("declarator {}", declarator_text(&function.declarator)),
                );
                for declaration in &function.declarations {
                    self.declaration(declaration, indent + 1);
                }
                self.statement(&function.body, indent + 1);
            }
            ExternalItem::StaticAssert(assertion) => {
                self.line(indent, "static-assert");
                self.expression(&assertion.condition, indent + 1);
            }
        }
    }

    fn declaration(&mut self, declaration: &Declaration, indent: usize) {
        self.line(indent, "declaration");
        self.specifiers(&declaration.specifiers, indent + 1);
        for declarator in &declaration.declarators {
            self.line(
                indent + 1,
                &format!("declarator {}", declarator_text(&declarator.declarator)),
            );
            if let Some(label) = &declarator.asm_label {
                self.line(
                    indent + 2,
                    &format!(
                        "asm-label {} {}",
                        label.keyword_spelling, label.literal_spelling
                    ),
                );
            }
            for attribute in &declarator.attributes {
                self.attribute(attribute, indent + 2);
            }
            if let Some(initializer) = &declarator.initializer {
                self.initializer(initializer, indent + 2);
            }
        }
    }

    fn specifiers(&mut self, specifiers: &DeclarationSpecifiers, indent: usize) {
        if specifiers.extension {
            self.line(indent, "extension");
        }
        for specifier in &specifiers.items {
            match specifier {
                DeclarationSpecifier::StorageClass(value) => {
                    self.line(indent, &format!("storage {value:?}"))
                }
                DeclarationSpecifier::Qualifier(value) => {
                    self.line(indent, &format!("qualifier {value:?}"))
                }
                DeclarationSpecifier::Function(value) => {
                    self.line(indent, &format!("function-specifier {value:?}"))
                }
                DeclarationSpecifier::Alignment(_) => self.line(indent, "alignment"),
                DeclarationSpecifier::Attribute(attribute) => self.attribute(attribute, indent),
                DeclarationSpecifier::Type(ty) => self.type_specifier(ty, indent),
            }
        }
    }

    fn type_specifier(&mut self, specifier: &TypeSpecifier, indent: usize) {
        match specifier {
            TypeSpecifier::Struct(record) => self.record("struct", record, indent),
            TypeSpecifier::Union(record) => self.record("union", record, indent),
            TypeSpecifier::Enum(enumeration) => self.enumeration(enumeration, indent),
            TypeSpecifier::TypedefName(name) => {
                self.line(indent, &format!("typedef-name {}", name.name))
            }
            TypeSpecifier::Atomic(_) => self.line(indent, "type Atomic"),
            TypeSpecifier::Typeof(_) => self.line(indent, "type Typeof"),
            TypeSpecifier::BuiltinVaList => self.line(indent, "type __builtin_va_list"),
            TypeSpecifier::Float16 => self.line(indent, "type _Float16"),
            TypeSpecifier::Int128 => self.line(indent, "type __int128"),
            TypeSpecifier::Int128T => self.line(indent, "type __int128_t"),
            TypeSpecifier::UInt128T => self.line(indent, "type __uint128_t"),
            other => self.line(indent, &format!("type {other:?}")),
        }
    }

    fn record(&mut self, kind: &str, record: &RecordSpecifier, indent: usize) {
        let name = record.tag.as_ref().map_or_else(
            || {
                let id = self.next_anonymous;
                self.next_anonymous += 1;
                format!("anonymous-{id}")
            },
            |tag| tag.name.clone(),
        );
        self.line(indent, &format!("{kind} {name}"));
        if let Some(items) = &record.items {
            for item in items {
                match item {
                    RecordItem::Pragma(pragma) => self.pragma(pragma, indent + 1),
                    RecordItem::StaticAssert(assertion) => {
                        self.line(indent + 1, "static-assert");
                        self.expression(&assertion.condition, indent + 2);
                    }
                    RecordItem::Declaration(declaration) => {
                        self.line(indent + 1, "member-declaration");
                        self.specifiers(&declaration.specifiers, indent + 2);
                        for declarator in &declaration.declarators {
                            let name = declarator
                                .declarator
                                .as_ref()
                                .map_or("_".to_owned(), declarator_text);
                            self.line(indent + 2, &format!("member {name}"));
                            if let Some(width) = &declarator.bit_width {
                                self.line(indent + 3, "bit-width");
                                self.expression(width, indent + 4);
                            }
                        }
                    }
                }
            }
        }
    }

    fn enumeration(&mut self, enumeration: &EnumSpecifier, indent: usize) {
        let name = enumeration.tag.as_ref().map_or_else(
            || {
                let id = self.next_anonymous;
                self.next_anonymous += 1;
                format!("anonymous-{id}")
            },
            |tag| tag.name.clone(),
        );
        self.line(indent, &format!("enum {name}"));
        if let Some(enumerators) = &enumeration.enumerators {
            for enumerator in enumerators {
                self.line(indent + 1, &format!("enumerator {}", enumerator.name.name));
                for attribute in &enumerator.attributes {
                    self.attribute(attribute, indent + 2);
                }
                if let Some(value) = &enumerator.value {
                    self.expression(value, indent + 2);
                }
            }
        }
    }

    fn attribute(&mut self, attribute: &Attribute, indent: usize) {
        self.line(
            indent,
            &format!("attribute {} {}", attribute.introducer, attribute.name.name),
        );
    }

    fn initializer(&mut self, initializer: &Initializer, indent: usize) {
        match initializer {
            Initializer::Expression(expression) => self.expression(expression, indent),
            Initializer::List { entries, .. } => {
                self.line(indent, "initializer-list");
                for entry in entries {
                    self.line(indent + 1, "initializer-entry");
                    self.initializer(&entry.initializer, indent + 2);
                }
            }
        }
    }

    fn statement(&mut self, statement: &Statement, indent: usize) {
        let name = match &statement.kind {
            StatementKind::Label { .. } => "label",
            StatementKind::Case { .. } => "case",
            StatementKind::Default(_) => "default",
            StatementKind::Compound(_) => "compound",
            StatementKind::Expression(_) => "expression-statement",
            StatementKind::If { .. } => "if",
            StatementKind::Switch { .. } => "switch",
            StatementKind::While { .. } => "while",
            StatementKind::DoWhile { .. } => "do-while",
            StatementKind::For { .. } => "for",
            StatementKind::Goto(_) => "goto",
            StatementKind::ComputedGoto(_) => "computed-goto",
            StatementKind::Asm(_) => "asm-statement",
            StatementKind::Continue => "continue",
            StatementKind::Break => "break",
            StatementKind::Return(_) => "return",
        };
        self.line(indent, name);
        match &statement.kind {
            StatementKind::Compound(items) => {
                for item in items {
                    match item {
                        BlockItem::Declaration(declaration) => {
                            self.declaration(declaration, indent + 1)
                        }
                        BlockItem::StaticAssert(assertion) => {
                            self.line(indent + 1, "static-assert");
                            self.expression(&assertion.condition, indent + 2);
                        }
                        BlockItem::Statement(statement) => self.statement(statement, indent + 1),
                        BlockItem::Pragma(pragma) => self.pragma(pragma, indent + 1),
                    }
                }
            }
            StatementKind::Expression(Some(expression))
            | StatementKind::Return(Some(expression))
            | StatementKind::ComputedGoto(expression) => self.expression(expression, indent + 1),
            StatementKind::Asm(asm) => {
                let qualifiers = asm
                    .qualifiers
                    .iter()
                    .map(|qualifier| qualifier.spelling.as_str())
                    .collect::<Vec<_>>()
                    .join(",");
                self.line(
                    indent + 1,
                    &format!(
                        "asm template={:?} qualifiers=[{}] colon-groups={}",
                        asm.template.code_units, qualifiers, asm.colon_group_count
                    ),
                );
                for (kind, operands) in [("output", &asm.outputs), ("input", &asm.inputs)] {
                    for operand in operands {
                        self.line(
                            indent + 2,
                            &format!(
                                "{kind} name={} constraint={:?}",
                                operand
                                    .symbolic_name
                                    .as_ref()
                                    .map_or("_", |name| name.name.as_str()),
                                operand.constraint.literal.code_units
                            ),
                        );
                        self.expression(&operand.expression, indent + 3);
                    }
                }
                for clobber in &asm.clobbers {
                    self.line(
                        indent + 2,
                        &format!("clobber {:?}", clobber.literal.code_units),
                    );
                }
                for label in &asm.goto_labels {
                    self.line(indent + 2, &format!("goto-label {}", label.name));
                }
            }
            StatementKind::If {
                condition,
                then_statement,
                else_statement,
            } => {
                self.expression(condition, indent + 1);
                self.statement(then_statement, indent + 1);
                if let Some(statement) = else_statement {
                    self.statement(statement, indent + 1);
                }
            }
            StatementKind::Switch {
                expression,
                statement,
            } => {
                self.expression(expression, indent + 1);
                self.statement(statement, indent + 1);
            }
            StatementKind::While {
                condition,
                statement,
            } => {
                self.expression(condition, indent + 1);
                self.statement(statement, indent + 1);
            }
            StatementKind::DoWhile {
                statement,
                condition,
            } => {
                self.statement(statement, indent + 1);
                self.expression(condition, indent + 1);
            }
            StatementKind::For {
                condition,
                step,
                statement,
                ..
            } => {
                if let Some(condition) = condition {
                    self.expression(condition, indent + 1);
                }
                if let Some(step) = step {
                    self.expression(step, indent + 1);
                }
                self.statement(statement, indent + 1);
            }
            StatementKind::Label { statement, .. }
            | StatementKind::Case { statement, .. }
            | StatementKind::Default(statement) => self.statement(statement, indent + 1),
            StatementKind::Expression(None)
            | StatementKind::Goto(_)
            | StatementKind::Continue
            | StatementKind::Break
            | StatementKind::Return(None) => {}
        }
    }

    fn expression(&mut self, expression: &Expression, indent: usize) {
        match &expression.kind {
            ExpressionKind::Identifier(identifier) => {
                self.line(indent, &format!("name {}", identifier.name))
            }
            ExpressionKind::LabelAddress(label) => {
                self.line(indent, &format!("label-address {}", label.name))
            }
            ExpressionKind::Integer(value) => {
                self.line(indent, &format!("integer {}", value.value))
            }
            ExpressionKind::Floating(_) => self.line(indent, "floating"),
            ExpressionKind::Character(value) => {
                self.line(indent, &format!("character {}", value.value))
            }
            ExpressionKind::String(value) => {
                self.line(indent, &format!("string {:?}", value.prefix))
            }
            ExpressionKind::Unary { operator, operand } => {
                self.line(indent, &format!("unary {operator:?}"));
                self.expression(operand, indent + 1);
            }
            ExpressionKind::Binary {
                operator,
                left,
                right,
            } => {
                self.line(indent, &format!("binary {operator:?}"));
                self.expression(left, indent + 1);
                self.expression(right, indent + 1);
            }
            ExpressionKind::Assignment {
                operator,
                target,
                value,
            } => {
                self.line(indent, &format!("assignment {operator:?}"));
                self.expression(target, indent + 1);
                self.expression(value, indent + 1);
            }
            ExpressionKind::Conditional {
                condition,
                then_expression,
                else_expression,
            } => {
                self.line(indent, "conditional");
                self.expression(condition, indent + 1);
                self.expression(then_expression, indent + 1);
                self.expression(else_expression, indent + 1);
            }
            ExpressionKind::Call { callee, arguments } => {
                self.line(indent, "call");
                self.expression(callee, indent + 1);
                for argument in arguments {
                    self.expression(argument, indent + 1);
                }
            }
            ExpressionKind::BuiltinOffsetof { .. } => self.line(indent, "builtin-offsetof"),
            ExpressionKind::BuiltinVaStart {
                list,
                last_named_parameter,
            } => {
                self.line(indent, "builtin-va-start");
                self.expression(list, indent + 1);
                self.expression(last_named_parameter, indent + 1);
            }
            ExpressionKind::BuiltinVaArg { list, .. } => {
                self.line(indent, "builtin-va-arg");
                self.expression(list, indent + 1);
            }
            ExpressionKind::BuiltinVaCopy {
                destination,
                source,
            } => {
                self.line(indent, "builtin-va-copy");
                self.expression(destination, indent + 1);
                self.expression(source, indent + 1);
            }
            ExpressionKind::BuiltinVaEnd { list } => {
                self.line(indent, "builtin-va-end");
                self.expression(list, indent + 1);
            }
            ExpressionKind::BuiltinExpect { value, expected } => {
                self.line(indent, "builtin-expect");
                self.expression(value, indent + 1);
                self.expression(expected, indent + 1);
            }
            ExpressionKind::BuiltinHugeVal => {
                self.line(indent, "builtin-huge-val");
            }
            ExpressionKind::BuiltinInfF => {
                self.line(indent, "builtin-inff");
            }
            ExpressionKind::BuiltinNanF { payload } => {
                self.line(indent, "builtin-nanf");
                self.expression(payload, indent + 1);
            }
            ExpressionKind::BuiltinIntegerIntrinsic { operation, operand } => {
                self.line(indent, operation.spelling());
                self.expression(operand, indent + 1);
            }
            ExpressionKind::BuiltinMemoryOperation {
                operation,
                arguments,
            } => {
                self.line(indent, operation.spelling());
                for argument in arguments {
                    self.expression(argument, indent + 1);
                }
            }
            ExpressionKind::BuiltinPrefetch { arguments } => {
                self.line(indent, "builtin-prefetch");
                for argument in arguments {
                    self.expression(argument, indent + 1);
                }
            }
            ExpressionKind::BuiltinSyncOperation {
                operation,
                arguments,
            } => {
                self.line(indent, operation.spelling());
                for argument in arguments {
                    self.expression(argument, indent + 1);
                }
            }
            ExpressionKind::BuiltinSyncSynchronize => {
                self.line(indent, "builtin-sync-synchronize");
            }
            ExpressionKind::BuiltinAtomicOperation {
                operation,
                arguments,
            } => {
                self.line(indent, operation.spelling());
                for argument in arguments {
                    self.expression(argument, indent + 1);
                }
            }
            ExpressionKind::Comma(expressions) => {
                self.line(indent, "comma");
                for expression in expressions {
                    self.expression(expression, indent + 1);
                }
            }
            ExpressionKind::Parenthesized(inner)
            | ExpressionKind::PostfixIncrement(inner)
            | ExpressionKind::PostfixDecrement(inner)
            | ExpressionKind::SizeofExpression(inner)
            | ExpressionKind::Extension(inner) => {
                self.line(indent, "expression");
                self.expression(inner, indent + 1);
            }
            ExpressionKind::StatementExpression(items) => {
                self.line(indent, "statement-expression");
                for item in items {
                    match item {
                        BlockItem::Declaration(declaration) => {
                            self.declaration(declaration, indent + 1)
                        }
                        BlockItem::StaticAssert(assertion) => {
                            self.line(indent + 1, "static-assert");
                            self.expression(&assertion.condition, indent + 2);
                        }
                        BlockItem::Statement(statement) => self.statement(statement, indent + 1),
                        BlockItem::Pragma(pragma) => self.pragma(pragma, indent + 1),
                    }
                }
            }
            ExpressionKind::Subscript { base, index } => {
                self.line(indent, "subscript");
                self.expression(base, indent + 1);
                self.expression(index, indent + 1);
            }
            ExpressionKind::Member { base, member, .. } => {
                self.line(indent, &format!("member {}", member.name));
                self.expression(base, indent + 1);
            }
            ExpressionKind::GenericSelection { controlling, .. } => {
                self.line(indent, "generic-selection");
                self.expression(controlling, indent + 1);
            }
            ExpressionKind::CompoundLiteral { .. } => self.line(indent, "compound-literal"),
            ExpressionKind::SizeofType(_) => self.line(indent, "sizeof-type"),
            ExpressionKind::AlignofType(_) => self.line(indent, "alignof-type"),
            ExpressionKind::Cast { expression, .. } => {
                self.line(indent, "cast");
                self.expression(expression, indent + 1);
            }
        }
    }

    fn pragma(&mut self, pragma: &PragmaEvent, indent: usize) {
        let text = match pragma {
            PragmaEvent::Once { .. } => "once".to_owned(),
            PragmaEvent::SystemHeader { .. } => "GCC system_header".to_owned(),
            PragmaEvent::Diagnostic { .. } => "GCC diagnostic".to_owned(),
            PragmaEvent::GccOptimize { payload, .. } => format!(
                "GCC optimize {}",
                payload
                    .iter()
                    .map(|token| token.spelling.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
            PragmaEvent::Pack { payload, .. } => format!(
                "pack {}",
                payload
                    .iter()
                    .map(|token| token.spelling.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
            PragmaEvent::Unknown { text, .. } => text.clone(),
        };
        self.line(indent, &format!("pragma {text}"));
    }

    fn line(&mut self, indent: usize, text: &str) {
        self.output.push_str(&"  ".repeat(indent));
        self.output.push_str(text);
        self.output.push('\n');
    }
}

fn declarator_text(declarator: &Declarator) -> String {
    let mut text = "*".repeat(declarator.pointers.len());
    text.push_str(&direct_declarator_text(&declarator.direct));
    text
}

fn direct_declarator_text(declarator: &DirectDeclarator) -> String {
    match declarator {
        DirectDeclarator::Identifier(identifier) => identifier.name.clone(),
        DirectDeclarator::Abstract(_) => "_".to_owned(),
        DirectDeclarator::Parenthesized(inner, _) => format!("({})", declarator_text(inner)),
        DirectDeclarator::Array { inner, .. } => {
            format!("{}[]", direct_declarator_text(inner))
        }
        DirectDeclarator::Function {
            inner,
            parameters,
            has_parameter_type_list,
            variadic,
            old_style_names,
            ..
        } => {
            let arity = if !has_parameter_type_list && old_style_names.is_empty() {
                "unspecified".to_owned()
            } else if old_style_names.is_empty() {
                parameters.len().to_string()
            } else {
                old_style_names.len().to_string()
            };
            let suffix = if *variadic { ",..." } else { "" };
            format!("{}({arity}{suffix})", direct_declarator_text(inner))
        }
    }
}
