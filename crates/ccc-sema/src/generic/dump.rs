use std::collections::HashSet;
use std::fmt::Write;

use super::model::*;

/// Renders the complete typed tree without source paths or byte offsets.
pub fn dump_frontend_typed_ast(unit: &FullTypedTranslationUnit) -> String {
    let mut output = String::from("translation-unit full-typed\n");
    let mut emitted_globals = HashSet::new();
    let mut emitted_functions = HashSet::new();
    for item in &unit.external_items {
        match item {
            FullTypedExternalItem::Global(id) if emitted_globals.insert(*id) => {
                dump_global(&mut output, unit, *id)
            }
            FullTypedExternalItem::Global(id) => {
                line(
                    &mut output,
                    0,
                    format_args!("global-redeclaration @{}", id.0),
                );
            }
            FullTypedExternalItem::Function(id) if emitted_functions.insert(*id) => {
                dump_function(&mut output, unit, *id)
            }
            FullTypedExternalItem::Function(id) => {
                line(
                    &mut output,
                    0,
                    format_args!("function-redeclaration @{}", id.0),
                );
            }
            FullTypedExternalItem::Typedef(id) => {
                let typedef = &unit.typedefs[id.0 as usize];
                line(
                    &mut output,
                    0,
                    format_args!(
                        "typedef !{} {} : {}",
                        id.0,
                        typedef.name,
                        unit.types.display_qualified(typedef.ty)
                    ),
                );
            }
            FullTypedExternalItem::TypeDeclaration { ty, .. } => {
                line(
                    &mut output,
                    0,
                    format_args!("type-declaration {}", unit.types.display(*ty)),
                );
            }
            FullTypedExternalItem::StaticAssert { value, .. } => {
                line(&mut output, 0, format_args!("static-assert {value}"));
            }
            FullTypedExternalItem::Pragma(pragma) => {
                line(
                    &mut output,
                    0,
                    format_args!("pragma {}", render_pragma(pragma)),
                );
            }
        }
    }
    output
}

fn dump_global(output: &mut String, unit: &FullTypedTranslationUnit, id: GlobalId) {
    let global = &unit.globals[id.0 as usize];
    line(
        output,
        0,
        format_args!(
            "global @{} {} : {} storage={:?} linkage={:?} duration={:?} symbol={} definition={:?}",
            id.0,
            global.name,
            unit.types.display_qualified(global.ty),
            global.storage,
            global.linkage,
            global.duration,
            global.emission.symbol_name,
            global.emission.definition
        ),
    );
    if let Some(initializer) = &global.initializer {
        dump_initializer(output, unit, initializer, 1);
    }
}

fn dump_function(output: &mut String, unit: &FullTypedTranslationUnit, id: FullFunctionId) {
    let function = &unit.functions[id.0 as usize];
    line(
        output,
        0,
        format_args!(
            "function @{} {} : {} storage={:?} linkage={:?} visibility={:?} inline={} noreturn={} {}",
            id.0,
            function.name,
            unit.types.display(function.signature),
            function.storage,
            function.linkage,
            function.visibility,
            function.properties.inline,
            function.properties.no_return,
            if function.body.is_some() {
                "definition"
            } else {
                "declaration"
            }
        ),
    );
    for parameter in &function.parameters {
        line(
            output,
            1,
            format_args!(
                "parameter %{} {} : {}",
                parameter.local.0,
                parameter.name,
                unit.types.display_qualified(parameter.ty)
            ),
        );
    }
    if let Some(body) = &function.body {
        dump_statement(output, unit, body, 1);
    }
}

