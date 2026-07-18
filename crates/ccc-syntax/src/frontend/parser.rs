//! Recursive-descent parsing over the ordered frontend item stream.

use std::fmt;

use ccc_pp::{LanguageMode, PragmaEvent};
use ccc_session::Span;

use crate::{Keyword, Punctuator};

use super::{ast::*, names::*, span_through, token::*};

const MAX_RECURSION_DEPTH: usize = 256;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParseError {
    pub code: &'static str,
    pub span: Span,
    pub message: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.message.fmt(formatter)
    }
}

impl std::error::Error for ParseError {}

enum ParseItem<'a> {
    Token(&'a Token),
    Pragma(&'a PragmaEvent),
}

/// Parses a generic translation unit from the ordered phase-7 item stream.
pub fn parse(items: &[FrontendItem]) -> Result<TranslationUnit, ParseError> {
    parse_with_mode(items, LanguageMode::Gnu11)
}

pub fn parse_with_mode(
    items: &[FrontendItem],
    language_mode: LanguageMode,
) -> Result<TranslationUnit, ParseError> {
    let items = items
        .iter()
        .filter_map(|item| match item {
            FrontendItem::Token(token) => Some(ParseItem::Token(token)),
            FrontendItem::Pragma(pragma) => Some(ParseItem::Pragma(pragma)),
            FrontendItem::LineMarker(_) | FrontendItem::Newline => None,
        })
        .collect::<Vec<_>>();
    Parser {
        items,
        position: 0,
        names: NameClassEnv::new(),
        recursion_depth: 0,
        language_mode,
    }
    .translation_unit()
}

struct Parser<'a> {
    items: Vec<ParseItem<'a>>,
    position: usize,
    names: NameClassEnv,
    recursion_depth: usize,
    language_mode: LanguageMode,
}