fn dump_statement(
    output: &mut String,
    unit: &FullTypedTranslationUnit,
    statement: &FullTypedStatement,
    indent: usize,
) {
    match &statement.kind {
        FullTypedStatementKind::Label {
            label,
            name,
            statement,
        } => {
            line(output, indent, format_args!("label ^{} {name}", label.0));
            dump_statement(output, unit, statement, indent + 1);
        }
        FullTypedStatementKind::Case { value, statement } => {
            line(output, indent, format_args!("case {value}"));
            dump_statement(output, unit, statement, indent + 1);
        }
        FullTypedStatementKind::Default(statement) => {
            line(output, indent, format_args!("default"));
            dump_statement(output, unit, statement, indent + 1);
        }
        FullTypedStatementKind::Compound(items) => {
            line(output, indent, format_args!("compound"));
            for item in items {
                dump_block_item(output, unit, item, indent + 1);
            }
        }
        FullTypedStatementKind::Expression(expression) => {
            line(output, indent, format_args!("expression-statement"));
            if let Some(expression) = expression {
                dump_expression(output, unit, expression, indent + 1);
            }
        }
        FullTypedStatementKind::If {
            condition,
            then_statement,
            else_statement,
        } => {
            line(output, indent, format_args!("if"));
            dump_named_expression(output, unit, "condition", condition, indent + 1);
            line(output, indent + 1, format_args!("then"));
            dump_statement(output, unit, then_statement, indent + 2);
            if let Some(statement) = else_statement {
                line(output, indent + 1, format_args!("else"));
                dump_statement(output, unit, statement, indent + 2);
            }
        }
        FullTypedStatementKind::Switch {
            expression,
            statement,
        } => {
            line(output, indent, format_args!("switch"));
            dump_named_expression(output, unit, "value", expression, indent + 1);
            dump_statement(output, unit, statement, indent + 1);
        }
        FullTypedStatementKind::While {
            condition,
            statement,
        } => {
            line(output, indent, format_args!("while"));
            dump_named_expression(output, unit, "condition", condition, indent + 1);
            dump_statement(output, unit, statement, indent + 1);
        }
        FullTypedStatementKind::DoWhile {
            statement,
            condition,
        } => {
            line(output, indent, format_args!("do"));
            dump_statement(output, unit, statement, indent + 1);
            dump_named_expression(output, unit, "condition", condition, indent + 1);
        }
        FullTypedStatementKind::For {
            initializer,
            condition,
            step,
            statement,
        } => {
            line(output, indent, format_args!("for"));
            match initializer {
                FullTypedForInitializer::Empty => {
                    line(output, indent + 1, format_args!("initializer empty"));
                }
                FullTypedForInitializer::Expression(expression) => {
                    dump_named_expression(output, unit, "initializer", expression, indent + 1);
                }
                FullTypedForInitializer::Declarations(items) => {
                    line(output, indent + 1, format_args!("initializer declarations"));
                    for item in items {
                        dump_block_item(output, unit, item, indent + 2);
                    }
                }
            }
            if let Some(condition) = condition {
                dump_named_expression(output, unit, "condition", condition, indent + 1);
            }
            if let Some(step) = step {
                dump_named_expression(output, unit, "step", step, indent + 1);
            }
            dump_statement(output, unit, statement, indent + 1);
        }
        FullTypedStatementKind::Goto { label, name } => {
            line(output, indent, format_args!("goto ^{} {name}", label.0));
        }
        FullTypedStatementKind::Continue => line(output, indent, format_args!("continue")),
        FullTypedStatementKind::Break => line(output, indent, format_args!("break")),
        FullTypedStatementKind::Return(expression) => {
            line(output, indent, format_args!("return"));
            if let Some(expression) = expression {
                dump_expression(output, unit, expression, indent + 1);
            }
        }
    }
}

fn dump_block_item(
    output: &mut String,
    unit: &FullTypedTranslationUnit,
    item: &FullTypedBlockItem,
    indent: usize,
) {
    match item {
        FullTypedBlockItem::Declaration(declaration) => {
            line(
                output,
                indent,
                format_args!(
                    "local %{} {} : {} storage={:?} duration={:?}",
                    declaration.local.0,
                    declaration.name,
                    unit.types.display_qualified(declaration.ty),
                    declaration.storage,
                    declaration.duration
                ),
            );
            if let Some(emission) = &declaration.emission {
                line(
                    output,
                    indent + 1,
                    format_args!(
                        "data symbol={} visibility={:?} section={:?} align={:?} tls={:?}",
                        emission.symbol_name,
                        emission.visibility,
                        emission.section,
                        emission.requested_alignment,
                        emission.tls
                    ),
                );
            }
            if let Some(initializer) = &declaration.initializer {
                dump_initializer(output, unit, initializer, indent + 1);
            }
        }
        FullTypedBlockItem::Typedef(typedef) => line(
            output,
            indent,
            format_args!(
                "typedef !{} {} : {}",
                typedef.id.0,
                typedef.name,
                unit.types.display_qualified(typedef.ty)
            ),
        ),
        FullTypedBlockItem::ExternalObject(id) => {
            line(output, indent, format_args!("extern-object @{}", id.0));
        }
        FullTypedBlockItem::FunctionDeclaration(id) => {
            line(
                output,
                indent,
                format_args!("function-declaration @{}", id.0),
            );
        }
        FullTypedBlockItem::StaticAssert { value, .. } => {
            line(output, indent, format_args!("static-assert {value}"));
        }
        FullTypedBlockItem::Statement(statement) => {
            dump_statement(output, unit, statement, indent);
        }
        FullTypedBlockItem::Pragma(pragma) => {
            line(
                output,
                indent,
                format_args!("pragma {}", render_pragma(pragma)),
            );
        }
    }
}

fn dump_initializer(
    output: &mut String,
    unit: &FullTypedTranslationUnit,
    initializer: &FullTypedInitializer,
    indent: usize,
) {
    line(
        output,
        indent,
        format_args!(
            "initializer : {}",
            unit.types.display_qualified(initializer.ty)
        ),
    );
    match &initializer.kind {
        FullTypedInitializerKind::Scalar(expression) => {
            dump_expression(output, unit, expression, indent + 1)
        }
        FullTypedInitializerKind::Aggregate(entries) => {
            for entry in entries {
                line(
                    output,
                    indent + 1,
                    format_args!("subobject {:?}", entry.path),
                );
                dump_initializer(output, unit, &entry.initializer, indent + 2);
            }
        }
        FullTypedInitializerKind::String(id) => {
            line(output, indent + 1, format_args!("string ${}", id.0));
        }
        FullTypedInitializerKind::Zero => line(output, indent + 1, format_args!("zero")),
    }
}

fn dump_named_expression(
    output: &mut String,
    unit: &FullTypedTranslationUnit,
    name: &str,
    expression: &FullTypedExpression,
    indent: usize,
) {
    line(output, indent, format_args!("{name}"));
    dump_expression(output, unit, expression, indent + 1);
}

fn dump_expression(
    output: &mut String,
    unit: &FullTypedTranslationUnit,
    expression: &FullTypedExpression,
    indent: usize,
) {
    let ty = unit.types.display_qualified(expression.ty);
    let suffix = format!(" : {ty} {:?}", expression.category);
    match &expression.kind {
        FullTypedExpressionKind::Constant(value) => {
            line(output, indent, format_args!("constant {value:?}{suffix}"));
        }
        FullTypedExpressionKind::StringLiteral(id) => {
            line(output, indent, format_args!("string ${}{suffix}", id.0));
        }
        FullTypedExpressionKind::DeclRef(reference) => {
            line(
                output,
                indent,
                format_args!("decl-ref {reference:?}{suffix}"),
            );
        }
        FullTypedExpressionKind::Conversion { kind, expression } => {
            line(output, indent, format_args!("convert {kind:?}{suffix}"));
            dump_expression(output, unit, expression, indent + 1);
        }
        FullTypedExpressionKind::Unary { operator, operand } => {
            line(output, indent, format_args!("unary {operator:?}{suffix}"));
            dump_expression(output, unit, operand, indent + 1);
        }
        FullTypedExpressionKind::Binary {
            operator,
            left,
            right,
        } => {
            line(output, indent, format_args!("binary {operator:?}{suffix}"));
            dump_expression(output, unit, left, indent + 1);
            dump_expression(output, unit, right, indent + 1);
        }
        FullTypedExpressionKind::AddressOf(operand) => {
            line(output, indent, format_args!("address-of{suffix}"));
            dump_expression(output, unit, operand, indent + 1);
        }
        FullTypedExpressionKind::Dereference(operand) => {
            line(output, indent, format_args!("dereference{suffix}"));
            dump_expression(output, unit, operand, indent + 1);
        }
        FullTypedExpressionKind::Subscript { base, index } => {
            line(output, indent, format_args!("subscript{suffix}"));
            dump_expression(output, unit, base, indent + 1);
            dump_expression(output, unit, index, indent + 1);
        }
        FullTypedExpressionKind::Member {
            base,
            field_index,
            name,
            indirect,
            bitfield,
        } => {
            let bitfield = bitfield.as_deref().map_or_else(String::new, |descriptor| {
                format!(
                    " bitfield={}:{}:{}/{}",
                    descriptor.storage_offset,
                    descriptor.storage_size,
                    descriptor.bit_offset,
                    descriptor.width
                )
            });
            line(
                output,
                indent,
                format_args!("member #{field_index} {name} indirect={indirect}{bitfield}{suffix}"),
            );
            dump_expression(output, unit, base, indent + 1);
        }
        FullTypedExpressionKind::Assignment {
            operator,
            target,
            value,
            store,
            compound,
        } => {
            let compound = compound.as_ref().map_or_else(
                || "none".to_owned(),
                |plan| {
                    format!(
                        "operator={:?} load={} calculation={} access={:?} result={:?}",
                        plan.operator,
                        unit.types.display_qualified(plan.load_ty),
                        unit.types.display_qualified(plan.calculation_ty),
                        plan.load,
                        plan.result_conversion
                    )
                },
            );
            line(
                output,
                indent,
                format_args!("assignment {operator:?} store={store:?} compound={compound}{suffix}"),
            );
            dump_expression(output, unit, target, indent + 1);
            dump_expression(output, unit, value, indent + 1);
        }
        FullTypedExpressionKind::Increment {
            operand,
            decrement,
            postfix,
            store,
        } => {
            line(
                output,
                indent,
                format_args!(
                    "increment decrement={decrement} postfix={postfix} store={store:?}{suffix}"
                ),
            );
            dump_expression(output, unit, operand, indent + 1);
        }
        FullTypedExpressionKind::Call {
            callee,
            function,
            arguments,
            variadic_boundary,
        } => {
            line(
                output,
                indent,
                format_args!(
                    "call function={function:?} variadic-boundary={variadic_boundary}{suffix}"
                ),
            );
            dump_expression(output, unit, callee, indent + 1);
            for argument in arguments {
                dump_expression(output, unit, argument, indent + 1);
            }
        }
        FullTypedExpressionKind::Conditional {
            condition,
            then_expression,
            else_expression,
        } => {
            line(output, indent, format_args!("conditional{suffix}"));
            dump_expression(output, unit, condition, indent + 1);
            dump_expression(output, unit, then_expression, indent + 1);
            dump_expression(output, unit, else_expression, indent + 1);
        }
        FullTypedExpressionKind::Comma(expressions) => {
            line(output, indent, format_args!("comma{suffix}"));
            for expression in expressions {
                dump_expression(output, unit, expression, indent + 1);
            }
        }
        FullTypedExpressionKind::Sizeof { operand_ty, size } => line(
            output,
            indent,
            format_args!(
                "sizeof {} = {size}{suffix}",
                unit.types.display_qualified(*operand_ty)
            ),
        ),
        FullTypedExpressionKind::Alignof { operand_ty, align } => line(
            output,
            indent,
            format_args!(
                "alignof {} = {align}{suffix}",
                unit.types.display_qualified(*operand_ty)
            ),
        ),
        FullTypedExpressionKind::Offsetof {
            record_ty,
            path,
            offset,
        } => line(
            output,
            indent,
            format_args!(
                "offsetof {} {:?} = {offset}{suffix}",
                unit.types.display_qualified(*record_ty),
                path
            ),
        ),
        FullTypedExpressionKind::VaStart {
            list,
            last_named_parameter,
        } => {
            line(
                output,
                indent,
                format_args!("va-start last=l{}{suffix}", last_named_parameter.0),
            );
            dump_expression(output, unit, list, indent + 1);
        }
        FullTypedExpressionKind::VaArg { list, requested } => {
            line(
                output,
                indent,
                format_args!(
                    "va-arg requested={}{suffix}",
                    unit.types.display_qualified(*requested)
                ),
            );
            dump_expression(output, unit, list, indent + 1);
        }
        FullTypedExpressionKind::VaCopy {
            destination,
            source,
        } => {
            line(output, indent, format_args!("va-copy{suffix}"));
            dump_expression(output, unit, destination, indent + 1);
            dump_expression(output, unit, source, indent + 1);
        }
        FullTypedExpressionKind::VaEnd { list } => {
            line(output, indent, format_args!("va-end{suffix}"));
            dump_expression(output, unit, list, indent + 1);
        }
    }
}

fn line(output: &mut String, indent: usize, arguments: std::fmt::Arguments<'_>) {
    let _ = writeln!(output, "{}{arguments}", "  ".repeat(indent));
}

fn render_pragma(pragma: &ccc_pp::PragmaEvent) -> String {
    match pragma {
        ccc_pp::PragmaEvent::Once { .. } => "once".to_owned(),
        ccc_pp::PragmaEvent::SystemHeader { .. } => "system-header".to_owned(),
        ccc_pp::PragmaEvent::Diagnostic { action, option, .. } => option.as_ref().map_or_else(
            || format!("diagnostic {action:?}"),
            |option| format!("diagnostic {action:?} {option}"),
        ),
        ccc_pp::PragmaEvent::Pack { payload, .. } => format!(
            "pack {}",
            payload
                .iter()
                .map(|token| token.spelling.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        ),
        ccc_pp::PragmaEvent::Unknown { text, .. } => format!("unknown {text}"),
    }
}