impl Parser<'_> {
    fn translation_unit(mut self) -> Result<TranslationUnit, ParseError> {
        let mut items = Vec::new();
        while self.current_item().is_some() {
            if let Some(pragma) = self.consume_pragma() {
                items.push(ExternalItem::Pragma(pragma));
                continue;
            }
            if self.language_mode == LanguageMode::Gnu11
                && self.consume_punctuator(Punctuator::Semicolon).is_some()
            {
                continue;
            }
            if self.check_keyword(Keyword::StaticAssert) {
                let assertion = self.static_assert()?;
                items.push(ExternalItem::StaticAssert(Box::new(assertion)));
                continue;
            }
            items.push(self.external_declaration()?);
        }
        Ok(TranslationUnit {
            items,
            scope_events: self.names.events,
        })
    }

    fn external_declaration(&mut self) -> Result<ExternalItem, ParseError> {
        let checkpoint = self.names.checkpoint();
        let result = self.external_declaration_inner();
        match result {
            Ok(item) => {
                self.names.commit(checkpoint);
                Ok(item)
            }
            Err(error) => {
                self.names.rollback(checkpoint);
                Err(error)
            }
        }
    }

    fn external_declaration_inner(&mut self) -> Result<ExternalItem, ParseError> {
        let specifiers = self.declaration_specifiers()?;
        let start = specifiers.span;
        if let Some(semicolon) = self.consume_punctuator(Punctuator::Semicolon) {
            return Ok(ExternalItem::Declaration(Declaration {
                span: span_through(start, semicolon.span),
                specifiers,
                declarators: Vec::new(),
            }));
        }

        let declarator = self.declarator(false)?;
        let class = if specifiers.is_typedef() {
            NameClass::TypedefName
        } else {
            NameClass::Ordinary
        };
        self.bind_declarator(&declarator, class);

        if self.check_punctuator(Punctuator::LeftBrace)
            || (declarator.has_old_style_names() && self.starts_old_style_parameter_declaration())
        {
            let scope_span = declarator.span;
            self.names.enter_scope(ScopeKind::Function, scope_span);
            for name in declarator.old_style_names() {
                self.names
                    .bind(name.name.clone(), NameClass::Ordinary, name.span);
            }
            for parameter in declarator.parameters() {
                if let Some(parameter_declarator) = &parameter.declarator {
                    self.bind_declarator(parameter_declarator, NameClass::Ordinary);
                }
            }
            let mut declarations = Vec::new();
            while !self.check_punctuator(Punctuator::LeftBrace) {
                declarations.push(self.declaration()?);
            }
            let body_start = self
                .current_token()
                .expect("a function definition has an opening brace")
                .span;
            self.names.bind("__func__", NameClass::Ordinary, body_start);
            let function_start = start;
            let body = self.compound_statement(false)?;
            self.names.leave_scope(body.span);
            return Ok(ExternalItem::FunctionDefinition(Box::new(
                FunctionDefinition {
                    span: span_through(function_start, body.span),
                    specifiers,
                    declarator,
                    declarations,
                    body,
                },
            )));
        }

        let first = self.finish_init_declarator(declarator)?;
        let mut declarators = vec![first];
        while self.consume_punctuator(Punctuator::Comma).is_some() {
            let leading_attributes = self.attributes()?;
            let declarator = self.declarator_with_leading_attributes(leading_attributes)?;
            self.bind_declarator(&declarator, class);
            declarators.push(self.finish_init_declarator(declarator)?);
        }
        let semicolon =
            self.expect_punctuator(Punctuator::Semicolon, "expected `;` after declaration")?;
        Ok(ExternalItem::Declaration(Declaration {
            span: span_through(start, semicolon.span),
            specifiers,
            declarators,
        }))
    }

    fn declaration(&mut self) -> Result<Declaration, ParseError> {
        let checkpoint = self.names.checkpoint();
        let result = self.declaration_inner();
        match result {
            Ok(declaration) => {
                self.names.commit(checkpoint);
                Ok(declaration)
            }
            Err(error) => {
                self.names.rollback(checkpoint);
                Err(error)
            }
        }
    }

    fn declaration_inner(&mut self) -> Result<Declaration, ParseError> {
        let specifiers = self.declaration_specifiers()?;
        let start = specifiers.span;
        let class = if specifiers.is_typedef() {
            NameClass::TypedefName
        } else {
            NameClass::Ordinary
        };
        let mut declarators = Vec::new();
        if !self.check_punctuator(Punctuator::Semicolon) {
            loop {
                let leading_attributes = self.attributes()?;
                let declarator = self.declarator_with_leading_attributes(leading_attributes)?;
                self.bind_declarator(&declarator, class);
                declarators.push(self.finish_init_declarator(declarator)?);
                if self.consume_punctuator(Punctuator::Comma).is_none() {
                    break;
                }
            }
        }
        let semicolon =
            self.expect_punctuator(Punctuator::Semicolon, "expected `;` after declaration")?;
        Ok(Declaration {
            span: span_through(start, semicolon.span),
            specifiers,
            declarators,
        })
    }

    fn declarator_with_leading_attributes(
        &mut self,
        mut leading_attributes: Vec<Attribute>,
    ) -> Result<Declarator, ParseError> {
        let mut declarator = self.declarator(false)?;
        if let Some(first) = leading_attributes.first() {
            declarator.span = span_through(first.span, declarator.span);
            leading_attributes.append(&mut declarator.attributes);
            declarator.attributes = leading_attributes;
        }
        Ok(declarator)
    }

    fn finish_init_declarator(
        &mut self,
        declarator: Declarator,
    ) -> Result<InitDeclarator, ParseError> {
        let start = declarator.span;
        let mut attributes = self.attributes()?;
        let asm_label = if self.check_asm_keyword() {
            Some(self.asm_label()?)
        } else {
            None
        };
        attributes.extend(self.attributes()?);
        let initializer = if self.consume_punctuator(Punctuator::Assign).is_some() {
            Some(self.initializer()?)
        } else {
            None
        };
        let end = initializer
            .as_ref()
            .map(Initializer::span)
            .or_else(|| attributes.last().map(|attribute| attribute.span))
            .or_else(|| asm_label.as_ref().map(|label| label.span))
            .unwrap_or(declarator.span);
        Ok(InitDeclarator {
            span: span_through(start, end),
            declarator,
            asm_label,
            attributes,
            initializer,
        })
    }

    fn declaration_specifiers(&mut self) -> Result<DeclarationSpecifiers, ParseError> {
        let start = self.current_span()?;
        let mut items = Vec::new();
        let mut extension = false;
        let mut end = start;
        let mut saw_type_specifier = false;
        loop {
            let item = match self.current_token().map(|token| &token.kind) {
                Some(TokenKind::Keyword(Keyword::Extension)) => {
                    self.position += 1;
                    extension = true;
                    continue;
                }
                Some(TokenKind::Keyword(Keyword::Typedef)) => {
                    self.position += 1;
                    DeclarationSpecifier::StorageClass(StorageClass::Typedef)
                }
                Some(TokenKind::Keyword(Keyword::Extern)) => {
                    self.position += 1;
                    DeclarationSpecifier::StorageClass(StorageClass::Extern)
                }
                Some(TokenKind::Keyword(Keyword::Static)) => {
                    self.position += 1;
                    DeclarationSpecifier::StorageClass(StorageClass::Static)
                }
                Some(TokenKind::Keyword(Keyword::ThreadLocal)) => {
                    self.position += 1;
                    DeclarationSpecifier::StorageClass(StorageClass::ThreadLocal)
                }
                Some(TokenKind::Keyword(Keyword::GnuThreadLocal)) => {
                    self.position += 1;
                    DeclarationSpecifier::StorageClass(StorageClass::GnuThreadLocal)
                }
                Some(TokenKind::Keyword(Keyword::Auto)) => {
                    self.position += 1;
                    DeclarationSpecifier::StorageClass(StorageClass::Auto)
                }
                Some(TokenKind::Keyword(Keyword::Register)) => {
                    self.position += 1;
                    DeclarationSpecifier::StorageClass(StorageClass::Register)
                }
                Some(TokenKind::Keyword(Keyword::Const)) => {
                    self.position += 1;
                    DeclarationSpecifier::Qualifier(TypeQualifier::Const)
                }
                Some(TokenKind::Keyword(Keyword::Restrict)) => {
                    self.position += 1;
                    DeclarationSpecifier::Qualifier(TypeQualifier::Restrict)
                }
                Some(TokenKind::Keyword(Keyword::Volatile)) => {
                    self.position += 1;
                    DeclarationSpecifier::Qualifier(TypeQualifier::Volatile)
                }
                Some(TokenKind::Keyword(Keyword::Inline)) => {
                    self.position += 1;
                    DeclarationSpecifier::Function(FunctionSpecifier::Inline)
                }
                Some(TokenKind::Keyword(Keyword::Noreturn)) => {
                    self.position += 1;
                    DeclarationSpecifier::Function(FunctionSpecifier::NoReturn)
                }
                Some(TokenKind::Keyword(Keyword::Void)) => self.simple_type(TypeSpecifier::Void),
                Some(TokenKind::Keyword(Keyword::Char)) => self.simple_type(TypeSpecifier::Char),
                Some(TokenKind::Keyword(Keyword::Short)) => self.simple_type(TypeSpecifier::Short),
                Some(TokenKind::Keyword(Keyword::Int)) => self.simple_type(TypeSpecifier::Int),
                Some(TokenKind::Keyword(Keyword::Long)) => self.simple_type(TypeSpecifier::Long),
                Some(TokenKind::Keyword(Keyword::Float)) => self.simple_type(TypeSpecifier::Float),
                Some(TokenKind::Keyword(Keyword::Double)) => {
                    self.simple_type(TypeSpecifier::Double)
                }
                Some(TokenKind::Keyword(Keyword::Signed)) => {
                    self.simple_type(TypeSpecifier::Signed)
                }
                Some(TokenKind::Keyword(Keyword::Unsigned)) => {
                    self.simple_type(TypeSpecifier::Unsigned)
                }
                Some(TokenKind::Keyword(Keyword::Bool)) => self.simple_type(TypeSpecifier::Bool),
                Some(TokenKind::Keyword(Keyword::Complex)) => {
                    self.simple_type(TypeSpecifier::Complex)
                }
                Some(TokenKind::Keyword(Keyword::Imaginary)) => {
                    self.simple_type(TypeSpecifier::Imaginary)
                }
                Some(TokenKind::Keyword(Keyword::Struct)) => {
                    self.position += 1;
                    DeclarationSpecifier::Type(TypeSpecifier::Struct(Box::new(
                        self.record_specifier()?,
                    )))
                }
                Some(TokenKind::Keyword(Keyword::Union)) => {
                    self.position += 1;
                    DeclarationSpecifier::Type(TypeSpecifier::Union(Box::new(
                        self.record_specifier()?,
                    )))
                }
                Some(TokenKind::Keyword(Keyword::Enum)) => {
                    self.position += 1;
                    DeclarationSpecifier::Type(TypeSpecifier::Enum(Box::new(
                        self.enum_specifier()?,
                    )))
                }
                Some(TokenKind::Keyword(Keyword::Atomic))
                    if self.peek_punctuator(1, Punctuator::LeftParen) =>
                {
                    self.position += 1;
                    self.expect_punctuator(Punctuator::LeftParen, "expected `(` after `_Atomic`")?;
                    let ty = self.type_name()?;
                    self.expect_punctuator(Punctuator::RightParen, "expected `)` after type name")?;
                    DeclarationSpecifier::Type(TypeSpecifier::Atomic(Box::new(ty)))
                }
                Some(TokenKind::Keyword(Keyword::Atomic)) => {
                    self.position += 1;
                    DeclarationSpecifier::Qualifier(TypeQualifier::Atomic)
                }
                Some(TokenKind::Keyword(Keyword::Typeof)) => {
                    self.position += 1;
                    DeclarationSpecifier::Type(TypeSpecifier::Typeof(self.typeof_specifier()?))
                }
                Some(TokenKind::Identifier) if self.current_is_plain_gnu("typeof") => {
                    self.position += 1;
                    DeclarationSpecifier::Type(TypeSpecifier::Typeof(self.typeof_specifier()?))
                }
                Some(TokenKind::Identifier)
                    if self
                        .current_token()
                        .is_some_and(|token| token.spelling == "__builtin_va_list") =>
                {
                    self.position += 1;
                    DeclarationSpecifier::Type(TypeSpecifier::BuiltinVaList)
                }
                Some(TokenKind::Keyword(Keyword::Alignas)) => {
                    self.position += 1;
                    DeclarationSpecifier::Alignment(self.alignment_specifier()?)
                }
                Some(TokenKind::Keyword(Keyword::Attribute)) => {
                    let mut parsed = self.attributes()?;
                    if parsed.is_empty() {
                        break;
                    }
                    items.extend(parsed.drain(..).map(DeclarationSpecifier::Attribute));
                    end = self.previous_span();
                    continue;
                }
                Some(TokenKind::Identifier)
                    if self.current_identifier_is_typedef() && !saw_type_specifier =>
                {
                    let identifier = self.identifier()?;
                    DeclarationSpecifier::Type(TypeSpecifier::TypedefName(identifier))
                }
                _ => break,
            };
            if matches!(item, DeclarationSpecifier::Type(_)) {
                saw_type_specifier = true;
            }
            items.push(item);
            end = self.previous_span();
        }
        if items.is_empty() {
            return Err(self.error_current("expected declaration specifiers"));
        }
        Ok(DeclarationSpecifiers {
            items,
            extension,
            span: span_through(start, end),
        })
    }

    fn simple_type(&mut self, specifier: TypeSpecifier) -> DeclarationSpecifier {
        self.position += 1;
        DeclarationSpecifier::Type(specifier)
    }

    fn alignment_specifier(&mut self) -> Result<AlignmentSpecifier, ParseError> {
        self.expect_punctuator(Punctuator::LeftParen, "expected `(` after `_Alignas`")?;
        let result = if self.looks_like_type_name() {
            AlignmentSpecifier::Type(Box::new(self.type_name()?))
        } else {
            AlignmentSpecifier::Expression(Box::new(self.assignment_expression()?))
        };
        self.expect_punctuator(Punctuator::RightParen, "expected `)` after alignment")?;
        Ok(result)
    }

    fn typeof_specifier(&mut self) -> Result<TypeofSpecifier, ParseError> {
        self.expect_punctuator(Punctuator::LeftParen, "expected `(` after `typeof`")?;
        let result = if self.looks_like_type_name() {
            TypeofSpecifier::Type(Box::new(self.type_name()?))
        } else {
            TypeofSpecifier::Expression(Box::new(self.expression()?))
        };
        self.expect_punctuator(Punctuator::RightParen, "expected `)` after `typeof`")?;
        Ok(result)
    }

    fn record_specifier(&mut self) -> Result<RecordSpecifier, ParseError> {
        let start = self.previous_span();
        let leading_attributes = self.attributes()?;
        let tag = if self.check_identifier() {
            Some(self.identifier()?)
        } else {
            None
        };
        let mut attributes = leading_attributes;
        attributes.extend(self.attributes()?);
        let items = if self.consume_punctuator(Punctuator::LeftBrace).is_some() {
            let mut members = Vec::new();
            while !self.check_punctuator(Punctuator::RightBrace) {
                if let Some(pragma) = self.consume_pragma() {
                    members.push(RecordItem::Pragma(pragma));
                } else if self.check_keyword(Keyword::StaticAssert) {
                    members.push(RecordItem::StaticAssert(Box::new(self.static_assert()?)));
                } else {
                    members.push(RecordItem::Declaration(self.record_declaration()?));
                }
            }
            self.expect_punctuator(Punctuator::RightBrace, "expected `}` after record members")?;
            Some(members)
        } else {
            None
        };
        if tag.is_none() && items.is_none() {
            return Err(self.error_current("expected a tag or record definition"));
        }
        attributes.extend(self.attributes()?);
        let end = attributes
            .last()
            .map(|attribute| attribute.span)
            .or_else(|| items.as_ref().map(|_| self.previous_span()))
            .or_else(|| tag.as_ref().map(|tag| tag.span))
            .unwrap_or(start);
        Ok(RecordSpecifier {
            tag,
            attributes,
            items,
            span: span_through(start, end),
        })
    }

    fn record_declaration(&mut self) -> Result<RecordDeclaration, ParseError> {
        let specifiers = self.declaration_specifiers()?;
        let start = specifiers.span;
        let mut declarators = Vec::new();
        if !self.check_punctuator(Punctuator::Semicolon) {
            loop {
                let declarator = if self.check_punctuator(Punctuator::Colon) {
                    None
                } else {
                    Some(self.declarator(false)?)
                };
                let bit_width = if self.consume_punctuator(Punctuator::Colon).is_some() {
                    Some(self.conditional_expression()?)
                } else {
                    None
                };
                let attributes = self.attributes()?;
                let start = declarator.as_ref().map_or_else(
                    || bit_width.as_ref().unwrap().span,
                    |declarator| declarator.span,
                );
                let end = attributes.last().map_or_else(
                    || bit_width.as_ref().map_or(start, |width| width.span),
                    |a| a.span,
                );
                declarators.push(RecordDeclarator {
                    declarator,
                    bit_width,
                    attributes,
                    span: span_through(start, end),
                });
                if self.consume_punctuator(Punctuator::Comma).is_none() {
                    break;
                }
            }
        }
        let semicolon = self.expect_punctuator(
            Punctuator::Semicolon,
            "expected `;` after record declaration",
        )?;
        Ok(RecordDeclaration {
            specifiers,
            declarators,
            span: span_through(start, semicolon.span),
        })
    }

    fn enum_specifier(&mut self) -> Result<EnumSpecifier, ParseError> {
        let start = self.previous_span();
        let leading_attributes = self.attributes()?;
        let tag = if self.check_identifier() {
            Some(self.identifier()?)
        } else {
            None
        };
        let mut attributes = leading_attributes;
        attributes.extend(self.attributes()?);
        let enumerators = if self.consume_punctuator(Punctuator::LeftBrace).is_some() {
            let mut result = Vec::new();
            if !self.check_punctuator(Punctuator::RightBrace) {
                loop {
                    let leading_attributes = self.attributes()?;
                    let name = self.identifier()?;
                    let mut item_attributes = leading_attributes;
                    item_attributes.extend(self.attributes()?);
                    let value = if self.consume_punctuator(Punctuator::Assign).is_some() {
                        Some(self.conditional_expression()?)
                    } else {
                        None
                    };
                    let start = item_attributes
                        .first()
                        .map_or(name.span, |attribute| attribute.span);
                    let end = value
                        .as_ref()
                        .map(|value| value.span)
                        .or_else(|| item_attributes.last().map(|attribute| attribute.span))
                        .unwrap_or(name.span);
                    let span = span_through(start, end);
                    self.names
                        .bind(name.name.clone(), NameClass::Ordinary, name.span);
                    result.push(Enumerator {
                        name,
                        value,
                        attributes: item_attributes,
                        span,
                    });
                    if self.consume_punctuator(Punctuator::Comma).is_none()
                        || self.check_punctuator(Punctuator::RightBrace)
                    {
                        break;
                    }
                }
            }
            self.expect_punctuator(Punctuator::RightBrace, "expected `}` after enumerators")?;
            Some(result)
        } else {
            None
        };
        if tag.is_none() && enumerators.is_none() {
            return Err(self.error_current("expected an enum tag or definition"));
        }
        attributes.extend(self.attributes()?);
        let end = attributes
            .last()
            .map(|attribute| attribute.span)
            .or_else(|| enumerators.as_ref().map(|_| self.previous_span()))
            .or_else(|| tag.as_ref().map(|tag| tag.span))
            .unwrap_or(start);
        Ok(EnumSpecifier {
            tag,
            enumerators,
            attributes,
            span: span_through(start, end),
        })
    }

    fn declarator(&mut self, abstract_allowed: bool) -> Result<Declarator, ParseError> {
        self.nested(|parser| parser.declarator_inner(abstract_allowed))
    }

    fn declarator_inner(&mut self, abstract_allowed: bool) -> Result<Declarator, ParseError> {
        let start = self.current_span()?;
        let mut pointers = Vec::new();
        while let Some(star) = self.consume_punctuator(Punctuator::Star) {
            let mut qualifiers = Vec::new();
            let mut attributes = Vec::new();
            loop {
                if let Some(qualifier) = self.consume_type_qualifier() {
                    qualifiers.push(qualifier);
                } else if self.check_keyword(Keyword::Attribute) {
                    attributes.extend(self.attributes()?);
                } else {
                    break;
                }
            }
            let end = attributes
                .last()
                .map_or(star.span, |attribute| attribute.span);
            pointers.push(Pointer {
                qualifiers,
                attributes,
                span: span_through(star.span, end),
            });
        }

        let mut direct = if self.check_identifier() {
            DirectDeclarator::Identifier(self.identifier()?)
        } else if let Some(left) = self.consume_punctuator(Punctuator::LeftParen) {
            let nested = self.declarator(true)?;
            let right = self.expect_punctuator(
                Punctuator::RightParen,
                "expected `)` after nested declarator",
            )?;
            DirectDeclarator::Parenthesized(Box::new(nested), span_through(left.span, right.span))
        } else if abstract_allowed {
            DirectDeclarator::Abstract(start)
        } else {
            return Err(self.error_current("expected a declarator"));
        };

        loop {
            if let Some(left) = self.consume_punctuator(Punctuator::LeftBracket) {
                let mut qualifiers = Vec::new();
                let mut is_static = false;
                loop {
                    if self.consume_keyword(Keyword::Static).is_some() {
                        is_static = true;
                    } else if let Some(qualifier) = self.consume_type_qualifier() {
                        qualifiers.push(qualifier);
                    } else {
                        break;
                    }
                }
                let size = if self.consume_punctuator(Punctuator::Star).is_some() {
                    ArraySize::Star
                } else if self.check_punctuator(Punctuator::RightBracket) {
                    ArraySize::Unspecified
                } else {
                    ArraySize::Expression(Box::new(self.assignment_expression()?))
                };
                let right = self.expect_punctuator(
                    Punctuator::RightBracket,
                    "expected `]` after array declarator",
                )?;
                direct = DirectDeclarator::Array {
                    inner: Box::new(direct),
                    qualifiers,
                    is_static,
                    size,
                    span: span_through(left.span, right.span),
                };
            } else if let Some(left) = self.consume_punctuator(Punctuator::LeftParen) {
                self.names
                    .enter_scope(ScopeKind::FunctionPrototype, left.span);
                let (parameters, has_parameter_type_list, variadic, old_style_names) =
                    self.parameter_list()?;
                let right = self.expect_punctuator(
                    Punctuator::RightParen,
                    "expected `)` after function declarator",
                )?;
                self.names.leave_scope(right.span);
                direct = DirectDeclarator::Function {
                    inner: Box::new(direct),
                    parameters,
                    has_parameter_type_list,
                    variadic,
                    old_style_names,
                    span: span_through(left.span, right.span),
                };
            } else {
                break;
            }
        }
        let attributes = self.attributes()?;
        let end = attributes
            .last()
            .map_or_else(|| direct.span(), |attribute| attribute.span);
        Ok(Declarator {
            pointers,
            direct,
            attributes,
            span: span_through(start, end),
        })
    }

    fn parameter_list(
        &mut self,
    ) -> Result<(Vec<ParameterDeclaration>, bool, bool, Vec<Identifier>), ParseError> {
        if self.check_punctuator(Punctuator::RightParen) {
            return Ok((Vec::new(), false, false, Vec::new()));
        }
        if self.check_identifier()
            && !self.current_identifier_is_typedef()
            && (self.peek_punctuator(1, Punctuator::Comma)
                || self.peek_punctuator(1, Punctuator::RightParen))
        {
            let mut names = Vec::new();
            loop {
                let name = self.identifier()?;
                self.names
                    .bind(name.name.clone(), NameClass::Ordinary, name.span);
                names.push(name);
                if self.consume_punctuator(Punctuator::Comma).is_none() {
                    break;
                }
            }
            return Ok((Vec::new(), false, false, names));
        }

        let mut parameters = Vec::new();
        let mut variadic = false;
        loop {
            if self.consume_punctuator(Punctuator::Ellipsis).is_some() {
                variadic = true;
                break;
            }
            let specifiers = self.declaration_specifiers()?;
            let start = specifiers.span;
            let declarator = if self.starts_declarator() || self.starts_abstract_declarator() {
                Some(self.declarator(true)?)
            } else {
                None
            };
            if let Some(declarator) = &declarator {
                self.bind_declarator(declarator, NameClass::Ordinary);
            }
            let end = declarator
                .as_ref()
                .map_or(start, |declarator| declarator.span);
            parameters.push(ParameterDeclaration {
                specifiers,
                declarator,
                span: span_through(start, end),
            });
            if self.consume_punctuator(Punctuator::Comma).is_none() {
                break;
            }
            if self.check_punctuator(Punctuator::Ellipsis) {
                self.position += 1;
                variadic = true;
                break;
            }
        }
        Ok((parameters, true, variadic, Vec::new()))
    }

    fn type_name(&mut self) -> Result<TypeName, ParseError> {
        let specifiers = self.declaration_specifiers()?;
        let start = specifiers.span;
        let declarator = if self.starts_abstract_declarator() {
            Some(self.declarator(true)?)
        } else {
            None
        };
        let end = declarator
            .as_ref()
            .map_or(start, |declarator| declarator.span);
        Ok(TypeName {
            specifiers,
            declarator,
            span: span_through(start, end),
        })
    }

    fn attributes(&mut self) -> Result<Vec<Attribute>, ParseError> {
        let mut result = Vec::new();
        while let Some(keyword) = self.consume_keyword(Keyword::Attribute) {
            let introducer = keyword.spelling.clone();
            self.expect_punctuator(Punctuator::LeftParen, "expected `((` after attribute")?;
            self.expect_punctuator(Punctuator::LeftParen, "expected `((` after attribute")?;
            if !self.check_punctuator(Punctuator::RightParen) {
                loop {
                    let name = self.attribute_name()?;
                    let mut arguments = Vec::new();
                    let end = if let Some(left) = self.consume_punctuator(Punctuator::LeftParen) {
                        let mut depth = 1_usize;
                        let mut last = left.span;
                        while depth != 0 {
                            let token = self.current_token().ok_or_else(|| {
                                self.error_eof("unterminated attribute arguments")
                            })?;
                            match token.kind {
                                TokenKind::Punctuator(Punctuator::LeftParen) => depth += 1,
                                TokenKind::Punctuator(Punctuator::RightParen) => {
                                    depth -= 1;
                                    if depth == 0 {
                                        last = token.span;
                                        self.position += 1;
                                        break;
                                    }
                                }
                                _ => {}
                            }
                            if depth != 0 {
                                last = token.span;
                                arguments.push(token.clone());
                                self.position += 1;
                            }
                        }
                        last
                    } else {
                        name.span
                    };
                    result.push(Attribute {
                        introducer: introducer.clone(),
                        span: span_through(name.span, end),
                        name,
                        arguments,
                    });
                    if self.consume_punctuator(Punctuator::Comma).is_none() {
                        break;
                    }
                    if self.check_punctuator(Punctuator::RightParen) {
                        break;
                    }
                }
            }
            self.expect_punctuator(Punctuator::RightParen, "expected `))` after attributes")?;
            let right =
                self.expect_punctuator(Punctuator::RightParen, "expected `))` after attributes")?;
            if result.is_empty() {
                result.push(Attribute {
                    introducer,
                    name: Identifier {
                        name: String::new(),
                        span: keyword.span,
                    },
                    arguments: Vec::new(),
                    span: span_through(keyword.span, right.span),
                });
            }
        }
        Ok(result)
    }

    fn attribute_name(&mut self) -> Result<Identifier, ParseError> {
        let token = self
            .current_token()
            .ok_or_else(|| self.error_eof("expected an attribute name"))?
            .clone();
        if !matches!(token.kind, TokenKind::Identifier | TokenKind::Keyword(_)) {
            return Err(self.error_at(token.span, "expected an attribute name"));
        }
        self.position += 1;
        Ok(Identifier {
            name: token.spelling,
            span: token.span,
        })
    }

    fn asm_label(&mut self) -> Result<AsmLabel, ParseError> {
        let keyword = self
            .consume_asm_keyword()
            .expect("caller checked the keyword");
        self.expect_punctuator(Punctuator::LeftParen, "expected `(` after `asm`")?;
        let token = self
            .current_token()
            .ok_or_else(|| self.error_eof("expected a string literal in asm label"))?
            .clone();
        let TokenKind::String(literal) = token.kind else {
            return Err(self.error_at(token.span, "expected a string literal in asm label"));
        };
        self.position += 1;
        let right =
            self.expect_punctuator(Punctuator::RightParen, "expected `)` after asm label")?;
        Ok(AsmLabel {
            keyword_spelling: keyword.spelling,
            literal_spelling: token.spelling,
            literal,
            span: span_through(keyword.span, right.span),
        })
    }

    fn initializer(&mut self) -> Result<Initializer, ParseError> {
        if let Some(left) = self.consume_punctuator(Punctuator::LeftBrace) {
            let mut entries = Vec::new();
            if !self.check_punctuator(Punctuator::RightBrace) {
                loop {
                    let entry_start = self.current_span()?;
                    let mut designation = Vec::new();
                    while self.check_punctuator(Punctuator::LeftBracket)
                        || self.check_punctuator(Punctuator::Dot)
                    {
                        if self.consume_punctuator(Punctuator::LeftBracket).is_some() {
                            let index = self.conditional_expression()?;
                            self.expect_punctuator(
                                Punctuator::RightBracket,
                                "expected `]` after designator",
                            )?;
                            designation.push(Designator::Index(Box::new(index)));
                        } else {
                            self.position += 1;
                            designation.push(Designator::Member(self.identifier()?));
                        }
                    }
                    if !designation.is_empty() {
                        self.expect_punctuator(
                            Punctuator::Assign,
                            "expected `=` after designators",
                        )?;
                    }
                    let initializer = self.initializer()?;
                    let end = initializer.span();
                    entries.push(InitializerEntry {
                        designation,
                        initializer,
                        span: span_through(entry_start, end),
                    });
                    if self.consume_punctuator(Punctuator::Comma).is_none()
                        || self.check_punctuator(Punctuator::RightBrace)
                    {
                        break;
                    }
                }
            }
            let right = self.expect_punctuator(
                Punctuator::RightBrace,
                "expected `}` after initializer list",
            )?;
            Ok(Initializer::List {
                entries,
                span: span_through(left.span, right.span),
            })
        } else {
            self.assignment_expression()
                .map(Box::new)
                .map(Initializer::Expression)
        }
    }

    fn static_assert(&mut self) -> Result<StaticAssert, ParseError> {
        let keyword = self
            .consume_keyword(Keyword::StaticAssert)
            .expect("caller checked `_Static_assert`");
        self.expect_punctuator(Punctuator::LeftParen, "expected `(` after `_Static_assert`")?;
        let condition = self.conditional_expression()?;
        let message = if self.consume_punctuator(Punctuator::Comma).is_some() {
            let token = self
                .current_token()
                .ok_or_else(|| self.error_eof("expected a static assertion message"))?
                .clone();
            let TokenKind::String(message) = token.kind else {
                return Err(self.error_at(token.span, "expected a string literal message"));
            };
            self.position += 1;
            Some(message)
        } else {
            None
        };
        self.expect_punctuator(
            Punctuator::RightParen,
            "expected `)` after static assertion",
        )?;
        let semicolon =
            self.expect_punctuator(Punctuator::Semicolon, "expected `;` after static assertion")?;
        Ok(StaticAssert {
            condition,
            message,
            span: span_through(keyword.span, semicolon.span),
        })
    }

    fn compound_statement(&mut self, create_scope: bool) -> Result<Statement, ParseError> {
        let left = self.expect_punctuator(Punctuator::LeftBrace, "expected `{`")?;
        if create_scope {
            self.names.enter_scope(ScopeKind::Block, left.span);
        }
        let mut items = Vec::new();
        while !self.check_punctuator(Punctuator::RightBrace) {
            if self.current_item().is_none() {
                return Err(self.error_eof("expected `}` after block"));
            }
            if let Some(pragma) = self.consume_pragma() {
                items.push(BlockItem::Pragma(pragma));
            } else if self.check_keyword(Keyword::StaticAssert) {
                items.push(BlockItem::StaticAssert(Box::new(self.static_assert()?)));
            } else if self.starts_declaration() {
                items.push(BlockItem::Declaration(self.declaration()?));
            } else {
                items.push(BlockItem::Statement(Box::new(self.statement()?)));
            }
        }
        let right = self.expect_punctuator(Punctuator::RightBrace, "expected `}` after block")?;
        if create_scope {
            self.names.leave_scope(right.span);
        }
        Ok(Statement {
            kind: StatementKind::Compound(items),
            span: span_through(left.span, right.span),
        })
    }

    fn statement(&mut self) -> Result<Statement, ParseError> {
        if self.check_punctuator(Punctuator::LeftBrace) {
            return self.compound_statement(true);
        }
        if self.check_identifier() && self.peek_punctuator(1, Punctuator::Colon) {
            let label = self.identifier()?;
            self.expect_punctuator(Punctuator::Colon, "expected `:` after label")?;
            let attributes = self.attributes()?;
            let nested = Box::new(self.statement()?);
            return Ok(Statement {
                span: span_through(label.span, nested.span),
                kind: StatementKind::Label {
                    label,
                    statement: nested,
                    attributes,
                },
            });
        }
        if let Some(keyword) = self.consume_keyword(Keyword::Case) {
            let value = self.conditional_expression()?;
            self.expect_punctuator(Punctuator::Colon, "expected `:` after case value")?;
            let statement = Box::new(self.statement()?);
            return Ok(Statement {
                span: span_through(keyword.span, statement.span),
                kind: StatementKind::Case {
                    value: Box::new(value),
                    statement,
                },
            });
        }
        if let Some(keyword) = self.consume_keyword(Keyword::Default) {
            self.expect_punctuator(Punctuator::Colon, "expected `:` after default")?;
            let statement = Box::new(self.statement()?);
            return Ok(Statement {
                span: span_through(keyword.span, statement.span),
                kind: StatementKind::Default(statement),
            });
        }
        if let Some(keyword) = self.consume_keyword(Keyword::If) {
            let condition = self.parenthesized_expression("if")?;
            let then_statement = Box::new(self.statement()?);
            let else_statement = if self.consume_keyword(Keyword::Else).is_some() {
                Some(Box::new(self.statement()?))
            } else {
                None
            };
            let end = else_statement
                .as_ref()
                .map_or(then_statement.span, |statement| statement.span);
            return Ok(Statement {
                span: span_through(keyword.span, end),
                kind: StatementKind::If {
                    condition: Box::new(condition),
                    then_statement,
                    else_statement,
                },
            });
        }
        if let Some(keyword) = self.consume_keyword(Keyword::Switch) {
            let expression = self.parenthesized_expression("switch")?;
            let statement = Box::new(self.statement()?);
            return Ok(Statement {
                span: span_through(keyword.span, statement.span),
                kind: StatementKind::Switch {
                    expression: Box::new(expression),
                    statement,
                },
            });
        }
        if let Some(keyword) = self.consume_keyword(Keyword::While) {
            let condition = self.parenthesized_expression("while")?;
            let statement = Box::new(self.statement()?);
            return Ok(Statement {
                span: span_through(keyword.span, statement.span),
                kind: StatementKind::While {
                    condition: Box::new(condition),
                    statement,
                },
            });
        }
        if let Some(keyword) = self.consume_keyword(Keyword::Do) {
            let statement = Box::new(self.statement()?);
            self.expect_keyword(Keyword::While, "expected `while` after `do` body")?;
            let condition = self.parenthesized_expression("while")?;
            let semicolon = self.expect_punctuator(
                Punctuator::Semicolon,
                "expected `;` after do-while statement",
            )?;
            return Ok(Statement {
                span: span_through(keyword.span, semicolon.span),
                kind: StatementKind::DoWhile {
                    statement,
                    condition: Box::new(condition),
                },
            });
        }
        if let Some(keyword) = self.consume_keyword(Keyword::For) {
            return self.for_statement(keyword.span);
        }
        if let Some(keyword) = self.consume_keyword(Keyword::Goto) {
            if self.language_mode == LanguageMode::Gnu11
                && self.consume_punctuator(Punctuator::Star).is_some()
            {
                let expression = self.expression()?;
                let semicolon = self.expect_punctuator(
                    Punctuator::Semicolon,
                    "expected `;` after computed goto expression",
                )?;
                return Ok(Statement {
                    span: span_through(keyword.span, semicolon.span),
                    kind: StatementKind::ComputedGoto(Box::new(expression)),
                });
            }
            let label = self.identifier()?;
            let semicolon =
                self.expect_punctuator(Punctuator::Semicolon, "expected `;` after goto label")?;
            return Ok(Statement {
                span: span_through(keyword.span, semicolon.span),
                kind: StatementKind::Goto(label),
            });
        }
        for (keyword, kind) in [
            (Keyword::Continue, StatementKind::Continue),
            (Keyword::Break, StatementKind::Break),
        ] {
            if let Some(token) = self.consume_keyword(keyword) {
                let semicolon = self.expect_punctuator(
                    Punctuator::Semicolon,
                    "expected `;` after jump statement",
                )?;
                return Ok(Statement {
                    span: span_through(token.span, semicolon.span),
                    kind,
                });
            }
        }
        if let Some(keyword) = self.consume_keyword(Keyword::Return) {
            let expression = if self.check_punctuator(Punctuator::Semicolon) {
                None
            } else {
                Some(self.expression()?)
            };
            let semicolon = self
                .expect_punctuator(Punctuator::Semicolon, "expected `;` after return statement")?;
            return Ok(Statement {
                span: span_through(keyword.span, semicolon.span),
                kind: StatementKind::Return(expression.map(Box::new)),
            });
        }
        if let Some(semicolon) = self.consume_punctuator(Punctuator::Semicolon) {
            return Ok(Statement {
                kind: StatementKind::Expression(None),
                span: semicolon.span,
            });
        }
        let expression = self.expression()?;
        let semicolon =
            self.expect_punctuator(Punctuator::Semicolon, "expected `;` after expression")?;
        Ok(Statement {
            span: span_through(expression.span, semicolon.span),
            kind: StatementKind::Expression(Some(Box::new(expression))),
        })
    }

    fn for_statement(&mut self, start: Span) -> Result<Statement, ParseError> {
        let left = self.expect_punctuator(Punctuator::LeftParen, "expected `(` after `for`")?;
        self.names.enter_scope(ScopeKind::For, left.span);
        let initializer = if self.consume_punctuator(Punctuator::Semicolon).is_some() {
            ForInitializer::Empty
        } else if self.starts_declaration() {
            ForInitializer::Declaration(Box::new(self.declaration()?))
        } else {
            let expression = self.expression()?;
            self.expect_punctuator(Punctuator::Semicolon, "expected `;` after for initializer")?;
            ForInitializer::Expression(Box::new(expression))
        };
        let condition = if self.check_punctuator(Punctuator::Semicolon) {
            None
        } else {
            Some(self.expression()?)
        };
        self.expect_punctuator(Punctuator::Semicolon, "expected `;` in for statement")?;
        let step = if self.check_punctuator(Punctuator::RightParen) {
            None
        } else {
            Some(self.expression()?)
        };
        self.expect_punctuator(Punctuator::RightParen, "expected `)` after for clauses")?;
        let statement = Box::new(self.statement()?);
        self.names.leave_scope(statement.span);
        Ok(Statement {
            span: span_through(start, statement.span),
            kind: StatementKind::For {
                initializer: Box::new(initializer),
                condition: condition.map(Box::new),
                step: step.map(Box::new),
                statement,
            },
        })
    }

    fn parenthesized_expression(&mut self, construct: &str) -> Result<Expression, ParseError> {
        self.expect_punctuator(
            Punctuator::LeftParen,
            &format!("expected `(` after `{construct}`"),
        )?;
        let expression = self.expression()?;
        self.expect_punctuator(Punctuator::RightParen, "expected `)` after expression")?;
        Ok(expression)
    }

    fn expression(&mut self) -> Result<Expression, ParseError> {
        let first = self.assignment_expression()?;
        if self.consume_punctuator(Punctuator::Comma).is_none() {
            return Ok(first);
        }
        let start = first.span;
        let mut expressions = vec![first];
        loop {
            expressions.push(self.assignment_expression()?);
            if self.consume_punctuator(Punctuator::Comma).is_none() {
                break;
            }
        }
        let end = expressions.last().expect("the list is nonempty").span;
        Ok(Expression {
            kind: ExpressionKind::Comma(expressions),
            span: span_through(start, end),
        })
    }

    fn assignment_expression(&mut self) -> Result<Expression, ParseError> {
        let target = self.conditional_expression()?;
        let Some(operator) = self.consume_assignment_operator() else {
            return Ok(target);
        };
        let value = self.nested(Self::assignment_expression)?;
        let span = span_through(target.span, value.span);
        Ok(Expression {
            kind: ExpressionKind::Assignment {
                operator,
                target: Box::new(target),
                value: Box::new(value),
            },
            span,
        })
    }

    fn conditional_expression(&mut self) -> Result<Expression, ParseError> {
        let condition = self.binary_expression(1)?;
        if self.consume_punctuator(Punctuator::Question).is_none() {
            return Ok(condition);
        }
        let then_expression = self.expression()?;
        self.expect_punctuator(Punctuator::Colon, "expected `:` in conditional expression")?;
        let else_expression = self.nested(Self::conditional_expression)?;
        let span = span_through(condition.span, else_expression.span);
        Ok(Expression {
            kind: ExpressionKind::Conditional {
                condition: Box::new(condition),
                then_expression: Box::new(then_expression),
                else_expression: Box::new(else_expression),
            },
            span,
        })
    }

    fn binary_expression(&mut self, minimum_precedence: u8) -> Result<Expression, ParseError> {
        let mut left = self.cast_expression()?;
        while let Some((precedence, operator)) = self.current_binary_operator() {
            if precedence < minimum_precedence {
                break;
            }
            self.position += 1;
            let right = self.binary_expression(precedence + 1)?;
            let span = span_through(left.span, right.span);
            left = Expression {
                kind: ExpressionKind::Binary {
                    operator,
                    left: Box::new(left),
                    right: Box::new(right),
                },
                span,
            };
        }
        Ok(left)
    }

    fn cast_expression(&mut self) -> Result<Expression, ParseError> {
        if self.check_punctuator(Punctuator::LeftParen) && self.looks_like_type_name_at(1) {
            let left = self
                .consume_punctuator(Punctuator::LeftParen)
                .expect("the opening parenthesis was checked");
            let ty = self.type_name()?;
            self.expect_punctuator(Punctuator::RightParen, "expected `)` after type name")?;
            if self.check_punctuator(Punctuator::LeftBrace) {
                let initializer = self.initializer()?;
                let span = span_through(left.span, initializer.span());
                let expression = Expression {
                    kind: ExpressionKind::CompoundLiteral {
                        ty,
                        initializer: Box::new(initializer),
                    },
                    span,
                };
                return self.postfix_expression_suffix(expression);
            }
            let expression = self.nested(Self::cast_expression)?;
            let span = span_through(left.span, expression.span);
            return Ok(Expression {
                kind: ExpressionKind::Cast {
                    ty,
                    expression: Box::new(expression),
                },
                span,
            });
        }
        self.unary_expression()
    }

    fn unary_expression(&mut self) -> Result<Expression, ParseError> {
        if self.language_mode == LanguageMode::Gnu11
            && let Some(operator) = self.consume_punctuator(Punctuator::AmpAmp)
        {
            let label = self.identifier()?;
            return Ok(Expression {
                span: span_through(operator.span, label.span),
                kind: ExpressionKind::LabelAddress(label),
            });
        }
        if let Some(extension) = self.consume_keyword(Keyword::Extension) {
            let expression = self.nested(Self::cast_expression)?;
            return Ok(Expression {
                span: span_through(extension.span, expression.span),
                kind: ExpressionKind::Extension(Box::new(expression)),
            });
        }
        if let Some(keyword) = self.consume_keyword(Keyword::Sizeof) {
            if self.check_punctuator(Punctuator::LeftParen) && self.looks_like_type_name_at(1) {
                self.position += 1;
                let ty = self.type_name()?;
                let right = self
                    .expect_punctuator(Punctuator::RightParen, "expected `)` after sizeof type")?;
                return Ok(Expression {
                    kind: ExpressionKind::SizeofType(ty),
                    span: span_through(keyword.span, right.span),
                });
            }
            let operand = self.nested(Self::unary_expression)?;
            return Ok(Expression {
                span: span_through(keyword.span, operand.span),
                kind: ExpressionKind::SizeofExpression(Box::new(operand)),
            });
        }
        if let Some(keyword) = self.consume_keyword(Keyword::Alignof) {
            self.expect_punctuator(Punctuator::LeftParen, "expected `(` after `_Alignof`")?;
            let ty = self.type_name()?;
            let right = self
                .expect_punctuator(Punctuator::RightParen, "expected `)` after `_Alignof` type")?;
            return Ok(Expression {
                kind: ExpressionKind::AlignofType(ty),
                span: span_through(keyword.span, right.span),
            });
        }
        let unary = [
            (Punctuator::PlusPlus, UnaryOperator::PrefixIncrement),
            (Punctuator::MinusMinus, UnaryOperator::PrefixDecrement),
            (Punctuator::Amp, UnaryOperator::Address),
            (Punctuator::Star, UnaryOperator::Dereference),
            (Punctuator::Plus, UnaryOperator::Plus),
            (Punctuator::Minus, UnaryOperator::Minus),
            (Punctuator::Tilde, UnaryOperator::BitwiseNot),
            (Punctuator::Bang, UnaryOperator::LogicalNot),
        ];
        for (punctuator, operator) in unary {
            if let Some(token) = self.consume_punctuator(punctuator) {
                let operand = self.nested(Self::cast_expression)?;
                return Ok(Expression {
                    span: span_through(token.span, operand.span),
                    kind: ExpressionKind::Unary {
                        operator,
                        operand: Box::new(operand),
                    },
                });
            }
        }
        self.postfix_expression()
    }

    fn postfix_expression(&mut self) -> Result<Expression, ParseError> {
        let expression = self.primary_expression()?;
        self.postfix_expression_suffix(expression)
    }

    fn postfix_expression_suffix(
        &mut self,
        mut expression: Expression,
    ) -> Result<Expression, ParseError> {
        loop {
            if self.consume_punctuator(Punctuator::LeftBracket).is_some() {
                let index = self.expression()?;
                let right = self
                    .expect_punctuator(Punctuator::RightBracket, "expected `]` after subscript")?;
                let start = expression.span;
                expression = Expression {
                    kind: ExpressionKind::Subscript {
                        base: Box::new(expression),
                        index: Box::new(index),
                    },
                    span: span_through(start, right.span),
                };
            } else if self.consume_punctuator(Punctuator::LeftParen).is_some() {
                let mut arguments = Vec::new();
                if !self.check_punctuator(Punctuator::RightParen) {
                    loop {
                        arguments.push(self.assignment_expression()?);
                        if self.consume_punctuator(Punctuator::Comma).is_none() {
                            break;
                        }
                    }
                }
                let right =
                    self.expect_punctuator(Punctuator::RightParen, "expected `)` after arguments")?;
                let start = expression.span;
                expression = Expression {
                    kind: ExpressionKind::Call {
                        callee: Box::new(expression),
                        arguments,
                    },
                    span: span_through(start, right.span),
                };
            } else if self.check_punctuator(Punctuator::Dot)
                || self.check_punctuator(Punctuator::Arrow)
            {
                let indirect = self.consume_punctuator(Punctuator::Arrow).is_some();
                if !indirect {
                    self.position += 1;
                }
                let member = self.identifier()?;
                let start = expression.span;
                expression = Expression {
                    span: span_through(start, member.span),
                    kind: ExpressionKind::Member {
                        base: Box::new(expression),
                        member,
                        indirect,
                    },
                };
            } else if let Some(token) = self.consume_punctuator(Punctuator::PlusPlus) {
                let start = expression.span;
                expression = Expression {
                    kind: ExpressionKind::PostfixIncrement(Box::new(expression)),
                    span: span_through(start, token.span),
                };
            } else if let Some(token) = self.consume_punctuator(Punctuator::MinusMinus) {
                let start = expression.span;
                expression = Expression {
                    kind: ExpressionKind::PostfixDecrement(Box::new(expression)),
                    span: span_through(start, token.span),
                };
            } else {
                break;
            }
        }
        Ok(expression)
    }

    fn primary_expression(&mut self) -> Result<Expression, ParseError> {
        let token = self
            .current_token()
            .ok_or_else(|| self.error_current("expected an expression"))?
            .clone();
        if token.spelling == "__builtin_offsetof" {
            return self.builtin_offsetof();
        }
        if token.spelling == "__builtin_va_start" {
            return self.builtin_va_start();
        }
        if token.spelling == "__builtin_va_arg" {
            return self.builtin_va_arg();
        }
        if token.spelling == "__builtin_va_copy" {
            return self.builtin_va_copy();
        }
        if token.spelling == "__builtin_va_end" {
            return self.builtin_va_end();
        }
        if token.spelling == "__builtin_expect" {
            return self.builtin_expect();
        }
        if token.spelling == "__builtin_huge_val" {
            return self.builtin_huge_val();
        }
        if token.spelling == "__builtin_inff" {
            return self.builtin_inff();
        }
        if token.spelling == "__builtin_nanf" {
            return self.builtin_nanf();
        }
        let integer_intrinsic = match token.spelling.as_str() {
            "__builtin_bswap64" => Some(IntegerBuiltinOperation::ByteSwap64),
            "__builtin_clz" => Some(IntegerBuiltinOperation::CountLeadingZerosInt),
            "__builtin_clzl" => Some(IntegerBuiltinOperation::CountLeadingZerosLong),
            "__builtin_clzll" => Some(IntegerBuiltinOperation::CountLeadingZerosLongLong),
            "__builtin_ctzll" => Some(IntegerBuiltinOperation::CountTrailingZerosLongLong),
            "__builtin_popcount" => Some(IntegerBuiltinOperation::PopulationCountInt),
            "__builtin_popcountll" => Some(IntegerBuiltinOperation::PopulationCountLongLong),
            _ => None,
        };
        if let Some(operation) = integer_intrinsic {
            return self.builtin_integer_intrinsic(operation);
        }
        if token.spelling == "__builtin_prefetch" {
            return self.builtin_prefetch();
        }
        let sync_operation = match token.spelling.as_str() {
            "__sync_add_and_fetch" => Some(SyncBuiltinOperation::AddAndFetch),
            "__sync_fetch_and_add" => Some(SyncBuiltinOperation::FetchAndAdd),
            "__sync_sub_and_fetch" => Some(SyncBuiltinOperation::SubAndFetch),
            "__sync_bool_compare_and_swap" => Some(SyncBuiltinOperation::BoolCompareAndSwap),
            "__sync_val_compare_and_swap" => Some(SyncBuiltinOperation::ValCompareAndSwap),
            "__sync_lock_test_and_set" => Some(SyncBuiltinOperation::LockTestAndSet),
            _ => None,
        };
        if let Some(operation) = sync_operation {
            return self.builtin_sync_operation(operation);
        }
        if token.spelling == "__sync_synchronize" {
            return self.builtin_sync_synchronize();
        }
        match token.kind {
            TokenKind::Identifier => {
                self.position += 1;
                Ok(Expression {
                    kind: ExpressionKind::Identifier(Identifier {
                        name: token.spelling,
                        span: token.span,
                    }),
                    span: token.span,
                })
            }
            TokenKind::Integer(value) => {
                self.position += 1;
                Ok(Expression {
                    kind: ExpressionKind::Integer(value),
                    span: token.span,
                })
            }
            TokenKind::Floating(value) => {
                self.position += 1;
                Ok(Expression {
                    kind: ExpressionKind::Floating(value),
                    span: token.span,
                })
            }
            TokenKind::Character(value) => {
                self.position += 1;
                Ok(Expression {
                    kind: ExpressionKind::Character(value),
                    span: token.span,
                })
            }
            TokenKind::String(value) => {
                self.position += 1;
                Ok(Expression {
                    kind: ExpressionKind::String(value),
                    span: token.span,
                })
            }
            TokenKind::Keyword(Keyword::Generic) => self.generic_selection(),
            TokenKind::Punctuator(Punctuator::LeftParen) => {
                self.position += 1;
                let expression = self.expression()?;
                let right = self
                    .expect_punctuator(Punctuator::RightParen, "expected `)` after expression")?;
                Ok(Expression {
                    kind: ExpressionKind::Parenthesized(Box::new(expression)),
                    span: span_through(token.span, right.span),
                })
            }
            _ => Err(self.error_at(token.span, "expected a primary expression")),
        }
    }

    fn generic_selection(&mut self) -> Result<Expression, ParseError> {
        let keyword = self
            .consume_keyword(Keyword::Generic)
            .expect("caller checked `_Generic`");
        self.expect_punctuator(Punctuator::LeftParen, "expected `(` after `_Generic`")?;
        let controlling = self.assignment_expression()?;
        self.expect_punctuator(
            Punctuator::Comma,
            "expected `,` after controlling expression",
        )?;
        let mut associations = Vec::new();
        loop {
            let start = self.current_span()?;
            let ty = if self.consume_keyword(Keyword::Default).is_some() {
                None
            } else {
                Some(self.type_name()?)
            };
            self.expect_punctuator(Punctuator::Colon, "expected `:` in generic association")?;
            let expression = self.assignment_expression()?;
            associations.push(GenericAssociation {
                span: span_through(start, expression.span),
                ty,
                expression,
            });
            if self.consume_punctuator(Punctuator::Comma).is_none() {
                break;
            }
        }
        let right = self.expect_punctuator(
            Punctuator::RightParen,
            "expected `)` after generic selection",
        )?;
        Ok(Expression {
            span: span_through(keyword.span, right.span),
            kind: ExpressionKind::GenericSelection {
                controlling: Box::new(controlling),
                associations,
            },
        })
    }

    fn builtin_offsetof(&mut self) -> Result<Expression, ParseError> {
        let builtin = self
            .current_token()
            .expect("caller checked builtin")
            .clone();
        self.position += 1;
        self.expect_punctuator(
            Punctuator::LeftParen,
            "expected `(` after `__builtin_offsetof`",
        )?;
        let ty = self.type_name()?;
        self.expect_punctuator(Punctuator::Comma, "expected `,` after offsetof type name")?;
        let mut designator = vec![OffsetDesignator::Member(self.identifier()?)];
        loop {
            if self.consume_punctuator(Punctuator::Dot).is_some() {
                designator.push(OffsetDesignator::Member(self.identifier()?));
            } else if self.consume_punctuator(Punctuator::LeftBracket).is_some() {
                let index = self.expression()?;
                self.expect_punctuator(
                    Punctuator::RightBracket,
                    "expected `]` in offsetof designator",
                )?;
                designator.push(OffsetDesignator::Index(Box::new(index)));
            } else {
                break;
            }
        }
        let right = self.expect_punctuator(
            Punctuator::RightParen,
            "expected `)` after offsetof designator",
        )?;
        Ok(Expression {
            span: span_through(builtin.span, right.span),
            kind: ExpressionKind::BuiltinOffsetof {
                ty: Box::new(ty),
                designator,
            },
        })
    }

    fn builtin_va_start(&mut self) -> Result<Expression, ParseError> {
        let builtin = self
            .current_token()
            .expect("caller checked builtin")
            .clone();
        self.position += 1;
        self.expect_punctuator(
            Punctuator::LeftParen,
            "expected `(` after `__builtin_va_start`",
        )?;
        let list = self.assignment_expression()?;
        self.expect_punctuator(Punctuator::Comma, "expected `,` after va_list expression")?;
        let last_named_parameter = self.assignment_expression()?;
        let right = self.expect_punctuator(
            Punctuator::RightParen,
            "expected `)` after `__builtin_va_start` arguments",
        )?;
        Ok(Expression {
            kind: ExpressionKind::BuiltinVaStart {
                list: Box::new(list),
                last_named_parameter: Box::new(last_named_parameter),
            },
            span: span_through(builtin.span, right.span),
        })
    }

    fn builtin_va_arg(&mut self) -> Result<Expression, ParseError> {
        let builtin = self
            .current_token()
            .expect("caller checked builtin")
            .clone();
        self.position += 1;
        self.expect_punctuator(
            Punctuator::LeftParen,
            "expected `(` after `__builtin_va_arg`",
        )?;
        let list = self.assignment_expression()?;
        self.expect_punctuator(Punctuator::Comma, "expected `,` after va_list expression")?;
        let ty = self.type_name()?;
        let right = self.expect_punctuator(
            Punctuator::RightParen,
            "expected `)` after `__builtin_va_arg` type",
        )?;
        Ok(Expression {
            kind: ExpressionKind::BuiltinVaArg {
                list: Box::new(list),
                ty: Box::new(ty),
            },
            span: span_through(builtin.span, right.span),
        })
    }

    fn builtin_va_copy(&mut self) -> Result<Expression, ParseError> {
        let builtin = self
            .current_token()
            .expect("caller checked builtin")
            .clone();
        self.position += 1;
        self.expect_punctuator(
            Punctuator::LeftParen,
            "expected `(` after `__builtin_va_copy`",
        )?;
        let destination = self.assignment_expression()?;
        self.expect_punctuator(Punctuator::Comma, "expected `,` after destination va_list")?;
        let source = self.assignment_expression()?;
        let right = self.expect_punctuator(
            Punctuator::RightParen,
            "expected `)` after `__builtin_va_copy` arguments",
        )?;
        Ok(Expression {
            kind: ExpressionKind::BuiltinVaCopy {
                destination: Box::new(destination),
                source: Box::new(source),
            },
            span: span_through(builtin.span, right.span),
        })
    }

    fn builtin_va_end(&mut self) -> Result<Expression, ParseError> {
        let builtin = self
            .current_token()
            .expect("caller checked builtin")
            .clone();
        self.position += 1;
        self.expect_punctuator(
            Punctuator::LeftParen,
            "expected `(` after `__builtin_va_end`",
        )?;
        let list = self.assignment_expression()?;
        let right = self.expect_punctuator(
            Punctuator::RightParen,
            "expected `)` after `__builtin_va_end` argument",
        )?;
        Ok(Expression {
            kind: ExpressionKind::BuiltinVaEnd {
                list: Box::new(list),
            },
            span: span_through(builtin.span, right.span),
        })
    }

    fn builtin_expect(&mut self) -> Result<Expression, ParseError> {
        let builtin = self
            .current_token()
            .expect("caller checked builtin")
            .clone();
        self.position += 1;
        self.expect_punctuator(
            Punctuator::LeftParen,
            "expected `(` after `__builtin_expect`",
        )?;
        let value = self.assignment_expression()?;
        self.expect_punctuator(Punctuator::Comma, "expected `,` after predicted value")?;
        let expected = self.assignment_expression()?;
        let right = self.expect_punctuator(
            Punctuator::RightParen,
            "expected `)` after `__builtin_expect` arguments",
        )?;
        Ok(Expression {
            kind: ExpressionKind::BuiltinExpect {
                value: Box::new(value),
                expected: Box::new(expected),
            },
            span: span_through(builtin.span, right.span),
        })
    }

    fn builtin_huge_val(&mut self) -> Result<Expression, ParseError> {
        let builtin = self
            .current_token()
            .expect("caller checked builtin")
            .clone();
        self.position += 1;
        self.expect_punctuator(
            Punctuator::LeftParen,
            "expected `(` after `__builtin_huge_val`",
        )?;
        let right = self.expect_punctuator(
            Punctuator::RightParen,
            "`__builtin_huge_val` requires exactly zero arguments",
        )?;
        Ok(Expression {
            kind: ExpressionKind::BuiltinHugeVal,
            span: span_through(builtin.span, right.span),
        })
    }

    fn builtin_inff(&mut self) -> Result<Expression, ParseError> {
        let builtin = self
            .current_token()
            .expect("caller checked builtin")
            .clone();
        self.position += 1;
        self.expect_punctuator(Punctuator::LeftParen, "expected `(` after `__builtin_inff`")?;
        let right = self.expect_punctuator(
            Punctuator::RightParen,
            "`__builtin_inff` requires exactly zero arguments",
        )?;
        Ok(Expression {
            kind: ExpressionKind::BuiltinInfF,
            span: span_through(builtin.span, right.span),
        })
    }

    fn builtin_nanf(&mut self) -> Result<Expression, ParseError> {
        let builtin = self
            .current_token()
            .expect("caller checked builtin")
            .clone();
        self.position += 1;
        self.expect_punctuator(Punctuator::LeftParen, "expected `(` after `__builtin_nanf`")?;
        let payload = self.assignment_expression()?;
        let right = self.expect_punctuator(
            Punctuator::RightParen,
            "`__builtin_nanf` requires exactly one argument",
        )?;
        Ok(Expression {
            kind: ExpressionKind::BuiltinNanF {
                payload: Box::new(payload),
            },
            span: span_through(builtin.span, right.span),
        })
    }

    fn builtin_sync_synchronize(&mut self) -> Result<Expression, ParseError> {
        let builtin = self
            .current_token()
            .expect("caller checked builtin")
            .clone();
        self.position += 1;
        self.expect_punctuator(
            Punctuator::LeftParen,
            "expected `(` after `__sync_synchronize`",
        )?;
        let right = self.expect_punctuator(
            Punctuator::RightParen,
            "`__sync_synchronize` requires exactly zero arguments",
        )?;
        Ok(Expression {
            kind: ExpressionKind::BuiltinSyncSynchronize,
            span: span_through(builtin.span, right.span),
        })
    }

    fn builtin_integer_intrinsic(
        &mut self,
        operation: IntegerBuiltinOperation,
    ) -> Result<Expression, ParseError> {
        let builtin = self
            .current_token()
            .expect("caller checked builtin")
            .clone();
        self.position += 1;
        self.expect_punctuator(
            Punctuator::LeftParen,
            &format!("expected `(` after `{}`", operation.spelling()),
        )?;
        if self.check_punctuator(Punctuator::RightParen) {
            return Err(self.error_current(&format!(
                "`{}` requires exactly one argument",
                operation.spelling()
            )));
        }
        let operand = self.assignment_expression()?;
        let right = self.expect_punctuator(
            Punctuator::RightParen,
            &format!("`{}` requires exactly one argument", operation.spelling()),
        )?;
        Ok(Expression {
            kind: ExpressionKind::BuiltinIntegerIntrinsic {
                operation,
                operand: Box::new(operand),
            },
            span: span_through(builtin.span, right.span),
        })
    }

    fn builtin_prefetch(&mut self) -> Result<Expression, ParseError> {
        let builtin = self
            .current_token()
            .expect("caller checked builtin")
            .clone();
        self.position += 1;
        self.expect_punctuator(
            Punctuator::LeftParen,
            "expected `(` after `__builtin_prefetch`",
        )?;
        let mut arguments = Vec::new();
        while !self.check_punctuator(Punctuator::RightParen) {
            arguments.push(self.assignment_expression()?);
            if self.consume_punctuator(Punctuator::Comma).is_none() {
                break;
            }
            if self.check_punctuator(Punctuator::RightParen) {
                return Err(self.error_current(
                    "trailing `,` is not allowed in `__builtin_prefetch` arguments",
                ));
            }
        }
        if !(1..=3).contains(&arguments.len()) {
            return Err(
                self.error_current("`__builtin_prefetch` requires between one and three arguments")
            );
        }
        let right = self.expect_punctuator(
            Punctuator::RightParen,
            "expected `)` after `__builtin_prefetch` arguments",
        )?;
        Ok(Expression {
            kind: ExpressionKind::BuiltinPrefetch { arguments },
            span: span_through(builtin.span, right.span),
        })
    }

    fn builtin_sync_operation(
        &mut self,
        operation: SyncBuiltinOperation,
    ) -> Result<Expression, ParseError> {
        let builtin = self
            .current_token()
            .expect("caller checked builtin")
            .clone();
        self.position += 1;
        self.expect_punctuator(
            Punctuator::LeftParen,
            &format!("expected `(` after `{}`", operation.spelling()),
        )?;
        let mut arguments = Vec::new();
        while !self.check_punctuator(Punctuator::RightParen) {
            let protected = arguments.len() >= operation.fixed_arity();
            arguments.push(self.sync_builtin_argument(protected)?);
            if self.consume_punctuator(Punctuator::Comma).is_none() {
                break;
            }
            if self.check_punctuator(Punctuator::RightParen) {
                return Err(self.error_current(&format!(
                    "trailing `,` is not allowed in `{}` arguments",
                    operation.spelling()
                )));
            }
        }
        if arguments.len() < operation.fixed_arity() {
            return Err(self.error_current(&format!(
                "`{}` requires at least {} arguments",
                operation.spelling(),
                operation.fixed_arity()
            )));
        }
        let right = self.expect_punctuator(
            Punctuator::RightParen,
            &format!("expected `)` after `{}` arguments", operation.spelling()),
        )?;
        Ok(Expression {
            kind: ExpressionKind::BuiltinSyncOperation {
                operation,
                arguments,
            },
            span: span_through(builtin.span, right.span),
        })
    }

    fn sync_builtin_argument(&mut self, protected: bool) -> Result<Expression, ParseError> {
        if protected
            && self
                .current_token()
                .is_some_and(|token| token.spelling == "__sync_synchronize")
            && (self.peek_punctuator(1, Punctuator::Comma)
                || self.peek_punctuator(1, Punctuator::RightParen))
        {
            let token = self.current_token().expect("checked above").clone();
            self.position += 1;
            return Ok(Expression {
                kind: ExpressionKind::Identifier(Identifier {
                    name: token.spelling,
                    span: token.span,
                }),
                span: token.span,
            });
        }
        self.assignment_expression()
    }

    fn current_binary_operator(&self) -> Option<(u8, BinaryOperator)> {
        let punctuator = match self.current_token()?.kind {
            TokenKind::Punctuator(punctuator) => punctuator,
            _ => return None,
        };
        Some(match punctuator {
            Punctuator::PipePipe => (1, BinaryOperator::LogicalOr),
            Punctuator::AmpAmp => (2, BinaryOperator::LogicalAnd),
            Punctuator::Pipe => (3, BinaryOperator::BitwiseOr),
            Punctuator::Caret => (4, BinaryOperator::BitwiseXor),
            Punctuator::Amp => (5, BinaryOperator::BitwiseAnd),
            Punctuator::EqualEqual => (6, BinaryOperator::Equal),
            Punctuator::BangEqual => (6, BinaryOperator::NotEqual),
            Punctuator::Less => (7, BinaryOperator::Less),
            Punctuator::LessEqual => (7, BinaryOperator::LessEqual),
            Punctuator::Greater => (7, BinaryOperator::Greater),
            Punctuator::GreaterEqual => (7, BinaryOperator::GreaterEqual),
            Punctuator::LeftShift => (8, BinaryOperator::LeftShift),
            Punctuator::RightShift => (8, BinaryOperator::RightShift),
            Punctuator::Plus => (9, BinaryOperator::Add),
            Punctuator::Minus => (9, BinaryOperator::Subtract),
            Punctuator::Star => (10, BinaryOperator::Multiply),
            Punctuator::Slash => (10, BinaryOperator::Divide),
            Punctuator::Percent => (10, BinaryOperator::Remainder),
            _ => return None,
        })
    }

    fn consume_assignment_operator(&mut self) -> Option<AssignmentOperator> {
        let punctuator = match self.current_token()?.kind {
            TokenKind::Punctuator(punctuator) => punctuator,
            _ => return None,
        };
        let operator = match punctuator {
            Punctuator::Assign => AssignmentOperator::Assign,
            Punctuator::StarAssign => AssignmentOperator::Multiply,
            Punctuator::SlashAssign => AssignmentOperator::Divide,
            Punctuator::PercentAssign => AssignmentOperator::Remainder,
            Punctuator::PlusAssign => AssignmentOperator::Add,
            Punctuator::MinusAssign => AssignmentOperator::Subtract,
            Punctuator::LeftShiftAssign => AssignmentOperator::LeftShift,
            Punctuator::RightShiftAssign => AssignmentOperator::RightShift,
            Punctuator::AmpAssign => AssignmentOperator::BitwiseAnd,
            Punctuator::CaretAssign => AssignmentOperator::BitwiseXor,
            Punctuator::PipeAssign => AssignmentOperator::BitwiseOr,
            _ => return None,
        };
        self.position += 1;
        Some(operator)
    }

    fn consume_type_qualifier(&mut self) -> Option<TypeQualifier> {
        let qualifier = if self.check_keyword(Keyword::Const) {
            TypeQualifier::Const
        } else if self.check_keyword(Keyword::Restrict) {
            TypeQualifier::Restrict
        } else if self.check_keyword(Keyword::Volatile) {
            TypeQualifier::Volatile
        } else if self.check_keyword(Keyword::Atomic) {
            TypeQualifier::Atomic
        } else {
            return None;
        };
        self.position += 1;
        Some(qualifier)
    }

    fn bind_declarator(&mut self, declarator: &Declarator, class: NameClass) {
        if let Some(identifier) = declarator.identifier() {
            self.names
                .bind(identifier.name.clone(), class, identifier.span);
        }
    }

    fn starts_declaration(&self) -> bool {
        if self.current_identifier_is_typedef()
            || self.current_is_plain_gnu("typeof")
            || self
                .current_token()
                .is_some_and(|token| token.spelling == "__builtin_va_list")
        {
            return true;
        }
        matches!(
            self.current_token().map(|token| &token.kind),
            Some(TokenKind::Keyword(
                Keyword::Auto
                    | Keyword::Char
                    | Keyword::Const
                    | Keyword::Double
                    | Keyword::Enum
                    | Keyword::Extern
                    | Keyword::Float
                    | Keyword::Inline
                    | Keyword::Int
                    | Keyword::Long
                    | Keyword::Noreturn
                    | Keyword::Register
                    | Keyword::Restrict
                    | Keyword::Short
                    | Keyword::Signed
                    | Keyword::Static
                    | Keyword::Struct
                    | Keyword::ThreadLocal
                    | Keyword::GnuThreadLocal
                    | Keyword::Typedef
                    | Keyword::Union
                    | Keyword::Unsigned
                    | Keyword::Void
                    | Keyword::Volatile
                    | Keyword::Alignas
                    | Keyword::Atomic
                    | Keyword::Bool
                    | Keyword::Complex
                    | Keyword::Imaginary
                    | Keyword::Attribute
                    | Keyword::Extension
                    | Keyword::Typeof
            ))
        )
    }

    fn starts_old_style_parameter_declaration(&self) -> bool {
        self.current_identifier_is_typedef()
            || matches!(
                self.current_token().map(|token| &token.kind),
                Some(TokenKind::Keyword(
                    Keyword::Auto
                        | Keyword::Char
                        | Keyword::Const
                        | Keyword::Double
                        | Keyword::Extern
                        | Keyword::Float
                        | Keyword::Int
                        | Keyword::Long
                        | Keyword::Register
                        | Keyword::Restrict
                        | Keyword::Short
                        | Keyword::Signed
                        | Keyword::Static
                        | Keyword::Struct
                        | Keyword::Union
                        | Keyword::Unsigned
                        | Keyword::Void
                        | Keyword::Volatile
                        | Keyword::Atomic
                        | Keyword::Bool
                        | Keyword::Complex
                        | Keyword::Imaginary
                        | Keyword::Typeof
                ))
            )
    }

    fn looks_like_type_name(&self) -> bool {
        self.looks_like_type_name_at(0)
    }

    fn looks_like_type_name_at(&self, offset: usize) -> bool {
        let Some(token) = self.token_at(self.position + offset) else {
            return false;
        };
        if token.kind == TokenKind::Identifier
            && self.names.lookup(&token.spelling) == Some(NameClass::TypedefName)
        {
            return true;
        }
        if token.kind == TokenKind::Identifier
            && token.spelling == "typeof"
            && self.language_mode == LanguageMode::Gnu11
        {
            return true;
        }
        if token.kind == TokenKind::Identifier && token.spelling == "__builtin_va_list" {
            return true;
        }
        matches!(
            token.kind,
            TokenKind::Keyword(
                Keyword::Char
                    | Keyword::Const
                    | Keyword::Double
                    | Keyword::Enum
                    | Keyword::Float
                    | Keyword::Int
                    | Keyword::Long
                    | Keyword::Restrict
                    | Keyword::Short
                    | Keyword::Signed
                    | Keyword::Struct
                    | Keyword::Union
                    | Keyword::Unsigned
                    | Keyword::Void
                    | Keyword::Volatile
                    | Keyword::Atomic
                    | Keyword::Bool
                    | Keyword::Complex
                    | Keyword::Imaginary
                    | Keyword::Typeof
                    | Keyword::Attribute
            )
        )
    }

    fn starts_declarator(&self) -> bool {
        self.check_identifier()
            || self.check_punctuator(Punctuator::Star)
            || self.check_punctuator(Punctuator::LeftParen)
    }

    fn starts_abstract_declarator(&self) -> bool {
        self.check_punctuator(Punctuator::Star)
            || self.check_punctuator(Punctuator::LeftParen)
            || self.check_punctuator(Punctuator::LeftBracket)
    }

    fn current_identifier_is_typedef(&self) -> bool {
        self.current_token().is_some_and(|token| {
            token.kind == TokenKind::Identifier
                && self.names.lookup(&token.spelling) == Some(NameClass::TypedefName)
        })
    }

    fn current_is_plain_gnu(&self, spelling: &str) -> bool {
        self.language_mode == LanguageMode::Gnu11
            && self.current_token().is_some_and(|token| {
                token.kind == TokenKind::Identifier && token.spelling == spelling
            })
    }

    fn check_asm_keyword(&self) -> bool {
        self.check_keyword(Keyword::Asm) || self.current_is_plain_gnu("asm")
    }

    fn consume_asm_keyword(&mut self) -> Option<Token> {
        if self.check_keyword(Keyword::Asm) || self.current_is_plain_gnu("asm") {
            let token = self.current_token()?.clone();
            self.position += 1;
            Some(token)
        } else {
            None
        }
    }

    fn identifier(&mut self) -> Result<Identifier, ParseError> {
        let token = self
            .current_token()
            .ok_or_else(|| self.error_current("expected an identifier"))?
            .clone();
        if token.kind != TokenKind::Identifier {
            return Err(self.error_at(token.span, "expected an identifier"));
        }
        self.position += 1;
        Ok(Identifier {
            name: token.spelling,
            span: token.span,
        })
    }

    fn check_identifier(&self) -> bool {
        self.current_token()
            .is_some_and(|token| token.kind == TokenKind::Identifier)
    }

    fn consume_keyword(&mut self, keyword: Keyword) -> Option<Token> {
        if !self.check_keyword(keyword) {
            return None;
        }
        let token = self.current_token()?.clone();
        self.position += 1;
        Some(token)
    }

    fn expect_keyword(&mut self, keyword: Keyword, message: &str) -> Result<Token, ParseError> {
        self.consume_keyword(keyword)
            .ok_or_else(|| self.error_current(message))
    }

    fn check_keyword(&self, keyword: Keyword) -> bool {
        self.current_token()
            .is_some_and(|token| token.kind == TokenKind::Keyword(keyword))
    }

    fn consume_punctuator(&mut self, punctuator: Punctuator) -> Option<Token> {
        if !self.check_punctuator(punctuator) {
            return None;
        }
        let token = self.current_token()?.clone();
        self.position += 1;
        Some(token)
    }

    fn expect_punctuator(
        &mut self,
        punctuator: Punctuator,
        message: &str,
    ) -> Result<Token, ParseError> {
        self.consume_punctuator(punctuator)
            .ok_or_else(|| self.error_current(message))
    }

    fn check_punctuator(&self, punctuator: Punctuator) -> bool {
        self.current_token()
            .is_some_and(|token| token.kind == TokenKind::Punctuator(punctuator))
    }

    fn peek_punctuator(&self, offset: usize, punctuator: Punctuator) -> bool {
        self.token_at(self.position + offset)
            .is_some_and(|token| token.kind == TokenKind::Punctuator(punctuator))
    }

    fn consume_pragma(&mut self) -> Option<PragmaEvent> {
        let ParseItem::Pragma(pragma) = self.current_item()? else {
            return None;
        };
        let pragma = (*pragma).clone();
        self.position += 1;
        Some(pragma)
    }

    fn current_item(&self) -> Option<&ParseItem<'_>> {
        self.items.get(self.position)
    }

    fn current_token(&self) -> Option<&Token> {
        match self.current_item()? {
            ParseItem::Token(token) => Some(token),
            ParseItem::Pragma(_) => None,
        }
    }

    fn token_at(&self, position: usize) -> Option<&Token> {
        match self.items.get(position)? {
            ParseItem::Token(token) => Some(token),
            ParseItem::Pragma(_) => None,
        }
    }

    fn current_span(&self) -> Result<Span, ParseError> {
        self.current_item()
            .map(ParseItem::span)
            .ok_or_else(|| self.error_eof("unexpected end of input"))
    }

    fn previous_span(&self) -> Span {
        self.items[self.position - 1].span()
    }

    fn nested<T>(
        &mut self,
        parser: impl FnOnce(&mut Self) -> Result<T, ParseError>,
    ) -> Result<T, ParseError> {
        if self.recursion_depth >= MAX_RECURSION_DEPTH {
            return Err(self.error_current("syntax is too deeply nested"));
        }
        self.recursion_depth += 1;
        let result = parser(self);
        self.recursion_depth -= 1;
        result
    }

    fn error_current(&self, message: &str) -> ParseError {
        self.current_item().map_or_else(
            || self.error_eof(message),
            |item| self.error_at(item.span(), message),
        )
    }

    fn error_eof(&self, message: &str) -> ParseError {
        let last = self
            .items
            .last()
            .expect("an empty translation unit does not produce parse errors")
            .span();
        ParseError {
            code: "CCC1020",
            span: Span::with_origin(last.file, last.end, last.end, last.origin),
            message: message.to_owned(),
        }
    }

    fn error_at(&self, span: Span, message: &str) -> ParseError {
        ParseError {
            code: "CCC1020",
            span,
            message: message.to_owned(),
        }
    }
}

impl ParseItem<'_> {
    fn span(&self) -> Span {
        match self {
            Self::Token(token) => token.span,
            Self::Pragma(pragma) => pragma_span(pragma),
        }
    }
}

fn pragma_span(pragma: &PragmaEvent) -> Span {
    match pragma {
        PragmaEvent::Once { span }
        | PragmaEvent::SystemHeader { span }
        | PragmaEvent::Diagnostic { span, .. }
        | PragmaEvent::Pack { span, .. }
        | PragmaEvent::Unknown { span, .. } => *span,
    }
}
