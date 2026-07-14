use std::collections::{HashMap, HashSet};

use ccc_diag::Diagnostic;
use ccc_pp::{CharacterConstantPrefix, FloatingConstantSuffix, PragmaEvent, StringLiteralPrefix};
use ccc_session::Span;
use ccc_syntax::frontend as syntax;
use ccc_target::{CapabilityKind, CapabilityState, EffectiveCompilationConfig, PackingPolicy};
use ccc_types::{
    ArrayLength, ArrayType, BuiltinType, Field, FunctionParameters, FunctionType, LayoutShape,
    QualifiedType, RecordKind, TypeId, TypeKind, TypeQualifiers,
};

use super::model::*;
use super::scopes::{
    LabelScope, OrdinaryBindingConflict, OrdinarySymbol, ScopeStack, TagCategory, TagSymbol,
};

type AnalysisResult<T> = Result<T, ()>;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum StringEncodingKey {
    Ordinary,
    Utf8,
    Wide,
    Utf16,
    Utf32,
}

impl From<StringLiteralPrefix> for StringEncodingKey {
    fn from(prefix: StringLiteralPrefix) -> Self {
        match prefix {
            StringLiteralPrefix::None => Self::Ordinary,
            StringLiteralPrefix::Utf8 => Self::Utf8,
            StringLiteralPrefix::Wide => Self::Wide,
            StringLiteralPrefix::Utf16 => Self::Utf16,
            StringLiteralPrefix::Utf32 => Self::Utf32,
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct StringPoolKey {
    element: TypeId,
    encoding: StringEncodingKey,
    code_units: Vec<u32>,
    alignment: u64,
    mutable: bool,
}

#[derive(Clone, Debug)]
struct ResolvedParameter {
    name: Option<String>,
    ty: QualifiedType,
    span: Span,
}

#[derive(Clone, Debug)]
struct ResolvedDeclarator {
    name: Option<String>,
    name_span: Span,
    ty: QualifiedType,
    parameters: Vec<ResolvedParameter>,
    attributes: Vec<FullTypedAttribute>,
}

#[derive(Clone, Debug)]
struct DeclarationInfo {
    base: QualifiedType,
    storage: Option<syntax::StorageClass>,
    properties: FunctionProperties,
    attributes: Vec<FullTypedAttribute>,
}

#[derive(Default)]
struct SwitchState {
    cases: HashMap<i128, Span>,
    default: Option<Span>,
}

struct FunctionState {
    id: FullFunctionId,
    name: String,
    return_ty: QualifiedType,
    next_local: u32,
    labels: LabelScope,
    loop_depth: usize,
    switches: Vec<SwitchState>,
}

#[derive(Clone, Debug)]
struct PackingFrame {
    policy: PackingPolicy,
    label: Option<String>,
}

#[derive(Clone, Debug)]
struct PackingState {
    current: PackingPolicy,
    stack: Vec<PackingFrame>,
}

impl Default for PackingState {
    fn default() -> Self {
        Self {
            current: PackingPolicy::NATIVE,
            stack: Vec::new(),
        }
    }
}

pub fn analyze_frontend(
    unit: &syntax::TranslationUnit,
    config: &EffectiveCompilationConfig,
) -> Result<FullTypedTranslationUnit, Vec<Diagnostic>> {
    let mut analyzer = Analyzer::new(config);
    analyzer.analyze_translation_unit(unit);
    analyzer.complete_tentative_arrays();
    if analyzer.diagnostics.is_empty() {
        Ok(analyzer.finish())
    } else {
        Err(analyzer.diagnostics)
    }
}

struct Analyzer<'a> {
    config: &'a EffectiveCompilationConfig,
    types: ccc_types::TypeStore,
    external_items: Vec<FullTypedExternalItem>,
    globals: Vec<FullTypedGlobal>,
    functions: Vec<FullTypedFunction>,
    typedefs: Vec<FullTypedTypedef>,
    strings: Vec<FullTypedString>,
    string_pool: HashMap<StringPoolKey, StringId>,
    scopes: ScopeStack,
    function: Option<FunctionState>,
    packing: PackingState,
    diagnostics: Vec<Diagnostic>,
}

impl<'a> Analyzer<'a> {
    fn new(config: &'a EffectiveCompilationConfig) -> Self {
        Self {
            config,
            types: ccc_types::TypeStore::default(),
            external_items: Vec::new(),
            globals: Vec::new(),
            functions: Vec::new(),
            typedefs: Vec::new(),
            strings: Vec::new(),
            string_pool: HashMap::new(),
            scopes: ScopeStack::new(),
            function: None,
            packing: PackingState::default(),
            diagnostics: Vec::new(),
        }
    }

    fn finish(self) -> FullTypedTranslationUnit {
        FullTypedTranslationUnit {
            types: self.types,
            external_items: self.external_items,
            globals: self.globals,
            functions: self.functions,
            typedefs: self.typedefs,
            strings: self.strings,
        }
    }

    fn analyze_translation_unit(&mut self, unit: &syntax::TranslationUnit) {
        for item in &unit.items {
            match item {
                syntax::ExternalItem::Pragma(pragma) => {
                    if self.handle_pragma(pragma).is_ok() {
                        self.external_items
                            .push(FullTypedExternalItem::Pragma(pragma.clone()));
                    }
                }
                syntax::ExternalItem::Declaration(declaration) => {
                    let _ = self.analyze_file_declaration(declaration);
                }
                syntax::ExternalItem::FunctionDefinition(definition) => {
                    let _ = self.analyze_function_definition(definition);
                }
                syntax::ExternalItem::StaticAssert(assertion) => {
                    if let Ok(value) = self.analyze_static_assert(assertion) {
                        self.external_items
                            .push(FullTypedExternalItem::StaticAssert {
                                value,
                                span: assertion.span,
                            });
                    }
                }
            }
        }
    }

    fn complete_tentative_arrays(&mut self) {
        for global in &mut self.globals {
            if !global.tentative {
                continue;
            }
            let Some(TypeKind::Array(ArrayType {
                element,
                length: ArrayLength::Incomplete,
            })) = self.types.try_kind(global.ty.ty).cloned()
            else {
                continue;
            };
            global.ty.ty = self.types.array(ArrayType {
                element,
                length: ArrayLength::Constant(1),
            });
        }
    }

    fn analyze_file_declaration(
        &mut self,
        declaration: &syntax::Declaration,
    ) -> AnalysisResult<()> {
        let info = self.resolve_declaration_specifiers(&declaration.specifiers)?;
        if declaration.declarators.is_empty() {
            self.external_items
                .push(FullTypedExternalItem::TypeDeclaration {
                    ty: info.base.ty,
                    span: declaration.span,
                });
            return Ok(());
        }

        for init in &declaration.declarators {
            let resolved = self.resolve_declarator(info.base, &init.declarator)?;
            let Some(name) = resolved.name.clone() else {
                return self.fail(
                    "CCC2201",
                    init.span,
                    "a file-scope declarator must declare an identifier",
                );
            };
            let mut attributes = info.attributes.clone();
            attributes.extend(resolved.attributes.clone());
            attributes.extend(self.validate_attributes(&init.attributes)?);
            if info.storage == Some(syntax::StorageClass::Typedef) {
                if init.initializer.is_some() || init.asm_label.is_some() {
                    return self.fail(
                        "CCC2202",
                        init.span,
                        "a typedef cannot have an initializer or assembly label",
                    );
                }
                self.declare_typedef(name, resolved.ty, attributes, init.span)?;
            } else if self.types.function_signature(resolved.ty.ty).is_some() {
                if init.initializer.is_some() {
                    return self.fail(
                        "CCC2203",
                        init.span,
                        "a function declaration cannot have an initializer",
                    );
                }
                let asm_label = self.resolve_asm_label(init.asm_label.as_ref())?;
                let id = self.declare_function(
                    name,
                    resolved.ty.ty,
                    info.storage,
                    info.properties,
                    attributes,
                    asm_label,
                    init.span,
                )?;
                self.external_items
                    .push(FullTypedExternalItem::Function(id));
            } else {
                let asm_label = self.resolve_asm_label(init.asm_label.as_ref())?;
                let id = self.declare_global(
                    name,
                    resolved.ty,
                    info.storage,
                    attributes,
                    asm_label,
                    init.initializer.as_ref(),
                    init.span,
                )?;
                self.external_items.push(FullTypedExternalItem::Global(id));
            }
        }
        Ok(())
    }

    fn analyze_function_definition(
        &mut self,
        definition: &syntax::FunctionDefinition,
    ) -> AnalysisResult<()> {
        let info = self.resolve_declaration_specifiers(&definition.specifiers)?;
        if info.storage == Some(syntax::StorageClass::Typedef) {
            return self.fail(
                "CCC2204",
                definition.span,
                "a function definition cannot be a typedef",
            );
        }
        if !definition.declarations.is_empty() {
            return self.fail(
                "CCC2205",
                definition.span,
                "old-style function parameter declarations are not semantically supported",
            );
        }
        let resolved = self.resolve_declarator(info.base, &definition.declarator)?;
        let Some(name) = resolved.name.clone() else {
            return self.fail(
                "CCC2206",
                definition.span,
                "a function definition requires a name",
            );
        };
        let Some(signature) = self.types.function_signature(resolved.ty.ty) else {
            return self.fail(
                "CCC2207",
                definition.declarator.span,
                "a function definition declarator must have function type",
            );
        };
        let mut attributes = info.attributes;
        attributes.extend(resolved.attributes);
        let id = self.declare_function(
            name.clone(),
            resolved.ty.ty,
            info.storage,
            info.properties,
            attributes,
            None,
            definition.span,
        )?;
        if self.functions[id.0 as usize].body.is_some() {
            return self.fail(
                "CCC2208",
                definition.span,
                format!("function `{name}` is defined more than once"),
            );
        }

        let return_ty = signature.result;
        self.function = Some(FunctionState {
            id,
            name: name.clone(),
            return_ty,
            next_local: 0,
            labels: LabelScope::default(),
            loop_depth: 0,
            switches: Vec::new(),
        });
        self.collect_labels(&definition.body);
        self.push_scope();

        let mut typed_parameters = Vec::new();
        for parameter in &resolved.parameters {
            let Some(parameter_name) = parameter.name.clone() else {
                self.pop_scope();
                self.function = None;
                return self.fail(
                    "CCC2209",
                    parameter.span,
                    "a parameter in a function definition requires a name",
                );
            };
            let local = self.fresh_local();
            self.bind_current(
                parameter_name.clone(),
                OrdinarySymbol::Local(local, parameter.ty),
                parameter.span,
            )?;
            typed_parameters.push(FullTypedParameter {
                local,
                name: parameter_name,
                ty: parameter.ty,
                span: parameter.span,
            });
        }

        let body = self.analyze_statement(&definition.body);
        self.validate_labels();
        self.pop_scope();
        self.function = None;
        let body = body?;
        let function = &mut self.functions[id.0 as usize];
        function.parameters = typed_parameters;
        function.body = Some(body);
        function.span = definition.span;
        self.external_items
            .push(FullTypedExternalItem::Function(id));
        Ok(())
    }

    fn resolve_declaration_specifiers(
        &mut self,
        specifiers: &syntax::DeclarationSpecifiers,
    ) -> AnalysisResult<DeclarationInfo> {
        let mut storage = None;
        let mut properties = FunctionProperties::default();
        let mut qualifiers = TypeQualifiers::NONE;
        let mut type_specifiers = Vec::new();
        let mut attributes = Vec::new();

        for item in &specifiers.items {
            match item {
                syntax::DeclarationSpecifier::StorageClass(candidate) => {
                    if *candidate == syntax::StorageClass::GnuThreadLocal {
                        return self.fail(
                            "CCC2374",
                            specifiers.span,
                            "`__thread` is parse-only until GNU thread-local storage semantics are enabled",
                        );
                    }
                    if storage.replace(*candidate).is_some() {
                        return self.fail(
                            "CCC2210",
                            specifiers.span,
                            "a declaration has more than one storage-class specifier",
                        );
                    }
                }
                syntax::DeclarationSpecifier::Type(specifier) => type_specifiers.push(specifier),
                syntax::DeclarationSpecifier::Qualifier(qualifier) => {
                    qualifiers |= qualifier_bits(*qualifier)
                }
                syntax::DeclarationSpecifier::Function(specifier) => match specifier {
                    syntax::FunctionSpecifier::Inline => properties.inline = true,
                    syntax::FunctionSpecifier::NoReturn => properties.no_return = true,
                },
                syntax::DeclarationSpecifier::Alignment(_) => {
                    return self.fail(
                        "CCC2211",
                        specifiers.span,
                        "alignment specifiers are parsed but are not semantically supported",
                    );
                }
                syntax::DeclarationSpecifier::Attribute(attribute) => {
                    attributes.push(self.validate_attribute(attribute)?);
                }
            }
        }
        if type_specifiers.is_empty() {
            return self.fail(
                "CCC2212",
                specifiers.span,
                "a declaration requires a type specifier",
            );
        }
        let mut base = self.resolve_type_specifiers(&type_specifiers, specifiers.span)?;
        base.qualifiers |= qualifiers;
        Ok(DeclarationInfo {
            base,
            storage,
            properties,
            attributes,
        })
    }

    fn resolve_type_specifiers(
        &mut self,
        specifiers: &[&syntax::TypeSpecifier],
        span: Span,
    ) -> AnalysisResult<QualifiedType> {
        if specifiers.len() == 1 {
            match specifiers[0] {
                syntax::TypeSpecifier::Struct(record) => {
                    return self
                        .resolve_record_specifier(RecordKind::Struct, record)
                        .map(QualifiedType::unqualified);
                }
                syntax::TypeSpecifier::Union(record) => {
                    return self
                        .resolve_record_specifier(RecordKind::Union, record)
                        .map(QualifiedType::unqualified);
                }
                syntax::TypeSpecifier::Enum(enumeration) => {
                    return self
                        .resolve_enum_specifier(enumeration)
                        .map(QualifiedType::unqualified);
                }
                syntax::TypeSpecifier::TypedefName(identifier) => {
                    return match self.lookup_ordinary(&identifier.name) {
                        Some(OrdinarySymbol::Typedef(_, ty)) => Ok(*ty),
                        _ => self.fail(
                            "CCC2213",
                            identifier.span,
                            format!("`{}` is not a typedef name", identifier.name),
                        ),
                    };
                }
                syntax::TypeSpecifier::Atomic(type_name) => {
                    let mut ty = self.resolve_type_name(type_name)?;
                    ty.qualifiers |= TypeQualifiers::ATOMIC;
                    return Ok(ty);
                }
                syntax::TypeSpecifier::Typeof(_) => {
                    return self.fail(
                        "CCC2214",
                        span,
                        "`typeof` is parse-only and has no supported semantic meaning",
                    );
                }
                _ => {}
            }
        }
        if specifiers.iter().any(|specifier| {
            matches!(
                specifier,
                syntax::TypeSpecifier::Struct(_)
                    | syntax::TypeSpecifier::Union(_)
                    | syntax::TypeSpecifier::Enum(_)
                    | syntax::TypeSpecifier::TypedefName(_)
                    | syntax::TypeSpecifier::Atomic(_)
                    | syntax::TypeSpecifier::Typeof(_)
            )
        }) {
            return self.fail(
                "CCC2215",
                span,
                "this combination of type specifiers is invalid",
            );
        }

        let count = |needle: fn(&syntax::TypeSpecifier) -> bool| {
            specifiers.iter().filter(|item| needle(item)).count()
        };
        let void = count(|item| matches!(item, syntax::TypeSpecifier::Void));
        let char_count = count(|item| matches!(item, syntax::TypeSpecifier::Char));
        let short = count(|item| matches!(item, syntax::TypeSpecifier::Short));
        let int = count(|item| matches!(item, syntax::TypeSpecifier::Int));
        let long = count(|item| matches!(item, syntax::TypeSpecifier::Long));
        let float = count(|item| matches!(item, syntax::TypeSpecifier::Float));
        let double = count(|item| matches!(item, syntax::TypeSpecifier::Double));
        let signed = count(|item| matches!(item, syntax::TypeSpecifier::Signed));
        let unsigned = count(|item| matches!(item, syntax::TypeSpecifier::Unsigned));
        let boolean = count(|item| matches!(item, syntax::TypeSpecifier::Bool));
        let complex = count(|item| matches!(item, syntax::TypeSpecifier::Complex));
        let imaginary = count(|item| matches!(item, syntax::TypeSpecifier::Imaginary));

        if complex != 0 || imaginary != 0 {
            return self.fail(
                "CCC2216",
                span,
                "complex and imaginary arithmetic are not supported",
            );
        }
        if signed != 0 && unsigned != 0
            || void > 1
            || char_count > 1
            || short > 1
            || int > 1
            || long > 2
            || float > 1
            || double > 1
            || signed > 1
            || unsigned > 1
            || boolean > 1
        {
            return self.fail("CCC2217", span, "invalid repetition of type specifiers");
        }
        let total = specifiers.len();
        let builtin = if void == 1 && total == 1 {
            BuiltinType::Void
        } else if boolean == 1 && total == 1 {
            BuiltinType::Bool
        } else if char_count == 1 && total == char_count + signed + unsigned {
            if unsigned == 1 {
                BuiltinType::UnsignedChar
            } else if signed == 1 {
                BuiltinType::SignedChar
            } else {
                BuiltinType::Char
            }
        } else if short == 1 && total == short + int + signed + unsigned {
            if unsigned == 1 {
                BuiltinType::UnsignedShort
            } else {
                BuiltinType::Short
            }
        } else if long == 0
            && float == 0
            && double == 0
            && void == 0
            && char_count == 0
            && short == 0
            && boolean == 0
            && total == int + signed + unsigned
        {
            if unsigned == 1 {
                BuiltinType::UnsignedInt
            } else {
                BuiltinType::Int
            }
        } else if long >= 1 && float == 0 && double == 0 && total == long + int + signed + unsigned
        {
            match (long, unsigned) {
                (1, 0) => BuiltinType::Long,
                (1, 1) => BuiltinType::UnsignedLong,
                (2, 0) => BuiltinType::LongLong,
                (2, 1) => BuiltinType::UnsignedLongLong,
                _ => unreachable!("the counts were validated"),
            }
        } else if float == 1 && total == 1 {
            BuiltinType::Float
        } else if double == 1 && long == 0 && total == 1 {
            BuiltinType::Double
        } else if double == 1 && long == 1 && total == 2 {
            BuiltinType::LongDouble
        } else {
            return self.fail("CCC2218", span, "invalid combination of type specifiers");
        };
        Ok(QualifiedType::unqualified(self.types.builtin(builtin)))
    }

    fn resolve_type_name(&mut self, type_name: &syntax::TypeName) -> AnalysisResult<QualifiedType> {
        let info = self.resolve_declaration_specifiers(&type_name.specifiers)?;
        if info.storage.is_some() || info.properties != FunctionProperties::default() {
            return self.fail(
                "CCC2219",
                type_name.span,
                "a type name cannot contain storage-class or function specifiers",
            );
        }
        match &type_name.declarator {
            Some(declarator) => {
                let resolved = self.resolve_declarator(info.base, declarator)?;
                if resolved.name.is_some() {
                    return self.fail(
                        "CCC2220",
                        type_name.span,
                        "a type name cannot declare an identifier",
                    );
                }
                Ok(resolved.ty)
            }
            None => Ok(info.base),
        }
    }

    fn resolve_declarator(
        &mut self,
        mut ty: QualifiedType,
        declarator: &syntax::Declarator,
    ) -> AnalysisResult<ResolvedDeclarator> {
        let mut attributes = self.validate_attributes(&declarator.attributes)?;
        for pointer in &declarator.pointers {
            attributes.extend(self.validate_attributes(&pointer.attributes)?);
            let pointer_ty = self.types.pointer(ty);
            ty = QualifiedType::new(pointer_ty, qualifiers(&pointer.qualifiers));
        }
        let mut parameters = Vec::new();
        let (name, name_span, ty) =
            self.resolve_direct_declarator(ty, &declarator.direct, &mut parameters)?;
        Ok(ResolvedDeclarator {
            name,
            name_span,
            ty,
            parameters,
            attributes,
        })
    }

    fn resolve_direct_declarator(
        &mut self,
        ty: QualifiedType,
        direct: &syntax::DirectDeclarator,
        parameters_out: &mut Vec<ResolvedParameter>,
    ) -> AnalysisResult<(Option<String>, Span, QualifiedType)> {
        match direct {
            syntax::DirectDeclarator::Identifier(identifier) => {
                Ok((Some(identifier.name.clone()), identifier.span, ty))
            }
            syntax::DirectDeclarator::Abstract(span) => Ok((None, *span, ty)),
            syntax::DirectDeclarator::Parenthesized(inner, _) => {
                let resolved = self.resolve_declarator(ty, inner)?;
                if parameters_out.is_empty() {
                    *parameters_out = resolved.parameters;
                }
                Ok((resolved.name, resolved.name_span, resolved.ty))
            }
            syntax::DirectDeclarator::Array {
                inner,
                qualifiers: _,
                size,
                span,
                ..
            } => {
                if matches!(self.types.try_kind(ty.ty), Some(TypeKind::Function(_))) {
                    return self.fail(
                        "CCC2221",
                        *span,
                        "an array element cannot have function type",
                    );
                }
                if ty.ty == TypeId::VOID {
                    return self.fail("CCC2222", *span, "an array element cannot have void type");
                }
                let length = match size {
                    syntax::ArraySize::Unspecified => ArrayLength::Incomplete,
                    syntax::ArraySize::Star => {
                        ArrayLength::Variable(self.types.fresh_variable_length())
                    }
                    syntax::ArraySize::Expression(expression) => {
                        match self.try_evaluate_integer_constant(expression)? {
                            Some(value) if value > 0 => ArrayLength::Constant(value as u64),
                            Some(_) => {
                                return self.fail(
                                    "CCC2223",
                                    expression.span,
                                    "an array bound must be greater than zero",
                                );
                            }
                            None => {
                                if self.function.is_none() {
                                    return self.fail(
                                        "CCC2223",
                                        expression.span,
                                        "a file-scope array bound must be constant",
                                    );
                                }
                                ArrayLength::Variable(self.types.fresh_variable_length())
                            }
                        }
                    }
                };
                // Bracket qualifiers belong to the adjusted parameter pointer,
                // not the array element. `resolve_parameter` applies them when
                // this declarator is used as a parameter.
                let element = ty;
                let array =
                    QualifiedType::unqualified(self.types.array(ArrayType { element, length }));
                self.resolve_direct_declarator(array, inner, parameters_out)
            }
            syntax::DirectDeclarator::Function {
                inner,
                parameters,
                has_parameter_type_list,
                variadic,
                old_style_names,
                span,
            } => {
                if !old_style_names.is_empty() {
                    return self.fail(
                        "CCC2224",
                        *span,
                        "old-style identifier-list function types are not semantically supported",
                    );
                }
                if matches!(
                    self.types.try_kind(ty.ty),
                    Some(TypeKind::Array(_) | TypeKind::Function(_))
                ) {
                    return self.fail(
                        "CCC2225",
                        *span,
                        "a function cannot return an array or function type",
                    );
                }
                let mut resolved_parameters = Vec::new();
                for parameter in parameters {
                    resolved_parameters.push(self.resolve_parameter(parameter)?);
                }
                if resolved_parameters.len() == 1
                    && resolved_parameters[0].name.is_none()
                    && resolved_parameters[0].ty.ty == TypeId::VOID
                    && resolved_parameters[0].ty.qualifiers.is_empty()
                {
                    resolved_parameters.clear();
                } else if resolved_parameters
                    .iter()
                    .any(|parameter| parameter.ty.ty == TypeId::VOID)
                {
                    return self.fail(
                        "CCC2226",
                        *span,
                        "`void` must be the only unnamed function parameter",
                    );
                }
                let parameter_types = resolved_parameters
                    .iter()
                    // C ignores top-level parameter qualifiers when forming
                    // the function type, while the parameter object retains
                    // them for accesses inside a definition.
                    .map(|parameter| QualifiedType::unqualified(parameter.ty.ty))
                    .collect::<Vec<_>>();
                let signature = if !has_parameter_type_list && resolved_parameters.is_empty() {
                    FunctionType::unspecified(ty)
                } else if *variadic {
                    FunctionType::variadic(ty, parameter_types)
                } else {
                    FunctionType::prototype(ty, parameter_types)
                };
                let function_ty = QualifiedType::unqualified(self.types.function_type(signature));
                if parameters_out.is_empty() {
                    *parameters_out = resolved_parameters;
                }
                self.resolve_direct_declarator(function_ty, inner, parameters_out)
            }
        }
    }

    fn resolve_parameter(
        &mut self,
        parameter: &syntax::ParameterDeclaration,
    ) -> AnalysisResult<ResolvedParameter> {
        let info = self.resolve_declaration_specifiers(&parameter.specifiers)?;
        if !matches!(info.storage, None | Some(syntax::StorageClass::Register)) {
            return self.fail(
                "CCC2227",
                parameter.span,
                "a parameter may only use the `register` storage class",
            );
        }
        let (name, mut ty, span) = if let Some(declarator) = &parameter.declarator {
            let resolved = self.resolve_declarator(info.base, declarator)?;
            (resolved.name, resolved.ty, resolved.name_span)
        } else {
            (None, info.base, parameter.span)
        };
        ty = match self.types.try_kind(ty.ty).cloned() {
            Some(TypeKind::Array(array)) => {
                let pointer = self.types.pointer(array.element);
                QualifiedType::new(pointer, parameter_array_qualifiers(&parameter.declarator))
            }
            Some(TypeKind::Function(_)) => QualifiedType::unqualified(self.types.pointer(ty)),
            _ => ty,
        };
        Ok(ResolvedParameter { name, ty, span })
    }

    fn resolve_record_specifier(
        &mut self,
        kind: RecordKind,
        specifier: &syntax::RecordSpecifier,
    ) -> AnalysisResult<TypeId> {
        let category = match kind {
            RecordKind::Struct => TagCategory::Struct,
            RecordKind::Union => TagCategory::Union,
        };
        let tag = specifier.tag.as_ref().map(|tag| tag.name.clone());
        let existing = tag.as_deref().and_then(|name| self.lookup_tag(name));
        let (record_id, ty) = if let Some(existing) = existing {
            if existing.category != category {
                return self.fail(
                    "CCC2228",
                    specifier.span,
                    format!(
                        "tag `{}` was previously declared with a different tag kind",
                        tag.as_deref().unwrap_or_default()
                    ),
                );
            }
            let Some(TypeKind::Record(id)) = self.types.try_kind(existing.ty).cloned() else {
                return self.fail("CCC2229", specifier.span, "invalid record tag binding");
            };
            (id, existing.ty)
        } else {
            if specifier.items.is_none() && tag.is_none() {
                return self.fail(
                    "CCC2230",
                    specifier.span,
                    "an anonymous record specifier requires a definition",
                );
            }
            let (id, ty) = self.types.declare_record(kind, tag.clone());
            if let Some(tag) = &tag {
                self.bind_tag_current(tag.clone(), TagSymbol { category, ty }, specifier.span)?;
            }
            (id, ty)
        };

        let record_attributes = self.validate_attributes(&specifier.attributes)?;
        let Some(items) = &specifier.items else {
            return Ok(ty);
        };
        if self
            .types
            .record(record_id)
            .is_some_and(|definition| definition.is_complete())
        {
            return self.fail(
                "CCC2231",
                specifier.span,
                "a record type is defined more than once",
            );
        }

        let applied_packing = if record_attributes
            .iter()
            .any(|attribute| attribute.name == "packed")
        {
            self.packing.current.combine(PackingPolicy::PACKED)
        } else {
            self.packing.current
        };
        let mut fields = Vec::new();
        let mut field_names = HashSet::new();
        for item in items {
            match item {
                syntax::RecordItem::Pragma(pragma) => self.handle_pragma(pragma)?,
                syntax::RecordItem::StaticAssert(assertion) => {
                    let _ = self.analyze_static_assert(assertion)?;
                }
                syntax::RecordItem::Declaration(declaration) => {
                    let info = self.resolve_declaration_specifiers(&declaration.specifiers)?;
                    if info.storage.is_some() || info.properties != FunctionProperties::default() {
                        return self.fail(
                            "CCC2232",
                            declaration.span,
                            "a record member cannot have storage-class or function specifiers",
                        );
                    }
                    if declaration.declarators.is_empty() {
                        if !matches!(self.types.try_kind(info.base.ty), Some(TypeKind::Record(_))) {
                            return self.fail(
                                "CCC2233",
                                declaration.span,
                                "an unnamed record member must have struct or union type",
                            );
                        }
                        fields.push(Field::anonymous(info.base));
                        continue;
                    }
                    for member in &declaration.declarators {
                        let _ = self.validate_attributes(&member.attributes)?;
                        let (name, field_ty) = if let Some(declarator) = &member.declarator {
                            let resolved = self.resolve_declarator(info.base, declarator)?;
                            (resolved.name, resolved.ty)
                        } else {
                            (None, info.base)
                        };
                        if let Some(name) = &name
                            && !field_names.insert(name.clone())
                        {
                            return self.fail(
                                "CCC2234",
                                member.span,
                                format!("record member `{name}` is declared more than once"),
                            );
                        }
                        if field_ty.ty == TypeId::VOID
                            || self.types.function_signature(field_ty.ty).is_some()
                        {
                            return self.fail(
                                "CCC2235",
                                member.span,
                                "a record member must have object type",
                            );
                        }
                        if matches!(
                            self.types.try_kind(field_ty.ty),
                            Some(TypeKind::Array(ArrayType {
                                length: ArrayLength::Incomplete,
                                ..
                            }))
                        ) {
                            return self.fail(
                                "CCC2370",
                                member.span,
                                "flexible array members are not supported",
                            );
                        }
                        let field = if let Some(width) = &member.bit_width {
                            if !self.types.is_integer(field_ty.ty) {
                                return self.fail(
                                    "CCC2236",
                                    width.span,
                                    "a bitfield must have integer type",
                                );
                            }
                            let value = self.evaluate_integer_constant(width)?;
                            let width = u32::try_from(value).map_err(|_| {
                                self.emit(
                                    "CCC2237",
                                    width.span,
                                    "a bitfield width must be a nonnegative 32-bit value",
                                )
                            })?;
                            Field::bitfield(name, field_ty, width)
                        } else {
                            Field::new(name, field_ty)
                        };
                        fields.push(field);
                    }
                }
            }
        }
        self.types
            .complete_record_with_packing(record_id, fields, applied_packing)
            .map_err(|error| {
                self.emit("CCC2238", specifier.span, error.to_string());
            })?;
        self.types.layout_of(ty, self.config).map_err(|error| {
            self.emit("CCC2239", specifier.span, error.to_string());
        })?;
        Ok(ty)
    }

    fn resolve_enum_specifier(
        &mut self,
        specifier: &syntax::EnumSpecifier,
    ) -> AnalysisResult<TypeId> {
        let tag = specifier.tag.as_ref().map(|tag| tag.name.clone());
        let existing = tag.as_deref().and_then(|name| self.lookup_tag(name));
        let (enum_id, ty) = if let Some(existing) = existing {
            if existing.category != TagCategory::Enum {
                return self.fail(
                    "CCC2240",
                    specifier.span,
                    format!(
                        "tag `{}` was previously declared with a different tag kind",
                        tag.as_deref().unwrap_or_default()
                    ),
                );
            }
            let Some(TypeKind::Enum(id)) = self.types.try_kind(existing.ty).cloned() else {
                return self.fail("CCC2241", specifier.span, "invalid enum tag binding");
            };
            (id, existing.ty)
        } else {
            if specifier.enumerators.is_none() && tag.is_none() {
                return self.fail(
                    "CCC2242",
                    specifier.span,
                    "an anonymous enum specifier requires a definition",
                );
            }
            let (id, ty) = self.types.declare_enum(tag.clone());
            if let Some(tag) = &tag {
                self.bind_tag_current(
                    tag.clone(),
                    TagSymbol {
                        category: TagCategory::Enum,
                        ty,
                    },
                    specifier.span,
                )?;
            }
            (id, ty)
        };
        let _ = self.validate_attributes(&specifier.attributes)?;
        let Some(enumerators) = &specifier.enumerators else {
            return Ok(ty);
        };
        if self
            .types
            .enumeration(enum_id)
            .is_some_and(|definition| definition.is_complete())
        {
            return self.fail(
                "CCC2243",
                specifier.span,
                "an enum type is defined more than once",
            );
        }
        let mut next_value = 0_i128;
        let mut typed = Vec::new();
        for enumerator in enumerators {
            let _ = self.validate_attributes(&enumerator.attributes)?;
            let value = if let Some(expression) = &enumerator.value {
                self.evaluate_integer_constant(expression)?
            } else {
                next_value
            };
            next_value = value.checked_add(1).ok_or_else(|| {
                self.emit(
                    "CCC2244",
                    enumerator.span,
                    "enumerator value overflows the constant evaluator",
                )
            })?;
            self.bind_current(
                enumerator.name.name.clone(),
                OrdinarySymbol::Enumerator(value, QualifiedType::unqualified(ty)),
                enumerator.name.span,
            )?;
            typed.push(ccc_types::Enumerator {
                name: enumerator.name.name.clone(),
                value,
            });
        }
        let Some(underlying) = self.enum_underlying_type(&typed) else {
            return self.fail(
                "CCC2373",
                specifier.span,
                "enumerator values are not representable by a target integer type",
            );
        };
        self.types
            .complete_enum(enum_id, underlying, typed)
            .map_err(|error| {
                self.emit("CCC2245", specifier.span, error.to_string());
            })?;
        Ok(ty)
    }

    fn declare_typedef(
        &mut self,
        name: String,
        ty: QualifiedType,
        attributes: Vec<FullTypedAttribute>,
        span: Span,
    ) -> AnalysisResult<TypedefId> {
        if let Some(existing) = self.scopes.current_ordinary(&name).cloned() {
            if let OrdinarySymbol::Typedef(id, existing_ty) = existing
                && self.types_compatible(existing_ty, ty)
            {
                return Ok(id);
            }
            return self.fail(
                "CCC2246",
                span,
                format!("ordinary identifier `{name}` is redeclared incompatibly"),
            );
        }
        let id = TypedefId(self.typedefs.len() as u32);
        self.typedefs.push(FullTypedTypedef {
            id,
            name: name.clone(),
            ty,
            attributes,
            span,
        });
        self.bind_current(name, OrdinarySymbol::Typedef(id, ty), span)?;
        self.external_items.push(FullTypedExternalItem::Typedef(id));
        Ok(id)
    }

    #[allow(clippy::too_many_arguments)]
    fn declare_function(
        &mut self,
        name: String,
        signature: TypeId,
        storage: Option<syntax::StorageClass>,
        properties: FunctionProperties,
        attributes: Vec<FullTypedAttribute>,
        asm_label: Option<FullTypedAsmLabel>,
        span: Span,
    ) -> AnalysisResult<FullFunctionId> {
        if !matches!(
            storage,
            None | Some(syntax::StorageClass::Extern | syntax::StorageClass::Static)
        ) {
            return self.fail(
                "CCC2247",
                span,
                "a function may only have `extern` or `static` storage class",
            );
        }
        if let Some(existing) = self.lookup_file_ordinary(&name).cloned() {
            if let OrdinarySymbol::Function(id, existing_signature) = existing {
                let existing_linkage = self.functions[id.0 as usize].linkage;
                if storage == Some(syntax::StorageClass::Static)
                    && existing_linkage == Linkage::External
                {
                    return self.fail(
                        "CCC2372",
                        span,
                        format!(
                            "static declaration of function `{name}` follows a non-static declaration"
                        ),
                    );
                }
                let Some(composite) = self.composite_type_id(existing_signature, signature) else {
                    return self.fail(
                        "CCC2248",
                        span,
                        format!("function `{name}` is redeclared with an incompatible type"),
                    );
                };
                let function = &mut self.functions[id.0 as usize];
                function.signature = composite;
                function.properties.inline |= properties.inline;
                function.properties.no_return |= properties.no_return;
                if function.asm_label.is_none() {
                    function.asm_label = asm_label;
                }
                function.attributes.extend(attributes);
                self.scopes
                    .replace_file_ordinary(name, OrdinarySymbol::Function(id, composite));
                return Ok(id);
            }
            return self.fail(
                "CCC2249",
                span,
                format!("`{name}` was previously declared as a non-function"),
            );
        }
        let id = FullFunctionId(self.functions.len() as u32);
        let linkage = if storage == Some(syntax::StorageClass::Static) {
            Linkage::Internal
        } else {
            Linkage::External
        };
        let semantic_storage = if storage == Some(syntax::StorageClass::Static) {
            SemanticStorageClass::Static
        } else {
            SemanticStorageClass::Extern
        };
        self.functions.push(FullTypedFunction {
            id,
            name: name.clone(),
            signature,
            storage: semantic_storage,
            linkage,
            properties,
            parameters: Vec::new(),
            body: None,
            asm_label,
            attributes,
            span,
        });
        self.bind_file(name, OrdinarySymbol::Function(id, signature), span)?;
        Ok(id)
    }

    #[allow(clippy::too_many_arguments)]
    fn declare_global(
        &mut self,
        name: String,
        mut ty: QualifiedType,
        storage: Option<syntax::StorageClass>,
        attributes: Vec<FullTypedAttribute>,
        asm_label: Option<FullTypedAsmLabel>,
        initializer: Option<&syntax::Initializer>,
        span: Span,
    ) -> AnalysisResult<GlobalId> {
        if matches!(
            storage,
            Some(
                syntax::StorageClass::Typedef
                    | syntax::StorageClass::Auto
                    | syntax::StorageClass::Register
            )
        ) {
            return self.fail(
                "CCC2250",
                span,
                "this storage class is not valid on a file-scope object",
            );
        }
        self.validate_object_type(ty, span, false)?;
        let typed_initializer = if let Some(initializer) = initializer {
            let (typed, completed_ty) = self.analyze_initializer(ty, initializer)?;
            ty = completed_ty;
            if !initializer_is_static(&typed) {
                return self.fail(
                    "CCC2344",
                    span,
                    "a file-scope initializer must be a constant or relocatable address expression",
                );
            }
            Some(typed)
        } else {
            None
        };
        let tentative = initializer.is_none() && storage != Some(syntax::StorageClass::Extern);
        let definition = if initializer.is_some() {
            ObjectDefinitionPolicy::Definition
        } else if tentative {
            ObjectDefinitionPolicy::TentativeCommon
        } else {
            ObjectDefinitionPolicy::Declaration
        };
        let linkage = if storage == Some(syntax::StorageClass::Static) {
            Linkage::Internal
        } else {
            Linkage::External
        };
        let duration = if storage == Some(syntax::StorageClass::ThreadLocal) {
            StorageDuration::Thread
        } else {
            StorageDuration::Static
        };
        let semantic_storage = match storage {
            Some(syntax::StorageClass::Static) => SemanticStorageClass::Static,
            Some(syntax::StorageClass::ThreadLocal) => SemanticStorageClass::ThreadLocal,
            _ => SemanticStorageClass::Extern,
        };

        if let Some(existing) = self.lookup_file_ordinary(&name).cloned() {
            if let OrdinarySymbol::Global(id, existing_ty) = existing {
                let existing_linkage = self.globals[id.0 as usize].linkage;
                if storage == Some(syntax::StorageClass::Static)
                    && existing_linkage == Linkage::External
                {
                    return self.fail(
                        "CCC2372",
                        span,
                        format!(
                            "static declaration of object `{name}` follows a non-static declaration"
                        ),
                    );
                }
                let Some(composite) = self.composite_type(existing_ty, ty) else {
                    return self.fail(
                        "CCC2251",
                        span,
                        format!("object `{name}` is redeclared with an incompatible type"),
                    );
                };
                let global = &mut self.globals[id.0 as usize];
                if typed_initializer.is_some() && global.initializer.is_some() {
                    return self.fail(
                        "CCC2252",
                        span,
                        format!("object `{name}` is defined more than once"),
                    );
                }
                if typed_initializer.is_some() {
                    global.initializer = typed_initializer;
                    global.tentative = false;
                    global.emission.definition = ObjectDefinitionPolicy::Definition;
                } else if tentative
                    && global.emission.definition == ObjectDefinitionPolicy::Declaration
                {
                    global.tentative = true;
                    global.emission.definition = ObjectDefinitionPolicy::TentativeCommon;
                }
                global.ty = composite;
                self.scopes
                    .replace_file_ordinary(name, OrdinarySymbol::Global(id, composite));
                return Ok(id);
            }
            return self.fail(
                "CCC2253",
                span,
                format!("`{name}` was previously declared as a non-object"),
            );
        }

        let id = GlobalId(self.globals.len() as u32);
        let symbol_name = asm_label
            .as_ref()
            .map_or_else(|| name.clone(), |label| label.symbol.clone());
        let mut emission = GlobalEmission {
            symbol_name,
            visibility: SymbolVisibility::Default,
            section: None,
            requested_alignment: None,
            tls: (duration == StorageDuration::Thread).then_some(TlsModel::GeneralDynamic),
            definition,
        };
        self.apply_emission_attributes(&mut emission, &attributes, span)?;
        self.globals.push(FullTypedGlobal {
            id,
            name: name.clone(),
            ty,
            storage: semantic_storage,
            linkage,
            duration,
            initializer: typed_initializer,
            tentative,
            asm_label,
            attributes,
            emission,
            span,
        });
        self.bind_file(name, OrdinarySymbol::Global(id, ty), span)?;
        Ok(id)
    }

    fn analyze_block_declaration(
        &mut self,
        declaration: &syntax::Declaration,
    ) -> AnalysisResult<Vec<FullTypedBlockItem>> {
        let info = self.resolve_declaration_specifiers(&declaration.specifiers)?;
        let mut output = Vec::new();
        for init in &declaration.declarators {
            let resolved = self.resolve_declarator(info.base, &init.declarator)?;
            let Some(name) = resolved.name.clone() else {
                return self.fail(
                    "CCC2254",
                    init.span,
                    "a block-scope declarator must declare an identifier",
                );
            };
            let mut attributes = info.attributes.clone();
            attributes.extend(resolved.attributes);
            attributes.extend(self.validate_attributes(&init.attributes)?);
            if info.storage == Some(syntax::StorageClass::Typedef) {
                if init.initializer.is_some() || init.asm_label.is_some() {
                    return self.fail(
                        "CCC2255",
                        init.span,
                        "a typedef cannot have an initializer or assembly label",
                    );
                }
                let id = TypedefId(self.typedefs.len() as u32);
                let typed = FullTypedTypedef {
                    id,
                    name: name.clone(),
                    ty: resolved.ty,
                    attributes,
                    span: init.span,
                };
                self.bind_current(name, OrdinarySymbol::Typedef(id, resolved.ty), init.span)?;
                self.typedefs.push(typed.clone());
                output.push(FullTypedBlockItem::Typedef(Box::new(typed)));
                continue;
            }
            if self.types.function_signature(resolved.ty.ty).is_some() {
                let asm_label = self.resolve_asm_label(init.asm_label.as_ref())?;
                let id = self.declare_function(
                    name.clone(),
                    resolved.ty.ty,
                    info.storage,
                    info.properties,
                    attributes,
                    asm_label,
                    init.span,
                )?;
                let signature = self.functions[id.0 as usize].signature;
                self.bind_current(name, OrdinarySymbol::Function(id, signature), init.span)?;
                output.push(FullTypedBlockItem::FunctionDeclaration(id));
                continue;
            }
            if info.storage == Some(syntax::StorageClass::Extern) {
                if init.initializer.is_some() {
                    return self.fail(
                        "CCC2256",
                        init.span,
                        "a block-scope extern declaration cannot have an initializer",
                    );
                }
                let asm_label = self.resolve_asm_label(init.asm_label.as_ref())?;
                let id = self.declare_global(
                    name.clone(),
                    resolved.ty,
                    info.storage,
                    attributes,
                    asm_label,
                    None,
                    init.span,
                )?;
                let ty = self.globals[id.0 as usize].ty;
                self.bind_current(name, OrdinarySymbol::Global(id, ty), init.span)?;
                output.push(FullTypedBlockItem::ExternalObject(id));
                continue;
            }
            if init.asm_label.is_some() {
                return self.fail(
                    "CCC2257",
                    init.span,
                    "an automatic or static local cannot have an assembly label",
                );
            }
            self.validate_object_type(resolved.ty, init.span, init.initializer.is_none())?;
            if self.is_variably_modified(resolved.ty.ty) {
                return self.fail(
                    "CCC2258",
                    init.span,
                    "variably modified object storage is not supported",
                );
            }
            let local = self.fresh_local();
            self.bind_current(
                name.clone(),
                OrdinarySymbol::Local(local, resolved.ty),
                init.span,
            )?;
            let (storage, duration) = match info.storage {
                Some(syntax::StorageClass::Static) => {
                    (SemanticStorageClass::Static, StorageDuration::Static)
                }
                Some(syntax::StorageClass::Register) => {
                    (SemanticStorageClass::Register, StorageDuration::Automatic)
                }
                Some(syntax::StorageClass::ThreadLocal) => {
                    (SemanticStorageClass::ThreadLocal, StorageDuration::Thread)
                }
                None | Some(syntax::StorageClass::Auto) => {
                    (SemanticStorageClass::Automatic, StorageDuration::Automatic)
                }
                Some(syntax::StorageClass::Extern | syntax::StorageClass::Typedef) => {
                    unreachable!("handled above")
                }
                Some(syntax::StorageClass::GnuThreadLocal) => {
                    unreachable!("rejected with the declaration specifiers")
                }
            };
            let (initializer, completed_ty) = match init.initializer.as_ref() {
                Some(initializer) => {
                    let (typed, completed) = self.analyze_initializer(resolved.ty, initializer)?;
                    if duration != StorageDuration::Automatic && !initializer_is_static(&typed) {
                        return self.fail(
                            "CCC2367",
                            init.span,
                            "a static- or thread-duration block object requires a constant initializer",
                        );
                    }
                    (Some(typed), completed)
                }
                None => (None, resolved.ty),
            };
            self.validate_object_type(completed_ty, init.span, true)?;
            if completed_ty != resolved.ty {
                self.scopes.replace_current_ordinary(
                    name.clone(),
                    OrdinarySymbol::Local(local, completed_ty),
                );
            }
            let emission = if duration == StorageDuration::Automatic {
                None
            } else {
                let function = self
                    .function
                    .as_ref()
                    .expect("block objects occur inside a function");
                let mut emission = GlobalEmission {
                    symbol_name: format!(
                        "__ccc_block_static.{}.{}.{}.{}",
                        function.name, function.id.0, local.0, name
                    ),
                    visibility: SymbolVisibility::Internal,
                    section: None,
                    requested_alignment: None,
                    tls: (duration == StorageDuration::Thread).then_some(TlsModel::GeneralDynamic),
                    definition: ObjectDefinitionPolicy::Definition,
                };
                self.apply_emission_attributes(&mut emission, &attributes, init.span)?;
                Some(emission)
            };
            output.push(FullTypedBlockItem::Declaration(Box::new(
                FullTypedLocalDeclaration {
                    local,
                    name,
                    ty: completed_ty,
                    storage,
                    duration,
                    initializer,
                    attributes,
                    emission,
                    span: init.span,
                },
            )));
        }
        Ok(output)
    }

    fn analyze_statement(
        &mut self,
        statement: &syntax::Statement,
    ) -> AnalysisResult<FullTypedStatement> {
        use syntax::StatementKind as S;
        let kind =
            match &statement.kind {
                S::Label {
                    label,
                    statement: nested,
                    attributes,
                } => {
                    let _ = self.validate_attributes(attributes)?;
                    let id = {
                        let labels = &mut self
                            .function
                            .as_mut()
                            .expect("labels only occur inside functions")
                            .labels;
                        match labels.define(&label.name, label.span) {
                            Ok(id) => id,
                            Err(()) => {
                                return self.fail(
                                    "CCC2259",
                                    label.span,
                                    format!("label `{}` is defined more than once", label.name),
                                );
                            }
                        }
                    };
                    FullTypedStatementKind::Label {
                        label: id,
                        name: label.name.clone(),
                        statement: Box::new(self.analyze_statement(nested)?),
                    }
                }
                S::Case {
                    value,
                    statement: nested,
                } => {
                    let value = self.evaluate_integer_constant(value)?;
                    let Some(switch) = self
                        .function
                        .as_mut()
                        .and_then(|function| function.switches.last_mut())
                    else {
                        return self.fail(
                            "CCC2260",
                            statement.span,
                            "a `case` label must be inside a switch statement",
                        );
                    };
                    if let Some(previous) = switch.cases.insert(value, statement.span) {
                        self.diagnostics.push(
                            Diagnostic::error("CCC2261", format!("duplicate case value {value}"))
                                .with_primary(statement.span, "duplicate case")
                                .with_secondary(previous, "first used here"),
                        );
                        return Err(());
                    }
                    FullTypedStatementKind::Case {
                        value,
                        statement: Box::new(self.analyze_statement(nested)?),
                    }
                }
                S::Default(nested) => {
                    let Some(switch) = self
                        .function
                        .as_mut()
                        .and_then(|function| function.switches.last_mut())
                    else {
                        return self.fail(
                            "CCC2262",
                            statement.span,
                            "a `default` label must be inside a switch statement",
                        );
                    };
                    if let Some(previous) = switch.default.replace(statement.span) {
                        self.diagnostics.push(
                            Diagnostic::error("CCC2263", "multiple default labels in one switch")
                                .with_primary(statement.span, "duplicate default")
                                .with_secondary(previous, "first used here"),
                        );
                        return Err(());
                    }
                    FullTypedStatementKind::Default(Box::new(self.analyze_statement(nested)?))
                }
                S::Compound(items) => {
                    self.push_scope();
                    let result = self.analyze_compound_items(items);
                    self.pop_scope();
                    FullTypedStatementKind::Compound(result?)
                }
                S::Expression(expression) => {
                    let expression = expression
                        .as_ref()
                        .map(|expression| self.analyze_expression(expression))
                        .transpose()?
                        .map(|expression| self.value_conversion(expression))
                        .transpose()?;
                    FullTypedStatementKind::Expression(expression)
                }
                S::If {
                    condition,
                    then_statement,
                    else_statement,
                } => FullTypedStatementKind::If {
                    condition: self.analyze_condition(condition)?,
                    then_statement: Box::new(self.analyze_statement(then_statement)?),
                    else_statement: else_statement
                        .as_deref()
                        .map(|statement| self.analyze_statement(statement).map(Box::new))
                        .transpose()?,
                },
                S::Switch {
                    expression,
                    statement: nested,
                } => {
                    let expression = self.analyze_expression(expression)?;
                    let expression = self.value_conversion(expression)?;
                    let expression = self.integer_promote(expression)?;
                    if !self.types.is_integer(expression.ty.ty) {
                        return self.fail(
                            "CCC2264",
                            expression.span,
                            "a switch controlling expression must have integer type",
                        );
                    }
                    self.function
                        .as_mut()
                        .expect("switches only occur inside functions")
                        .switches
                        .push(SwitchState::default());
                    let nested = self.analyze_statement(nested);
                    self.function
                        .as_mut()
                        .expect("switch state exists")
                        .switches
                        .pop();
                    FullTypedStatementKind::Switch {
                        expression,
                        statement: Box::new(nested?),
                    }
                }
                S::While {
                    condition,
                    statement: nested,
                } => {
                    let condition = self.analyze_condition(condition)?;
                    self.enter_loop();
                    let nested = self.analyze_statement(nested);
                    self.leave_loop();
                    FullTypedStatementKind::While {
                        condition,
                        statement: Box::new(nested?),
                    }
                }
                S::DoWhile {
                    statement: nested,
                    condition,
                } => {
                    self.enter_loop();
                    let nested = self.analyze_statement(nested);
                    self.leave_loop();
                    FullTypedStatementKind::DoWhile {
                        statement: Box::new(nested?),
                        condition: self.analyze_condition(condition)?,
                    }
                }
                S::For {
                    initializer,
                    condition,
                    step,
                    statement: nested,
                } => {
                    self.push_scope();
                    let result = self.analyze_for_statement(
                        initializer,
                        condition.as_deref(),
                        step.as_deref(),
                        nested,
                    );
                    self.pop_scope();
                    let (initializer, condition, step, nested) = result?;
                    FullTypedStatementKind::For {
                        initializer,
                        condition: condition.map(Box::new),
                        step: step.map(Box::new),
                        statement: Box::new(nested),
                    }
                }
                S::Goto(label) => {
                    let labels = &mut self
                        .function
                        .as_mut()
                        .expect("gotos only occur inside functions")
                        .labels;
                    let id = labels.note_use(&label.name, label.span);
                    FullTypedStatementKind::Goto {
                        label: id,
                        name: label.name.clone(),
                    }
                }
                S::Continue => {
                    if self
                        .function
                        .as_ref()
                        .is_none_or(|function| function.loop_depth == 0)
                    {
                        return self.fail(
                            "CCC2265",
                            statement.span,
                            "a `continue` statement must be inside a loop",
                        );
                    }
                    FullTypedStatementKind::Continue
                }
                S::Break => {
                    if self.function.as_ref().is_none_or(|function| {
                        function.loop_depth == 0 && function.switches.is_empty()
                    }) {
                        return self.fail(
                            "CCC2266",
                            statement.span,
                            "a `break` statement must be inside a loop or switch",
                        );
                    }
                    FullTypedStatementKind::Break
                }
                S::Return(expression) => {
                    let return_ty = self
                        .function
                        .as_ref()
                        .expect("returns only occur inside functions")
                        .return_ty;
                    let expression = match (return_ty.ty == TypeId::VOID, expression) {
                        (true, None) => None,
                        (true, Some(expression)) => {
                            return self.fail(
                                "CCC2267",
                                expression.span,
                                "a void function cannot return a value",
                            );
                        }
                        (false, None) => {
                            return self.fail(
                                "CCC2268",
                                statement.span,
                                "a non-void function must return a value",
                            );
                        }
                        (false, Some(expression)) => {
                            let typed = self.analyze_expression(expression)?;
                            Some(self.assignment_conversion(typed, return_ty, expression.span)?)
                        }
                    };
                    FullTypedStatementKind::Return(expression)
                }
            };
        Ok(FullTypedStatement {
            kind,
            span: statement.span,
        })
    }

    fn analyze_compound_items(
        &mut self,
        items: &[syntax::BlockItem],
    ) -> AnalysisResult<Vec<FullTypedBlockItem>> {
        let mut output = Vec::new();
        for item in items {
            match item {
                syntax::BlockItem::Declaration(declaration) => {
                    output.extend(self.analyze_block_declaration(declaration)?);
                }
                syntax::BlockItem::StaticAssert(assertion) => {
                    let value = self.analyze_static_assert(assertion)?;
                    output.push(FullTypedBlockItem::StaticAssert {
                        value,
                        span: assertion.span,
                    });
                }
                syntax::BlockItem::Statement(statement) => output.push(
                    FullTypedBlockItem::Statement(Box::new(self.analyze_statement(statement)?)),
                ),
                syntax::BlockItem::Pragma(pragma) => {
                    self.handle_pragma(pragma)?;
                    output.push(FullTypedBlockItem::Pragma(pragma.clone()));
                }
            }
        }
        Ok(output)
    }

    fn analyze_for_statement(
        &mut self,
        initializer: &syntax::ForInitializer,
        condition: Option<&syntax::Expression>,
        step: Option<&syntax::Expression>,
        nested: &syntax::Statement,
    ) -> AnalysisResult<(
        FullTypedForInitializer,
        Option<FullTypedExpression>,
        Option<FullTypedExpression>,
        FullTypedStatement,
    )> {
        let initializer = match initializer {
            syntax::ForInitializer::Empty => FullTypedForInitializer::Empty,
            syntax::ForInitializer::Expression(expression) => {
                FullTypedForInitializer::Expression(self.analyze_expression(expression)?)
            }
            syntax::ForInitializer::Declaration(declaration) => {
                FullTypedForInitializer::Declarations(self.analyze_block_declaration(declaration)?)
            }
        };
        let condition = condition
            .map(|condition| self.analyze_condition(condition))
            .transpose()?;
        let step = step.map(|step| self.analyze_expression(step)).transpose()?;
        self.enter_loop();
        let nested = self.analyze_statement(nested);
        self.leave_loop();
        Ok((initializer, condition, step, nested?))
    }

    fn analyze_static_assert(&mut self, assertion: &syntax::StaticAssert) -> AnalysisResult<i128> {
        let value = self.evaluate_integer_constant(&assertion.condition)?;
        if value == 0 {
            return self.fail("CCC2269", assertion.span, "static assertion failed");
        }
        Ok(value)
    }

    fn analyze_expression(
        &mut self,
        expression: &syntax::Expression,
    ) -> AnalysisResult<FullTypedExpression> {
        use syntax::ExpressionKind as E;
        match &expression.kind {
            E::Identifier(identifier) => self.analyze_identifier(identifier),
            E::Integer(integer) => self.analyze_integer_literal(*integer, expression.span),
            E::Floating(floating) => {
                let ty = match floating.suffix {
                    FloatingConstantSuffix::Float => TypeId::FLOAT,
                    FloatingConstantSuffix::Double => TypeId::DOUBLE,
                    FloatingConstantSuffix::LongDouble => TypeId::LONG_DOUBLE,
                };
                Ok(self.constant_expression(
                    ConstantValue::Floating(floating.value),
                    QualifiedType::unqualified(ty),
                    expression.span,
                ))
            }
            E::Character(character) => {
                let ty = match character.prefix {
                    CharacterConstantPrefix::None => TypeId::INT,
                    CharacterConstantPrefix::Wide => self.wchar_type(),
                    CharacterConstantPrefix::Utf16 => TypeId::UNSIGNED_SHORT,
                    CharacterConstantPrefix::Utf32 => TypeId::UNSIGNED_INT,
                };
                let constant = if character.prefix == CharacterConstantPrefix::None
                    && character.character_count == 1
                    && self.config.target.data_layout.char_is_signed
                {
                    ConstantValue::Signed(sign_extend(
                        u128::from(character.value),
                        self.config.target.data_layout.char_width,
                    ))
                } else {
                    ConstantValue::Unsigned(u128::from(character.value))
                };
                Ok(self.constant_expression(
                    constant,
                    QualifiedType::unqualified(ty),
                    expression.span,
                ))
            }
            E::String(literal) => self.analyze_string_literal(literal, expression.span),
            E::Parenthesized(inner) => {
                let mut typed = self.analyze_expression(inner)?;
                typed.span = expression.span;
                Ok(typed)
            }
            E::GenericSelection { .. } => self.fail(
                "CCC2270",
                expression.span,
                "generic selections are parsed but are not semantically supported",
            ),
            E::Subscript { base, index } => self.analyze_subscript(base, index, expression.span),
            E::Call { callee, arguments } => self.analyze_call(callee, arguments, expression.span),
            E::Member {
                base,
                member,
                indirect,
            } => self.analyze_member(base, member, *indirect, expression.span),
            E::PostfixIncrement(operand) => {
                self.analyze_increment(operand, false, true, expression.span)
            }
            E::PostfixDecrement(operand) => {
                self.analyze_increment(operand, true, true, expression.span)
            }
            E::CompoundLiteral { .. } => self.fail(
                "CCC2271",
                expression.span,
                "compound literals are parsed but are not semantically supported",
            ),
            E::Unary { operator, operand } => {
                self.analyze_unary(*operator, operand, expression.span)
            }
            E::SizeofExpression(operand) => {
                let operand = self.analyze_expression(operand)?;
                self.analyze_sizeof(operand.ty, expression.span)
            }
            E::SizeofType(type_name) => {
                let ty = self.resolve_type_name(type_name)?;
                self.analyze_sizeof(ty, expression.span)
            }
            E::AlignofType(type_name) => {
                let ty = self.resolve_type_name(type_name)?;
                let layout = self.types.layout_of(ty.ty, self.config).map_err(|error| {
                    self.emit("CCC2272", expression.span, error.to_string());
                })?;
                let result_ty = QualifiedType::unqualified(self.size_type());
                Ok(FullTypedExpression {
                    kind: FullTypedExpressionKind::Alignof {
                        operand_ty: ty,
                        align: layout.align,
                    },
                    ty: result_ty,
                    category: ValueCategory::Value,
                    place: None,
                    constant: Some(ConstantValue::Unsigned(u128::from(layout.align))),
                    span: expression.span,
                })
            }
            E::Cast {
                ty: type_name,
                expression: operand,
            } => {
                let target = self.resolve_type_name(type_name)?;
                let operand = self.analyze_expression(operand)?;
                let operand = self.value_conversion(operand)?;
                self.explicit_conversion(operand, target, expression.span)
            }
            E::Binary {
                operator,
                left,
                right,
            } => self.analyze_binary(*operator, left, right, expression.span),
            E::Conditional {
                condition,
                then_expression,
                else_expression,
            } => self.analyze_conditional(
                condition,
                then_expression,
                else_expression,
                expression.span,
            ),
            E::Assignment {
                operator,
                target,
                value,
            } => self.analyze_assignment(*operator, target, value, expression.span),
            E::Comma(expressions) => {
                let mut typed = Vec::new();
                for (index, item) in expressions.iter().enumerate() {
                    let item = self.analyze_expression(item)?;
                    typed.push(if index + 1 == expressions.len() {
                        item
                    } else {
                        self.value_conversion(item)?
                    });
                }
                let Some(last) = typed.last() else {
                    return self.fail("CCC2273", expression.span, "an empty comma expression");
                };
                Ok(FullTypedExpression {
                    kind: FullTypedExpressionKind::Comma(typed.clone()),
                    ty: last.ty,
                    category: last.category,
                    place: last.place.clone(),
                    constant: last.constant,
                    span: expression.span,
                })
            }
            E::Extension(inner) => self.analyze_expression(inner),
            E::BuiltinOffsetof { ty, designator } => {
                self.analyze_offsetof(ty, designator, expression.span)
            }
        }
    }

    fn analyze_identifier(
        &mut self,
        identifier: &syntax::Identifier,
    ) -> AnalysisResult<FullTypedExpression> {
        let Some(symbol) = self.lookup_ordinary(&identifier.name).cloned() else {
            return self.fail(
                "CCC2274",
                identifier.span,
                format!("use of undeclared identifier `{}`", identifier.name),
            );
        };
        let (reference, ty, category, place, constant) = match symbol {
            OrdinarySymbol::Global(id, ty) => (
                SymbolReference::Global(id),
                ty,
                ValueCategory::Lvalue,
                Some(self.object_place(PlaceBase::Global(id), ty, true)),
                None,
            ),
            OrdinarySymbol::Function(id, ty) => (
                SymbolReference::Function(id),
                QualifiedType::unqualified(ty),
                ValueCategory::FunctionDesignator,
                None,
                Some(ConstantValue::Address(RelocatableAddress {
                    base: RelocatableBase::Function(id),
                    addend: 0,
                    one_past: false,
                })),
            ),
            OrdinarySymbol::Local(id, ty) => (
                SymbolReference::Local(id),
                ty,
                ValueCategory::Lvalue,
                Some(self.object_place(PlaceBase::Local(id), ty, true)),
                None,
            ),
            OrdinarySymbol::Enumerator(value, ty) => {
                let constant = if self.is_signed_integer(ty.ty) {
                    ConstantValue::Signed(value)
                } else {
                    ConstantValue::Unsigned(value as u128)
                };
                (
                    SymbolReference::Enumerator { value },
                    ty,
                    ValueCategory::Value,
                    None,
                    Some(constant),
                )
            }
            OrdinarySymbol::Typedef(_, _) => {
                return self.fail(
                    "CCC2275",
                    identifier.span,
                    format!("typedef name `{}` is not an expression", identifier.name),
                );
            }
        };
        Ok(FullTypedExpression {
            kind: FullTypedExpressionKind::DeclRef(reference),
            ty,
            category,
            place,
            constant,
            span: identifier.span,
        })
    }

    fn analyze_integer_literal(
        &mut self,
        integer: ccc_pp::IntegerConstant,
        span: Span,
    ) -> AnalysisResult<FullTypedExpression> {
        let signed = [BuiltinType::Int, BuiltinType::Long, BuiltinType::LongLong];
        let unsigned = [
            BuiltinType::UnsignedInt,
            BuiltinType::UnsignedLong,
            BuiltinType::UnsignedLongLong,
        ];
        let candidates: Vec<BuiltinType> =
            match (integer.suffix.unsigned, integer.suffix.long_count) {
                (true, 0) => unsigned.to_vec(),
                (true, 1) => unsigned[1..].to_vec(),
                (true, _) => vec![BuiltinType::UnsignedLongLong],
                (false, 1) if integer.radix == 10 => signed[1..].to_vec(),
                (false, 2) if integer.radix == 10 => vec![BuiltinType::LongLong],
                (false, 0) if integer.radix == 10 => signed.to_vec(),
                (false, 0) => vec![
                    BuiltinType::Int,
                    BuiltinType::UnsignedInt,
                    BuiltinType::Long,
                    BuiltinType::UnsignedLong,
                    BuiltinType::LongLong,
                    BuiltinType::UnsignedLongLong,
                ],
                (false, 1) => vec![
                    BuiltinType::Long,
                    BuiltinType::UnsignedLong,
                    BuiltinType::LongLong,
                    BuiltinType::UnsignedLongLong,
                ],
                (false, _) => vec![BuiltinType::LongLong, BuiltinType::UnsignedLongLong],
            };
        let Some(kind) = candidates
            .into_iter()
            .find(|candidate| self.integer_fits(*candidate, integer.value))
        else {
            return self.fail(
                "CCC2276",
                span,
                "integer constant is not representable in any candidate type",
            );
        };
        let constant = if is_signed_builtin(kind) {
            ConstantValue::Signed(integer.value as i128)
        } else {
            ConstantValue::Unsigned(integer.value)
        };
        Ok(self.constant_expression(
            constant,
            QualifiedType::unqualified(self.types.builtin(kind)),
            span,
        ))
    }

    fn analyze_string_literal(
        &mut self,
        literal: &ccc_pp::StringLiteral,
        span: Span,
    ) -> AnalysisResult<FullTypedExpression> {
        let element = match literal.prefix {
            StringLiteralPrefix::None | StringLiteralPrefix::Utf8 => TypeId::CHAR,
            StringLiteralPrefix::Wide => self.wchar_type(),
            StringLiteralPrefix::Utf16 => TypeId::UNSIGNED_SHORT,
            StringLiteralPrefix::Utf32 => TypeId::UNSIGNED_INT,
        };
        let mut code_units = literal.code_units.clone();
        code_units.push(0);
        let alignment = self
            .types
            .layout_of(element, self.config)
            .map_err(|error| {
                self.emit(
                    "CCC2371",
                    span,
                    format!("cannot determine string literal alignment: {error}"),
                );
            })?
            .align;
        let key = StringPoolKey {
            element,
            encoding: literal.prefix.into(),
            code_units: code_units.clone(),
            alignment,
            mutable: false,
        };
        let array_ty = self.types.array(ArrayType {
            element: QualifiedType::unqualified(element),
            length: ArrayLength::Constant(code_units.len() as u64),
        });
        let ty = QualifiedType::unqualified(array_ty);
        let id = if let Some(id) = self.string_pool.get(&key).copied() {
            id
        } else {
            let id = StringId(self.strings.len() as u32);
            self.strings.push(FullTypedString {
                id,
                prefix: literal.prefix,
                code_units,
                ty,
            });
            self.string_pool.insert(key, id);
            id
        };
        Ok(FullTypedExpression {
            kind: FullTypedExpressionKind::StringLiteral(id),
            ty,
            category: ValueCategory::Lvalue,
            place: Some(Place {
                base: PlaceBase::String(id),
                projections: Vec::new(),
                access: AccessSemantics::default(),
                modifiable: false,
                addressable: true,
                bitfield: None,
            }),
            constant: Some(ConstantValue::Address(RelocatableAddress {
                base: RelocatableBase::String(id),
                addend: 0,
                one_past: false,
            })),
            span,
        })
    }

    fn analyze_unary(
        &mut self,
        operator: syntax::UnaryOperator,
        operand: &syntax::Expression,
        span: Span,
    ) -> AnalysisResult<FullTypedExpression> {
        use syntax::UnaryOperator as U;
        match operator {
            U::PrefixIncrement => self.analyze_increment(operand, false, false, span),
            U::PrefixDecrement => self.analyze_increment(operand, true, false, span),
            U::Address => {
                let operand = self.analyze_expression(operand)?;
                if !matches!(
                    operand.category,
                    ValueCategory::Lvalue | ValueCategory::FunctionDesignator
                ) || operand
                    .place
                    .as_ref()
                    .is_some_and(|place| !place.addressable || place.bitfield.is_some())
                {
                    return self.fail(
                        "CCC2277",
                        span,
                        "the operand of `&` must be an addressable lvalue or function",
                    );
                }
                let ty = QualifiedType::unqualified(self.types.pointer(operand.ty));
                let constant = match &operand.kind {
                    FullTypedExpressionKind::DeclRef(SymbolReference::Global(id)) => {
                        Some(ConstantValue::Address(RelocatableAddress {
                            base: RelocatableBase::Global(*id),
                            addend: 0,
                            one_past: false,
                        }))
                    }
                    FullTypedExpressionKind::DeclRef(SymbolReference::Function(id)) => {
                        Some(ConstantValue::Address(RelocatableAddress {
                            base: RelocatableBase::Function(*id),
                            addend: 0,
                            one_past: false,
                        }))
                    }
                    FullTypedExpressionKind::StringLiteral(id) => {
                        Some(ConstantValue::Address(RelocatableAddress {
                            base: RelocatableBase::String(*id),
                            addend: 0,
                            one_past: false,
                        }))
                    }
                    _ => None,
                };
                Ok(FullTypedExpression {
                    kind: FullTypedExpressionKind::AddressOf(Box::new(operand)),
                    ty,
                    category: ValueCategory::Value,
                    place: None,
                    constant,
                    span,
                })
            }
            U::Dereference => {
                let operand = self.analyze_expression(operand)?;
                let operand = self.value_conversion(operand)?;
                let Some(pointee) = self.pointer_pointee(operand.ty.ty) else {
                    return self.fail("CCC2278", span, "the operand of `*` must be a pointer");
                };
                let category = if self.types.function_signature(pointee.ty).is_some() {
                    ValueCategory::FunctionDesignator
                } else {
                    ValueCategory::Lvalue
                };
                let place = (category == ValueCategory::Lvalue).then(|| {
                    let mut place = self.object_place(PlaceBase::Indirect, pointee, true);
                    place.projections.push(PlaceProjection::Dereference);
                    place
                });
                Ok(FullTypedExpression {
                    kind: FullTypedExpressionKind::Dereference(Box::new(operand)),
                    ty: pointee,
                    category,
                    place,
                    constant: None,
                    span,
                })
            }
            U::LogicalNot => {
                let operand = self.analyze_condition(operand)?;
                let constant = operand
                    .constant
                    .map(|value| ConstantValue::Signed(i128::from(value.is_zero())));
                Ok(FullTypedExpression {
                    kind: FullTypedExpressionKind::Unary {
                        operator,
                        operand: Box::new(operand),
                    },
                    ty: QualifiedType::unqualified(TypeId::INT),
                    category: ValueCategory::Value,
                    place: None,
                    constant,
                    span,
                })
            }
            U::Plus | U::Minus | U::BitwiseNot => {
                let operand = self.analyze_expression(operand)?;
                let operand = self.value_conversion(operand)?;
                let operand =
                    self.integer_or_arithmetic_promotion(operand, operator != U::BitwiseNot, span)?;
                self.reject_long_double_operation(&[operand.ty], span)?;
                let constant = operand.constant.and_then(|value| match (operator, value) {
                    (U::Plus, value) => Some(value),
                    (U::Minus, ConstantValue::Signed(value)) => {
                        value.checked_neg().map(ConstantValue::Signed)
                    }
                    (U::Minus, ConstantValue::Unsigned(value)) => {
                        Some(ConstantValue::Unsigned(value.wrapping_neg()))
                    }
                    (U::Minus, ConstantValue::Floating(value)) => {
                        Some(ConstantValue::Floating(-value))
                    }
                    (U::BitwiseNot, ConstantValue::Signed(value)) => {
                        Some(ConstantValue::Signed(!value))
                    }
                    (U::BitwiseNot, ConstantValue::Unsigned(value)) => {
                        Some(ConstantValue::Unsigned(!value))
                    }
                    _ => None,
                });
                Ok(FullTypedExpression {
                    kind: FullTypedExpressionKind::Unary {
                        operator,
                        operand: Box::new(operand.clone()),
                    },
                    ty: operand.ty,
                    category: ValueCategory::Value,
                    place: None,
                    constant,
                    span,
                })
            }
        }
    }

    fn analyze_binary(
        &mut self,
        operator: syntax::BinaryOperator,
        left: &syntax::Expression,
        right: &syntax::Expression,
        span: Span,
    ) -> AnalysisResult<FullTypedExpression> {
        use syntax::BinaryOperator as B;
        let left = self.analyze_expression(left)?;
        let left = self.value_conversion(left)?;
        let right = self.analyze_expression(right)?;
        let right = self.value_conversion(right)?;
        if matches!(operator, B::LogicalAnd | B::LogicalOr) {
            let left = self.convert_to_boolean(left)?;
            let right = self.convert_to_boolean(right)?;
            let constant = evaluate_binary_constant(operator, left.constant, right.constant);
            return Ok(FullTypedExpression {
                kind: FullTypedExpressionKind::Binary {
                    operator,
                    left: Box::new(left),
                    right: Box::new(right),
                },
                ty: QualifiedType::unqualified(TypeId::INT),
                category: ValueCategory::Value,
                place: None,
                constant,
                span,
            });
        }

        if matches!(operator, B::Add | B::Subtract) {
            if self.pointer_pointee(left.ty.ty).is_some() && self.types.is_integer(right.ty.ty) {
                let right = self.integer_promote(right)?;
                let constant = self.evaluate_pointer_arithmetic(
                    left.constant,
                    right.constant,
                    left.ty,
                    operator == B::Subtract,
                );
                return Ok(FullTypedExpression {
                    kind: FullTypedExpressionKind::Binary {
                        operator,
                        left: Box::new(left.clone()),
                        right: Box::new(right),
                    },
                    ty: left.ty,
                    category: ValueCategory::Value,
                    place: None,
                    constant,
                    span,
                });
            }
            if operator == B::Add
                && self.types.is_integer(left.ty.ty)
                && self.pointer_pointee(right.ty.ty).is_some()
            {
                let left = self.integer_promote(left)?;
                let constant = self.evaluate_pointer_arithmetic(
                    right.constant,
                    left.constant,
                    right.ty,
                    false,
                );
                return Ok(FullTypedExpression {
                    kind: FullTypedExpressionKind::Binary {
                        operator,
                        left: Box::new(left),
                        right: Box::new(right.clone()),
                    },
                    ty: right.ty,
                    category: ValueCategory::Value,
                    place: None,
                    constant,
                    span,
                });
            }
            if operator == B::Subtract
                && self.pointer_pointee(left.ty.ty).is_some()
                && self.pointer_pointee(right.ty.ty).is_some()
            {
                if !self.pointer_types_compatible(left.ty.ty, right.ty.ty) {
                    return self.fail(
                        "CCC2279",
                        span,
                        "pointer subtraction requires pointers to compatible types",
                    );
                }
                return Ok(FullTypedExpression {
                    kind: FullTypedExpressionKind::Binary {
                        operator,
                        left: Box::new(left),
                        right: Box::new(right),
                    },
                    ty: QualifiedType::unqualified(self.ptrdiff_type()),
                    category: ValueCategory::Value,
                    place: None,
                    constant: None,
                    span,
                });
            }
        }

        if matches!(
            operator,
            B::Equal | B::NotEqual | B::Less | B::LessEqual | B::Greater | B::GreaterEqual
        ) && (self.pointer_pointee(left.ty.ty).is_some()
            || self.pointer_pointee(right.ty.ty).is_some())
        {
            let (left, right) = self.convert_pointer_comparison(left, right, span)?;
            let constant = evaluate_binary_constant(operator, left.constant, right.constant);
            return Ok(FullTypedExpression {
                kind: FullTypedExpressionKind::Binary {
                    operator,
                    left: Box::new(left),
                    right: Box::new(right),
                },
                ty: QualifiedType::unqualified(TypeId::INT),
                category: ValueCategory::Value,
                place: None,
                constant,
                span,
            });
        }

        if matches!(operator, B::LeftShift | B::RightShift) {
            if !self.types.is_integer(left.ty.ty) || !self.types.is_integer(right.ty.ty) {
                return self.fail("CCC2280", span, "shift operands must have integer type");
            }
            let left = self.integer_promote(left)?;
            let right = self.integer_promote(right)?;
            let constant = evaluate_binary_constant(operator, left.constant, right.constant);
            return Ok(FullTypedExpression {
                kind: FullTypedExpressionKind::Binary {
                    operator,
                    left: Box::new(left.clone()),
                    right: Box::new(right),
                },
                ty: left.ty,
                category: ValueCategory::Value,
                place: None,
                constant,
                span,
            });
        }

        let integers_required = matches!(
            operator,
            B::Remainder | B::BitwiseAnd | B::BitwiseXor | B::BitwiseOr
        );
        if integers_required {
            if !self.types.is_integer(left.ty.ty) || !self.types.is_integer(right.ty.ty) {
                return self.fail("CCC2281", span, "operator requires integer operands");
            }
        } else if !self.types.is_arithmetic(left.ty.ty) || !self.types.is_arithmetic(right.ty.ty) {
            return self.fail("CCC2282", span, "operator requires arithmetic operands");
        }
        let (left, right, common) = self.usual_arithmetic_conversions(left, right, span)?;
        self.reject_long_double_operation(&[common], span)?;
        let result_ty = if matches!(
            operator,
            B::Less | B::LessEqual | B::Greater | B::GreaterEqual | B::Equal | B::NotEqual
        ) {
            QualifiedType::unqualified(TypeId::INT)
        } else {
            common
        };
        let constant = evaluate_binary_constant(operator, left.constant, right.constant);
        Ok(FullTypedExpression {
            kind: FullTypedExpressionKind::Binary {
                operator,
                left: Box::new(left),
                right: Box::new(right),
            },
            ty: result_ty,
            category: ValueCategory::Value,
            place: None,
            constant,
            span,
        })
    }

    fn analyze_assignment(
        &mut self,
        operator: syntax::AssignmentOperator,
        target: &syntax::Expression,
        value: &syntax::Expression,
        span: Span,
    ) -> AnalysisResult<FullTypedExpression> {
        let target = self.analyze_expression(target)?;
        let Some(place) = target.place.as_ref() else {
            return self.fail(
                "CCC2283",
                target.span,
                "assignment requires an lvalue target",
            );
        };
        if !place.modifiable {
            return self.fail(
                "CCC2284",
                target.span,
                "assignment target is not a modifiable lvalue",
            );
        }
        let (value, compound) = if operator == syntax::AssignmentOperator::Assign {
            let value = self.analyze_expression(value)?;
            let value_span = value.span;
            (
                self.assignment_conversion(value, target.ty, value_span)?,
                None,
            )
        } else {
            let (value, plan) =
                self.analyze_compound_assignment_value(operator, &target, value, span)?;
            (value, Some(plan))
        };
        let store = place.access;
        Ok(FullTypedExpression {
            kind: FullTypedExpressionKind::Assignment {
                operator,
                target: Box::new(target.clone()),
                value: Box::new(value),
                store,
                compound,
            },
            ty: QualifiedType::unqualified(target.ty.ty),
            category: ValueCategory::Value,
            place: None,
            constant: None,
            span,
        })
    }

    fn analyze_compound_assignment_value(
        &mut self,
        operator: syntax::AssignmentOperator,
        target: &FullTypedExpression,
        value: &syntax::Expression,
        span: Span,
    ) -> AnalysisResult<(FullTypedExpression, CompoundAssignmentPlan)> {
        use syntax::AssignmentOperator as A;
        let binary = match operator {
            A::Multiply => syntax::BinaryOperator::Multiply,
            A::Divide => syntax::BinaryOperator::Divide,
            A::Remainder => syntax::BinaryOperator::Remainder,
            A::Add => syntax::BinaryOperator::Add,
            A::Subtract => syntax::BinaryOperator::Subtract,
            A::LeftShift => syntax::BinaryOperator::LeftShift,
            A::RightShift => syntax::BinaryOperator::RightShift,
            A::BitwiseAnd => syntax::BinaryOperator::BitwiseAnd,
            A::BitwiseXor => syntax::BinaryOperator::BitwiseXor,
            A::BitwiseOr => syntax::BinaryOperator::BitwiseOr,
            A::Assign => unreachable!("simple assignment was handled separately"),
        };
        let right = self.analyze_expression(value)?;
        let right = self.value_conversion(right)?;
        use syntax::BinaryOperator as B;
        let load = target
            .place
            .as_ref()
            .map_or_else(|| access_semantics(target.ty), |place| place.access);
        if matches!(binary, B::Add | B::Subtract)
            && self.pointer_pointee(target.ty.ty).is_some()
            && self.types.is_integer(right.ty.ty)
        {
            let right = self.integer_promote(right)?;
            return Ok((
                right,
                CompoundAssignmentPlan {
                    operator: binary,
                    load_ty: QualifiedType::unqualified(target.ty.ty),
                    calculation_ty: QualifiedType::unqualified(target.ty.ty),
                    load,
                    result_conversion: None,
                },
            ));
        }
        let integers_required = matches!(
            binary,
            B::Remainder
                | B::LeftShift
                | B::RightShift
                | B::BitwiseAnd
                | B::BitwiseXor
                | B::BitwiseOr
        );
        if integers_required
            && (!self.types.is_integer(target.ty.ty) || !self.types.is_integer(right.ty.ty))
        {
            return self.fail(
                "CCC2285",
                span,
                "compound assignment requires integer operands",
            );
        }
        if !integers_required
            && (!self.types.is_arithmetic(target.ty.ty) || !self.types.is_arithmetic(right.ty.ty))
        {
            return self.fail(
                "CCC2286",
                span,
                "compound assignment requires arithmetic operands",
            );
        }
        if matches!(binary, B::LeftShift | B::RightShift) {
            let load_ty = QualifiedType::unqualified(self.promoted_integer_type(target.ty.ty));
            let right = self.integer_promote(right)?;
            return Ok((
                right,
                CompoundAssignmentPlan {
                    operator: binary,
                    load_ty,
                    calculation_ty: load_ty,
                    load,
                    result_conversion: (load_ty.ty != target.ty.ty)
                        .then_some(ConversionKind::IntegerConversion),
                },
            ));
        }
        let left_ty = if self.types.is_integer(target.ty.ty) {
            self.promoted_integer_type(target.ty.ty)
        } else {
            target.ty.ty
        };
        let right = if self.types.is_integer(right.ty.ty) {
            self.integer_promote(right)?
        } else {
            right
        };
        let common_ty = self.common_arithmetic_type(left_ty, right.ty.ty);
        let common = QualifiedType::unqualified(common_ty);
        let right = self.arithmetic_conversion(right, common)?;
        self.reject_long_double_operation(&[common], span)?;
        let result_conversion = (common.ty != target.ty.ty)
            .then_some(self.arithmetic_conversion_kind(common.ty, target.ty.ty));
        Ok((
            right,
            CompoundAssignmentPlan {
                operator: binary,
                load_ty: common,
                calculation_ty: common,
                load,
                result_conversion,
            },
        ))
    }

    fn analyze_increment(
        &mut self,
        operand: &syntax::Expression,
        decrement: bool,
        postfix: bool,
        span: Span,
    ) -> AnalysisResult<FullTypedExpression> {
        let operand = self.analyze_expression(operand)?;
        let Some(place) = operand.place.as_ref() else {
            return self.fail("CCC2287", span, "increment requires an lvalue operand");
        };
        if !place.modifiable
            || (!self.types.is_arithmetic(operand.ty.ty)
                && self.pointer_pointee(operand.ty.ty).is_none())
        {
            return self.fail(
                "CCC2288",
                span,
                "increment requires a modifiable scalar lvalue",
            );
        }
        self.reject_long_double_operation(&[operand.ty], span)?;
        let store = place.access;
        Ok(FullTypedExpression {
            kind: FullTypedExpressionKind::Increment {
                operand: Box::new(operand.clone()),
                decrement,
                postfix,
                store,
            },
            ty: QualifiedType::unqualified(operand.ty.ty),
            category: ValueCategory::Value,
            place: None,
            constant: None,
            span,
        })
    }

    fn analyze_conditional(
        &mut self,
        condition: &syntax::Expression,
        then_expression: &syntax::Expression,
        else_expression: &syntax::Expression,
        span: Span,
    ) -> AnalysisResult<FullTypedExpression> {
        let condition = self.analyze_condition(condition)?;
        let then_expression = self.analyze_expression(then_expression)?;
        let then_expression = self.value_conversion(then_expression)?;
        let else_expression = self.analyze_expression(else_expression)?;
        let else_expression = self.value_conversion(else_expression)?;
        let (then_expression, else_expression, ty) =
            if self.types.is_arithmetic(then_expression.ty.ty)
                && self.types.is_arithmetic(else_expression.ty.ty)
            {
                self.usual_arithmetic_conversions(then_expression, else_expression, span)?
            } else if self.pointer_pointee(then_expression.ty.ty).is_some()
                || self.pointer_pointee(else_expression.ty.ty).is_some()
            {
                let (left, right) =
                    self.convert_pointer_comparison(then_expression, else_expression, span)?;
                let ty = left.ty;
                (left, right, ty)
            } else if self.types_compatible(then_expression.ty, else_expression.ty) {
                let ty = then_expression.ty;
                (then_expression, else_expression, ty)
            } else {
                return self.fail(
                    "CCC2289",
                    span,
                    "conditional operands do not have a compatible common type",
                );
            };
        let constant = match condition.constant {
            Some(value) if value.is_zero() => else_expression.constant,
            Some(_) => then_expression.constant,
            None => None,
        };
        Ok(FullTypedExpression {
            kind: FullTypedExpressionKind::Conditional {
                condition: Box::new(condition),
                then_expression: Box::new(then_expression),
                else_expression: Box::new(else_expression),
            },
            ty,
            category: ValueCategory::Value,
            place: None,
            constant,
            span,
        })
    }

    fn analyze_subscript(
        &mut self,
        base: &syntax::Expression,
        index: &syntax::Expression,
        span: Span,
    ) -> AnalysisResult<FullTypedExpression> {
        let base = self.analyze_expression(base)?;
        let mut base = self.value_conversion(base)?;
        let index = self.analyze_expression(index)?;
        let mut index = self.value_conversion(index)?;
        if self.pointer_pointee(base.ty.ty).is_none() && self.pointer_pointee(index.ty.ty).is_some()
        {
            std::mem::swap(&mut base, &mut index);
        }
        let Some(element) = self.pointer_pointee(base.ty.ty) else {
            return self.fail("CCC2290", span, "subscript requires a pointer operand");
        };
        if !self.types.is_integer(index.ty.ty) {
            return self.fail("CCC2291", span, "an array subscript must have integer type");
        }
        let index = self.integer_promote(index)?;
        let mut place = self.object_place(PlaceBase::Indirect, element, true);
        place.projections.push(PlaceProjection::Index);
        Ok(FullTypedExpression {
            kind: FullTypedExpressionKind::Subscript {
                base: Box::new(base),
                index: Box::new(index),
            },
            ty: element,
            category: ValueCategory::Lvalue,
            place: Some(place),
            constant: None,
            span,
        })
    }

    fn analyze_member(
        &mut self,
        base: &syntax::Expression,
        member: &syntax::Identifier,
        indirect: bool,
        span: Span,
    ) -> AnalysisResult<FullTypedExpression> {
        let base = self.analyze_expression(base)?;
        let (base, record_ty, category, mut place) = if indirect {
            let base = self.value_conversion(base)?;
            let Some(pointee) = self.pointer_pointee(base.ty.ty) else {
                return self.fail("CCC2292", span, "`->` requires a pointer to a record");
            };
            (
                base,
                pointee,
                ValueCategory::Lvalue,
                Some(self.object_place(PlaceBase::Indirect, pointee, true)),
            )
        } else {
            let category = base.category;
            let place = base.place.clone();
            let ty = base.ty;
            (base, ty, category, place)
        };
        let Some(TypeKind::Record(record_id)) = self.types.try_kind(record_ty.ty).cloned() else {
            return self.fail("CCC2293", span, "member access requires a record type");
        };
        let Some(record) = self.types.record(record_id).cloned() else {
            return self.fail("CCC2294", span, "member access uses an unknown record type");
        };
        let Some(fields) = record.fields else {
            return self.fail(
                "CCC2295",
                span,
                "member access uses an incomplete record type",
            );
        };
        let Some((field_index, field)) = fields
            .iter()
            .enumerate()
            .find(|(_, field)| field.name.as_deref() == Some(member.name.as_str()))
        else {
            return self.fail(
                "CCC2296",
                member.span,
                format!("record has no member named `{}`", member.name),
            );
        };
        let result_ty = QualifiedType::new(field.ty.ty, field.ty.qualifiers | record_ty.qualifiers);
        let mut bitfield = None;
        if let Some(place) = &mut place {
            place.projections.push(PlaceProjection::Field {
                index: field_index,
                name: field.name.clone(),
            });
            place.access = access_semantics(result_ty);
            place.modifiable = self.is_modifiable_type(result_ty);
            if field.bitfield.is_some() {
                let layout = self
                    .types
                    .layout_of(record_ty.ty, self.config)
                    .map_err(|error| {
                        self.emit("CCC2297", span, error.to_string());
                    })?;
                let LayoutShape::Record(layout) = layout.shape else {
                    unreachable!("the queried type is a record")
                };
                let field_layout = &layout.fields[field_index];
                let shared = field_layout
                    .bitfield
                    .expect("a semantic bitfield has a bitfield layout");
                let descriptor = BitfieldPlace {
                    field_index,
                    storage_offset: shared.storage_offset,
                    storage_size: shared.storage_size,
                    storage_align: shared.storage_align,
                    bit_offset: shared.bit_offset,
                    width: shared.width,
                    signed: self.is_signed_integer(field.ty.ty),
                    access: place.access,
                };
                place.bitfield = Some(descriptor);
                place.addressable = false;
                bitfield = Some(descriptor);
            }
        }
        let _ = bitfield;
        Ok(FullTypedExpression {
            kind: FullTypedExpressionKind::Member {
                base: Box::new(base),
                field_index,
                name: member.name.clone(),
                indirect,
            },
            ty: result_ty,
            category,
            place,
            constant: None,
            span,
        })
    }

    fn analyze_call(
        &mut self,
        callee: &syntax::Expression,
        arguments: &[syntax::Expression],
        span: Span,
    ) -> AnalysisResult<FullTypedExpression> {
        let callee = self.analyze_expression(callee)?;
        let callee = self.value_conversion(callee)?;
        let function_id = direct_function_reference(&callee);
        let function_ty = if self.types.function_signature(callee.ty.ty).is_some() {
            callee.ty.ty
        } else if let Some(pointee) = self.pointer_pointee(callee.ty.ty) {
            pointee.ty
        } else {
            return self.fail(
                "CCC2298",
                span,
                "called expression does not have function type",
            );
        };
        let Some(signature) = self.types.function_signature(function_ty) else {
            return self.fail(
                "CCC2299",
                span,
                "called expression does not point to a function",
            );
        };
        self.reject_long_double_operation(&[signature.result], span)?;
        let mut typed_arguments = Vec::new();
        let fixed = match &signature.parameters {
            FunctionParameters::Unspecified => 0,
            FunctionParameters::Prototype(parameters) => parameters.len(),
        };
        if let FunctionParameters::Prototype(parameters) = &signature.parameters
            && (arguments.len() < parameters.len()
                || (!signature.variadic && arguments.len() != parameters.len()))
        {
            return self.fail(
                "CCC2300",
                span,
                format!(
                    "function expects {} argument(s), but {} were supplied",
                    parameters.len(),
                    arguments.len()
                ),
            );
        }
        for (index, argument) in arguments.iter().enumerate() {
            let argument = self.analyze_expression(argument)?;
            let argument_span = argument.span;
            let converted = match &signature.parameters {
                FunctionParameters::Prototype(parameters) if index < parameters.len() => {
                    self.assignment_conversion(argument, parameters[index], argument_span)?
                }
                _ => self.default_argument_promotion(argument)?,
            };
            self.reject_long_double_operation(&[converted.ty], converted.span)?;
            typed_arguments.push(converted);
        }
        Ok(FullTypedExpression {
            kind: FullTypedExpressionKind::Call {
                callee: Box::new(callee),
                function: function_id,
                arguments: typed_arguments,
                variadic_boundary: fixed,
            },
            ty: QualifiedType::unqualified(signature.result.ty),
            category: ValueCategory::Value,
            place: None,
            constant: None,
            span,
        })
    }

    fn analyze_sizeof(
        &mut self,
        operand_ty: QualifiedType,
        span: Span,
    ) -> AnalysisResult<FullTypedExpression> {
        let layout = self
            .types
            .layout_of(operand_ty.ty, self.config)
            .map_err(|error| {
                self.emit("CCC2301", span, error.to_string());
            })?;
        let result_ty = QualifiedType::unqualified(self.size_type());
        Ok(FullTypedExpression {
            kind: FullTypedExpressionKind::Sizeof {
                operand_ty,
                size: layout.size,
            },
            ty: result_ty,
            category: ValueCategory::Value,
            place: None,
            constant: Some(ConstantValue::Unsigned(u128::from(layout.size))),
            span,
        })
    }

    fn analyze_offsetof(
        &mut self,
        type_name: &syntax::TypeName,
        designators: &[syntax::OffsetDesignator],
        span: Span,
    ) -> AnalysisResult<FullTypedExpression> {
        if self
            .config
            .capabilities
            .state(CapabilityKind::Builtin, "__builtin_offsetof")
            != CapabilityState::Implemented
        {
            return self.fail(
                "CCC2369",
                span,
                "`__builtin_offsetof` requires implemented target-layout semantics",
            );
        }
        let record_ty = self.resolve_type_name(type_name)?;
        let mut current = record_ty;
        let mut offset = 0_u64;
        let mut path = Vec::new();
        for designator in designators {
            match designator {
                syntax::OffsetDesignator::Member(member) => {
                    let Some(TypeKind::Record(record_id)) =
                        self.types.try_kind(current.ty).cloned()
                    else {
                        return self.fail(
                            "CCC2302",
                            member.span,
                            "a member designator requires a record type",
                        );
                    };
                    let record = self
                        .types
                        .record(record_id)
                        .and_then(|record| record.fields.as_ref())
                        .cloned()
                        .ok_or_else(|| {
                            self.emit("CCC2303", member.span, "offsetof uses an incomplete record");
                        })?;
                    let Some((index, field)) = record
                        .iter()
                        .enumerate()
                        .find(|(_, field)| field.name.as_deref() == Some(member.name.as_str()))
                    else {
                        return self.fail(
                            "CCC2304",
                            member.span,
                            format!("record has no member named `{}`", member.name),
                        );
                    };
                    if field.bitfield.is_some() {
                        return self.fail(
                            "CCC2305",
                            member.span,
                            "offsetof cannot designate a bitfield",
                        );
                    }
                    let layout =
                        self.types
                            .layout_of(current.ty, self.config)
                            .map_err(|error| {
                                self.emit("CCC2306", member.span, error.to_string());
                            })?;
                    let LayoutShape::Record(layout) = layout.shape else {
                        unreachable!("the queried type is a record")
                    };
                    offset = offset
                        .checked_add(layout.fields[index].offset)
                        .ok_or_else(|| {
                            self.emit("CCC2307", span, "offsetof result overflows");
                        })?;
                    current = field.ty;
                    path.push(ResolvedOffsetDesignator::Field {
                        index,
                        name: member.name.clone(),
                    });
                }
                syntax::OffsetDesignator::Index(index) => {
                    let value = self.evaluate_integer_constant(index)?;
                    let value = u64::try_from(value).map_err(|_| {
                        self.emit(
                            "CCC2308",
                            index.span,
                            "offsetof array index must be nonnegative",
                        );
                    })?;
                    let Some(TypeKind::Array(array)) = self.types.try_kind(current.ty).cloned()
                    else {
                        return self.fail(
                            "CCC2309",
                            index.span,
                            "an index designator requires array type",
                        );
                    };
                    let element_layout = self
                        .types
                        .layout_of(array.element.ty, self.config)
                        .map_err(|error| {
                            self.emit("CCC2310", index.span, error.to_string());
                        })?;
                    offset = offset
                        .checked_add(value.checked_mul(element_layout.size).ok_or_else(|| {
                            self.emit("CCC2311", index.span, "offsetof index overflows");
                        })?)
                        .ok_or_else(|| {
                            self.emit("CCC2311", index.span, "offsetof result overflows");
                        })?;
                    current = array.element;
                    path.push(ResolvedOffsetDesignator::Index { value });
                }
            }
        }
        let result_ty = QualifiedType::unqualified(self.size_type());
        Ok(FullTypedExpression {
            kind: FullTypedExpressionKind::Offsetof {
                record_ty,
                path,
                offset,
            },
            ty: result_ty,
            category: ValueCategory::Value,
            place: None,
            constant: Some(ConstantValue::Unsigned(u128::from(offset))),
            span,
        })
    }

    fn analyze_initializer(
        &mut self,
        ty: QualifiedType,
        initializer: &syntax::Initializer,
    ) -> AnalysisResult<(FullTypedInitializer, QualifiedType)> {
        match initializer {
            syntax::Initializer::Expression(expression) => {
                let expression = self.analyze_expression(expression)?;
                if let Some(TypeKind::Array(array)) = self.types.try_kind(ty.ty).cloned()
                    && let FullTypedExpressionKind::StringLiteral(string) = expression.kind
                {
                    let literal_ty = self.strings[string.0 as usize].ty;
                    let Some(TypeKind::Array(literal_array)) =
                        self.types.try_kind(literal_ty.ty).cloned()
                    else {
                        unreachable!("a string literal has array type")
                    };
                    if !self.type_ids_compatible(array.element.ty, literal_array.element.ty) {
                        return self.fail(
                            "CCC2312",
                            expression.span,
                            "string literal element type is incompatible with the array",
                        );
                    }
                    let literal_length = match literal_array.length {
                        ArrayLength::Constant(length) => length,
                        _ => unreachable!("a string literal has constant length"),
                    };
                    let completed = match array.length {
                        ArrayLength::Incomplete => {
                            QualifiedType::unqualified(self.types.array(ArrayType {
                                element: array.element,
                                length: ArrayLength::Constant(literal_length),
                            }))
                        }
                        ArrayLength::Constant(length) if length + 1 < literal_length => {
                            return self.fail(
                                "CCC2313",
                                expression.span,
                                "string literal is too long for the initialized array",
                            );
                        }
                        ArrayLength::Variable(_) => {
                            return self.fail(
                                "CCC2314",
                                expression.span,
                                "a variable-length array cannot be initialized",
                            );
                        }
                        _ => ty,
                    };
                    return Ok((
                        FullTypedInitializer {
                            ty: completed,
                            kind: FullTypedInitializerKind::String(string),
                            span: expression.span,
                        },
                        completed,
                    ));
                }
                let expression_span = expression.span;
                let converted = self.assignment_conversion(expression, ty, expression_span)?;
                Ok((
                    FullTypedInitializer {
                        ty,
                        span: converted.span,
                        kind: FullTypedInitializerKind::Scalar(converted),
                    },
                    ty,
                ))
            }
            syntax::Initializer::List { entries, span } => {
                if entries.is_empty() {
                    return Ok((
                        FullTypedInitializer {
                            ty,
                            kind: FullTypedInitializerKind::Zero,
                            span: *span,
                        },
                        ty,
                    ));
                }
                match self.types.try_kind(ty.ty).cloned() {
                    Some(TypeKind::Array(array)) => {
                        self.analyze_array_initializer(ty, array, entries, *span)
                    }
                    Some(TypeKind::Record(record)) => {
                        self.analyze_record_initializer(ty, record, entries, *span)
                    }
                    _ if entries.len() == 1 && entries[0].designation.is_empty() => {
                        self.analyze_initializer(ty, &entries[0].initializer)
                    }
                    _ => self.fail(
                        "CCC2315",
                        *span,
                        "a scalar initializer list must contain exactly one undesignated value",
                    ),
                }
            }
        }
    }

    fn analyze_array_initializer(
        &mut self,
        ty: QualifiedType,
        array: ArrayType,
        entries: &[syntax::InitializerEntry],
        span: Span,
    ) -> AnalysisResult<(FullTypedInitializer, QualifiedType)> {
        if matches!(array.length, ArrayLength::Variable(_)) {
            return self.fail(
                "CCC2316",
                span,
                "a variable-length array cannot be initialized",
            );
        }
        let mut cursor = 0_u64;
        let mut maximum = None::<u64>;
        let mut typed_entries = Vec::new();
        for entry in entries {
            let (path, target_ty, next_cursor) = if entry.designation.is_empty() {
                (
                    vec![InitializerPathElement::Index(cursor)],
                    array.element,
                    cursor.checked_add(1).ok_or_else(|| {
                        self.emit("CCC2317", entry.span, "initializer index overflows");
                    })?,
                )
            } else {
                let (target_ty, path) =
                    self.resolve_initializer_designation(ty, &entry.designation)?;
                let Some(InitializerPathElement::Index(index)) = path.first() else {
                    return self.fail(
                        "CCC2318",
                        entry.span,
                        "an array designation must begin with an index",
                    );
                };
                let index = *index;
                (
                    path,
                    target_ty,
                    index.checked_add(1).ok_or_else(|| {
                        self.emit("CCC2317", entry.span, "initializer index overflows");
                    })?,
                )
            };
            cursor = next_cursor;
            let first_index = match path.first() {
                Some(InitializerPathElement::Index(index)) => *index,
                _ => unreachable!("array paths begin with an index"),
            };
            if let ArrayLength::Constant(length) = array.length
                && first_index >= length
            {
                return self.fail(
                    "CCC2319",
                    entry.span,
                    "array initializer index is outside the declared bound",
                );
            }
            maximum = Some(maximum.map_or(first_index, |old| old.max(first_index)));
            let (initializer, _) = self.analyze_initializer(target_ty, &entry.initializer)?;
            typed_entries.push(FullTypedInitializerEntry {
                path,
                initializer: Box::new(initializer),
            });
        }
        let completed = match array.length {
            ArrayLength::Incomplete => {
                let length = maximum.map_or(0, |maximum| maximum + 1);
                QualifiedType::unqualified(self.types.array(ArrayType {
                    element: array.element,
                    length: ArrayLength::Constant(length),
                }))
            }
            _ => ty,
        };
        Ok((
            FullTypedInitializer {
                ty: completed,
                kind: FullTypedInitializerKind::Aggregate(typed_entries),
                span,
            },
            completed,
        ))
    }

    fn analyze_record_initializer(
        &mut self,
        ty: QualifiedType,
        record_id: ccc_types::RecordId,
        entries: &[syntax::InitializerEntry],
        span: Span,
    ) -> AnalysisResult<(FullTypedInitializer, QualifiedType)> {
        let record = self
            .types
            .record(record_id)
            .cloned()
            .ok_or_else(|| self.emit("CCC2320", span, "unknown record initializer type"))?;
        let fields = record.fields.ok_or_else(|| {
            self.emit(
                "CCC2321",
                span,
                "an incomplete record cannot be initialized",
            )
        })?;
        let mut cursor = 0_usize;
        let mut initialized_union_member = None::<usize>;
        let mut typed_entries = Vec::new();
        for entry in entries {
            let (path, target_ty, selected_field) = if entry.designation.is_empty() {
                let Some(field) = fields.get(cursor) else {
                    return self.fail("CCC2322", entry.span, "too many record initializers");
                };
                let index = cursor;
                cursor += 1;
                (
                    vec![InitializerPathElement::Field {
                        index,
                        name: field.name.clone(),
                        bitfield: self
                            .initializer_bitfield(ty, record_id, index, field, entry.span)?,
                    }],
                    field.ty,
                    index,
                )
            } else {
                let (target_ty, path) =
                    self.resolve_initializer_designation(ty, &entry.designation)?;
                let Some(InitializerPathElement::Field { index, .. }) = path.first() else {
                    return self.fail(
                        "CCC2323",
                        entry.span,
                        "a record designation must begin with a member name",
                    );
                };
                let index = *index;
                cursor = index + 1;
                (path, target_ty, index)
            };
            if record.kind == RecordKind::Union {
                if initialized_union_member.is_some_and(|previous| previous != selected_field) {
                    return self.fail(
                        "CCC2324",
                        entry.span,
                        "a union initializer selects more than one member",
                    );
                }
                initialized_union_member = Some(selected_field);
            }
            let (initializer, _) = self.analyze_initializer(target_ty, &entry.initializer)?;
            typed_entries.push(FullTypedInitializerEntry {
                path,
                initializer: Box::new(initializer),
            });
        }
        Ok((
            FullTypedInitializer {
                ty,
                kind: FullTypedInitializerKind::Aggregate(typed_entries),
                span,
            },
            ty,
        ))
    }

    fn resolve_initializer_designation(
        &mut self,
        mut ty: QualifiedType,
        designators: &[syntax::Designator],
    ) -> AnalysisResult<(QualifiedType, Vec<InitializerPathElement>)> {
        let mut path = Vec::new();
        for designator in designators {
            match designator {
                syntax::Designator::Index(expression) => {
                    let value = self.evaluate_integer_constant(expression)?;
                    let value = u64::try_from(value).map_err(|_| {
                        self.emit(
                            "CCC2325",
                            expression.span,
                            "initializer index must be nonnegative",
                        );
                    })?;
                    let Some(TypeKind::Array(array)) = self.types.try_kind(ty.ty).cloned() else {
                        return self.fail(
                            "CCC2326",
                            expression.span,
                            "an index designator requires array type",
                        );
                    };
                    if let ArrayLength::Constant(length) = array.length
                        && value >= length
                    {
                        return self.fail(
                            "CCC2327",
                            expression.span,
                            "initializer index is outside the array bound",
                        );
                    }
                    path.push(InitializerPathElement::Index(value));
                    ty = array.element;
                }
                syntax::Designator::Member(member) => {
                    let Some(TypeKind::Record(record_id)) = self.types.try_kind(ty.ty).cloned()
                    else {
                        return self.fail(
                            "CCC2328",
                            member.span,
                            "a member designator requires record type",
                        );
                    };
                    let fields = self
                        .types
                        .record(record_id)
                        .and_then(|record| record.fields.as_ref())
                        .cloned()
                        .ok_or_else(|| {
                            self.emit(
                                "CCC2329",
                                member.span,
                                "member designator uses an incomplete record",
                            );
                        })?;
                    let Some((index, field)) = fields
                        .iter()
                        .enumerate()
                        .find(|(_, field)| field.name.as_deref() == Some(member.name.as_str()))
                    else {
                        return self.fail(
                            "CCC2330",
                            member.span,
                            format!("record has no member named `{}`", member.name),
                        );
                    };
                    path.push(InitializerPathElement::Field {
                        index,
                        name: field.name.clone(),
                        bitfield: self.initializer_bitfield(
                            ty,
                            record_id,
                            index,
                            field,
                            member.span,
                        )?,
                    });
                    ty = field.ty;
                }
            }
        }
        Ok((ty, path))
    }

    fn initializer_bitfield(
        &mut self,
        record_ty: QualifiedType,
        record_id: ccc_types::RecordId,
        field_index: usize,
        field: &Field,
        span: Span,
    ) -> AnalysisResult<Option<BitfieldPlace>> {
        if field.bitfield.is_none() {
            return Ok(None);
        }
        let layout = self
            .types
            .layout_of(record_ty.ty, self.config)
            .map_err(|error| {
                self.emit("CCC2368", span, error.to_string());
            })?;
        let LayoutShape::Record(layout) = layout.shape else {
            unreachable!("a bitfield belongs to a record")
        };
        debug_assert_eq!(layout.id, record_id);
        let shared = layout.fields[field_index]
            .bitfield
            .expect("a semantic bitfield has a shared layout descriptor");
        let field_ty = QualifiedType::new(field.ty.ty, field.ty.qualifiers | record_ty.qualifiers);
        let access = access_semantics(field_ty);
        Ok(Some(BitfieldPlace {
            field_index,
            storage_offset: shared.storage_offset,
            storage_size: shared.storage_size,
            storage_align: shared.storage_align,
            bit_offset: shared.bit_offset,
            width: shared.width,
            signed: self.is_signed_integer(field.ty.ty),
            access,
        }))
    }

    fn value_conversion(
        &mut self,
        expression: FullTypedExpression,
    ) -> AnalysisResult<FullTypedExpression> {
        match self.types.try_kind(expression.ty.ty).cloned() {
            Some(TypeKind::Array(array)) => {
                let target = QualifiedType::unqualified(self.types.pointer(array.element));
                Ok(self.conversion(ConversionKind::ArrayToPointer, expression, target, None))
            }
            Some(TypeKind::Function(_)) => {
                let target = QualifiedType::unqualified(self.types.pointer(expression.ty));
                Ok(self.conversion(ConversionKind::FunctionToPointer, expression, target, None))
            }
            _ if expression.category == ValueCategory::Lvalue => {
                let access = expression
                    .place
                    .as_ref()
                    .map_or_else(|| access_semantics(expression.ty), |place| place.access);
                let target = QualifiedType::unqualified(expression.ty.ty);
                Ok(self.conversion(
                    ConversionKind::LvalueToValue { access },
                    expression,
                    target,
                    None,
                ))
            }
            _ => Ok(expression),
        }
    }

    fn analyze_condition(
        &mut self,
        expression: &syntax::Expression,
    ) -> AnalysisResult<FullTypedExpression> {
        let expression = self.analyze_expression(expression)?;
        let expression = self.value_conversion(expression)?;
        self.convert_to_boolean(expression)
    }

    fn convert_to_boolean(
        &mut self,
        expression: FullTypedExpression,
    ) -> AnalysisResult<FullTypedExpression> {
        if !self.types.is_arithmetic(expression.ty.ty)
            && self.pointer_pointee(expression.ty.ty).is_none()
        {
            return self.fail(
                "CCC2331",
                expression.span,
                "a scalar expression is required",
            );
        }
        let constant = expression
            .constant
            .map(|value| ConstantValue::Signed(i128::from(!value.is_zero())));
        Ok(self.conversion(
            ConversionKind::ToBoolean,
            expression,
            QualifiedType::unqualified(TypeId::INT),
            constant,
        ))
    }

    fn integer_promote(
        &mut self,
        expression: FullTypedExpression,
    ) -> AnalysisResult<FullTypedExpression> {
        if !self.types.is_integer(expression.ty.ty) {
            return self.fail(
                "CCC2332",
                expression.span,
                "integer promotion requires integer type",
            );
        }
        let Some(kind) = self.integer_representation(expression.ty.ty) else {
            if matches!(
                self.types.try_kind(expression.ty.ty),
                Some(TypeKind::Enum(_))
            ) && let Some(value) = expression.constant.and_then(ConstantValue::as_i128)
                && let Some(target) = self.integer_type_for_range(value, value)
            {
                let constant = expression
                    .constant
                    .and_then(|constant| self.convert_constant(constant, target));
                return Ok(self.conversion(
                    ConversionKind::IntegerPromotion,
                    expression,
                    QualifiedType::unqualified(target),
                    constant,
                ));
            }
            return self.fail(
                "CCC2332",
                expression.span,
                "integer promotion requires a complete integer type",
            );
        };
        let target = self.promoted_integer_type(expression.ty.ty);
        if self.types.builtin_type(expression.ty.ty).is_some()
            && integer_rank(kind) >= integer_rank(BuiltinType::Int)
            && target == expression.ty.ty
        {
            return Ok(expression);
        }
        let constant = expression
            .constant
            .and_then(|value| self.convert_constant(value, target));
        Ok(self.conversion(
            ConversionKind::IntegerPromotion,
            expression,
            QualifiedType::unqualified(target),
            constant,
        ))
    }

    fn integer_or_arithmetic_promotion(
        &mut self,
        expression: FullTypedExpression,
        arithmetic_allowed: bool,
        span: Span,
    ) -> AnalysisResult<FullTypedExpression> {
        if self.types.is_integer(expression.ty.ty) {
            self.integer_promote(expression)
        } else if arithmetic_allowed && self.types.is_arithmetic(expression.ty.ty) {
            Ok(expression)
        } else {
            self.fail("CCC2333", span, "operator has an invalid operand type")
        }
    }

    fn usual_arithmetic_conversions(
        &mut self,
        left: FullTypedExpression,
        right: FullTypedExpression,
        span: Span,
    ) -> AnalysisResult<(FullTypedExpression, FullTypedExpression, QualifiedType)> {
        if !self.types.is_arithmetic(left.ty.ty) || !self.types.is_arithmetic(right.ty.ty) {
            return self.fail("CCC2334", span, "arithmetic operands are required");
        }
        let left = if self.types.is_integer(left.ty.ty) {
            self.integer_promote(left)?
        } else {
            left
        };
        let right = if self.types.is_integer(right.ty.ty) {
            self.integer_promote(right)?
        } else {
            right
        };
        let target = if left.ty.ty == TypeId::LONG_DOUBLE || right.ty.ty == TypeId::LONG_DOUBLE {
            TypeId::LONG_DOUBLE
        } else if left.ty.ty == TypeId::DOUBLE || right.ty.ty == TypeId::DOUBLE {
            TypeId::DOUBLE
        } else if left.ty.ty == TypeId::FLOAT || right.ty.ty == TypeId::FLOAT {
            TypeId::FLOAT
        } else {
            self.common_integer_type(left.ty.ty, right.ty.ty)
        };
        let ty = QualifiedType::unqualified(target);
        let left = self.arithmetic_conversion(left, ty)?;
        let right = self.arithmetic_conversion(right, ty)?;
        Ok((left, right, ty))
    }

    fn arithmetic_conversion(
        &mut self,
        expression: FullTypedExpression,
        target: QualifiedType,
    ) -> AnalysisResult<FullTypedExpression> {
        if expression.ty.ty == target.ty {
            return Ok(expression);
        }
        let source_integer = self.types.is_integer(expression.ty.ty);
        let target_integer = self.types.is_integer(target.ty);
        let kind = match (source_integer, target_integer) {
            (true, true) => ConversionKind::IntegerConversion,
            (true, false) => ConversionKind::IntegerToFloating,
            (false, true) => ConversionKind::FloatingToInteger,
            (false, false) => ConversionKind::FloatingConversion,
        };
        let constant = expression
            .constant
            .and_then(|constant| self.convert_constant(constant, target.ty));
        Ok(self.conversion(kind, expression, target, constant))
    }

    fn assignment_conversion(
        &mut self,
        expression: FullTypedExpression,
        target: QualifiedType,
        span: Span,
    ) -> AnalysisResult<FullTypedExpression> {
        let expression = self.value_conversion(expression)?;
        if target.ty == TypeId::BOOL
            && (self.types.is_arithmetic(expression.ty.ty)
                || self.pointer_pointee(expression.ty.ty).is_some())
        {
            let constant = expression
                .constant
                .and_then(|value| self.convert_constant(value, target.ty));
            return Ok(self.conversion(
                ConversionKind::ToBoolean,
                expression,
                QualifiedType::unqualified(target.ty),
                constant,
            ));
        }
        if self.types.is_arithmetic(target.ty) && self.types.is_arithmetic(expression.ty.ty) {
            self.reject_long_double_operation(&[target, expression.ty], span)?;
            return self.arithmetic_conversion(expression, QualifiedType::unqualified(target.ty));
        }
        if self.pointer_pointee(target.ty).is_some() {
            if expression.constant.is_some_and(ConstantValue::is_zero)
                && self.types.is_integer(expression.ty.ty)
            {
                return Ok(self.conversion(
                    ConversionKind::PointerConversion,
                    expression,
                    QualifiedType::unqualified(target.ty),
                    Some(ConstantValue::NullPointer),
                ));
            }
            if self.pointer_pointee(expression.ty.ty).is_some()
                && self.pointers_assignment_compatible(target.ty, expression.ty.ty)
            {
                let constant = expression.constant;
                return Ok(self.conversion(
                    ConversionKind::PointerConversion,
                    expression,
                    QualifiedType::unqualified(target.ty),
                    constant,
                ));
            }
            return self.fail(
                "CCC2335",
                span,
                "pointer assignment uses incompatible types",
            );
        }
        if self.types_compatible(target, expression.ty) {
            return Ok(expression);
        }
        self.fail(
            "CCC2336",
            span,
            "initializer or assignment has incompatible type",
        )
    }

    fn explicit_conversion(
        &mut self,
        expression: FullTypedExpression,
        target: QualifiedType,
        span: Span,
    ) -> AnalysisResult<FullTypedExpression> {
        if target.ty == TypeId::VOID {
            return Ok(self.conversion(
                ConversionKind::ToVoid,
                expression,
                QualifiedType::unqualified(TypeId::VOID),
                None,
            ));
        }
        self.reject_long_double_operation(&[target, expression.ty], span)?;
        if target.ty == TypeId::BOOL
            && (self.types.is_arithmetic(expression.ty.ty)
                || self.pointer_pointee(expression.ty.ty).is_some())
        {
            let constant = expression
                .constant
                .and_then(|value| self.convert_constant(value, target.ty));
            return Ok(self.conversion(
                ConversionKind::ToBoolean,
                expression,
                QualifiedType::unqualified(target.ty),
                constant,
            ));
        }
        if self.types.is_arithmetic(target.ty) && self.types.is_arithmetic(expression.ty.ty) {
            return self.arithmetic_conversion(expression, QualifiedType::unqualified(target.ty));
        }
        let target_pointer = self.pointer_pointee(target.ty).is_some();
        let source_pointer = self.pointer_pointee(expression.ty.ty).is_some();
        if target_pointer && (source_pointer || self.types.is_integer(expression.ty.ty))
            || (source_pointer && self.types.is_integer(target.ty))
        {
            let constant =
                if target_pointer && expression.constant.is_some_and(ConstantValue::is_zero) {
                    Some(ConstantValue::NullPointer)
                } else {
                    expression.constant
                };
            return Ok(self.conversion(
                ConversionKind::PointerConversion,
                expression,
                QualifiedType::unqualified(target.ty),
                constant,
            ));
        }
        self.fail("CCC2337", span, "invalid cast between these types")
    }

    fn default_argument_promotion(
        &mut self,
        expression: FullTypedExpression,
    ) -> AnalysisResult<FullTypedExpression> {
        let expression = self.value_conversion(expression)?;
        if self.types.is_integer(expression.ty.ty) {
            return self.integer_promote(expression);
        }
        if expression.ty.ty == TypeId::FLOAT {
            return self
                .arithmetic_conversion(expression, QualifiedType::unqualified(TypeId::DOUBLE));
        }
        Ok(expression)
    }

    fn conversion(
        &self,
        kind: ConversionKind,
        expression: FullTypedExpression,
        target: QualifiedType,
        constant: Option<ConstantValue>,
    ) -> FullTypedExpression {
        let span = expression.span;
        let inherited = expression.constant;
        FullTypedExpression {
            kind: FullTypedExpressionKind::Conversion {
                kind,
                expression: Box::new(expression),
            },
            ty: target,
            category: ValueCategory::Value,
            place: None,
            constant: constant.or_else(|| {
                matches!(
                    kind,
                    ConversionKind::ArrayToPointer | ConversionKind::FunctionToPointer
                )
                .then_some(inherited)
                .flatten()
            }),
            span,
        }
    }

    fn constant_expression(
        &self,
        constant: ConstantValue,
        ty: QualifiedType,
        span: Span,
    ) -> FullTypedExpression {
        FullTypedExpression {
            kind: FullTypedExpressionKind::Constant(constant),
            ty,
            category: ValueCategory::Value,
            place: None,
            constant: Some(constant),
            span,
        }
    }

    fn evaluate_integer_constant(
        &mut self,
        expression: &syntax::Expression,
    ) -> AnalysisResult<i128> {
        let Some(value) = self.try_evaluate_integer_constant(expression)? else {
            return self.fail(
                "CCC2338",
                expression.span,
                "an integer constant expression is required",
            );
        };
        Ok(value)
    }

    fn try_evaluate_integer_constant(
        &mut self,
        expression: &syntax::Expression,
    ) -> AnalysisResult<Option<i128>> {
        let typed = self.analyze_expression(expression)?;
        let typed = self.value_conversion(typed)?;
        if !self.types.is_integer(typed.ty.ty) {
            return self.fail(
                "CCC2339",
                expression.span,
                "constant expression must have integer type",
            );
        }
        Ok(typed.constant.and_then(ConstantValue::as_i128))
    }

    fn convert_pointer_comparison(
        &mut self,
        left: FullTypedExpression,
        right: FullTypedExpression,
        span: Span,
    ) -> AnalysisResult<(FullTypedExpression, FullTypedExpression)> {
        if self.pointer_pointee(left.ty.ty).is_some()
            && right.constant.is_some_and(ConstantValue::is_zero)
            && self.types.is_integer(right.ty.ty)
        {
            let target = left.ty;
            let right = self.conversion(
                ConversionKind::PointerConversion,
                right,
                target,
                Some(ConstantValue::NullPointer),
            );
            return Ok((left, right));
        }
        if self.pointer_pointee(right.ty.ty).is_some()
            && left.constant.is_some_and(ConstantValue::is_zero)
            && self.types.is_integer(left.ty.ty)
        {
            let target = right.ty;
            let left = self.conversion(
                ConversionKind::PointerConversion,
                left,
                target,
                Some(ConstantValue::NullPointer),
            );
            return Ok((left, right));
        }
        if self.pointer_pointee(left.ty.ty).is_some()
            && self.pointer_pointee(right.ty.ty).is_some()
            && self.pointers_assignment_compatible(left.ty.ty, right.ty.ty)
        {
            let target = left.ty;
            let constant = right.constant;
            let right = if right.ty.ty == target.ty {
                right
            } else {
                self.conversion(ConversionKind::PointerConversion, right, target, constant)
            };
            return Ok((left, right));
        }
        self.fail(
            "CCC2340",
            span,
            "comparison uses incompatible pointer types",
        )
    }

    fn evaluate_pointer_arithmetic(
        &self,
        pointer: Option<ConstantValue>,
        index: Option<ConstantValue>,
        pointer_ty: QualifiedType,
        subtract: bool,
    ) -> Option<ConstantValue> {
        let ConstantValue::Address(mut address) = pointer? else {
            return None;
        };
        let index = index?.as_i128()?;
        let pointee = self.pointer_pointee(pointer_ty.ty)?;
        let size = i128::from(self.types.layout_of(pointee.ty, self.config).ok()?.size);
        let delta = index.checked_mul(size)?;
        address.addend = if subtract {
            address.addend.checked_sub(delta)?
        } else {
            address.addend.checked_add(delta)?
        };
        Some(ConstantValue::Address(address))
    }

    fn common_integer_type(&self, left: TypeId, right: TypeId) -> TypeId {
        let left = self
            .integer_representation(left)
            .unwrap_or(BuiltinType::Int);
        let right = self
            .integer_representation(right)
            .unwrap_or(BuiltinType::Int);
        if left == right {
            return self.types.builtin(left);
        }
        let left_signed = self.integer_kind_is_signed(left);
        let right_signed = self.integer_kind_is_signed(right);
        let left_rank = integer_rank(left);
        let right_rank = integer_rank(right);
        if left_signed == right_signed {
            return self
                .types
                .builtin(if left_rank >= right_rank { left } else { right });
        }
        let (signed, unsigned) = if left_signed {
            (left, right)
        } else {
            (right, left)
        };
        if integer_rank(unsigned) >= integer_rank(signed) {
            return self.types.builtin(unsigned);
        }
        if self.integer_width(signed) > self.integer_width(unsigned) {
            return self.types.builtin(signed);
        }
        self.types.builtin(unsigned_counterpart(signed))
    }

    fn promoted_integer_type(&self, ty: TypeId) -> TypeId {
        let Some(kind) = self.integer_representation(ty) else {
            return TypeId::INT;
        };
        if integer_rank(kind) >= integer_rank(BuiltinType::Int) {
            return self.types.builtin(kind);
        }
        let signed = match kind {
            BuiltinType::Char => self.config.target.data_layout.char_is_signed,
            _ => self.integer_kind_is_signed(kind),
        };
        if signed || self.integer_width(kind) < self.integer_width(BuiltinType::Int) {
            TypeId::INT
        } else {
            TypeId::UNSIGNED_INT
        }
    }

    fn common_arithmetic_type(&self, left: TypeId, right: TypeId) -> TypeId {
        if left == TypeId::LONG_DOUBLE || right == TypeId::LONG_DOUBLE {
            TypeId::LONG_DOUBLE
        } else if left == TypeId::DOUBLE || right == TypeId::DOUBLE {
            TypeId::DOUBLE
        } else if left == TypeId::FLOAT || right == TypeId::FLOAT {
            TypeId::FLOAT
        } else {
            self.common_integer_type(left, right)
        }
    }

    fn arithmetic_conversion_kind(&self, source: TypeId, target: TypeId) -> ConversionKind {
        match (self.types.is_integer(source), self.types.is_integer(target)) {
            (true, true) => ConversionKind::IntegerConversion,
            (true, false) => ConversionKind::IntegerToFloating,
            (false, true) => ConversionKind::FloatingToInteger,
            (false, false) => ConversionKind::FloatingConversion,
        }
    }

    fn convert_constant(&self, value: ConstantValue, target: TypeId) -> Option<ConstantValue> {
        if self.types.is_integer(target) {
            let kind = self
                .integer_representation(target)
                .unwrap_or(BuiltinType::Int);
            if kind == BuiltinType::Bool {
                return Some(ConstantValue::Unsigned(u128::from(!value.is_zero())));
            }
            let width = self.integer_width(kind);
            let raw = match value {
                ConstantValue::Signed(value) => value as u128,
                ConstantValue::Unsigned(value) => value,
                ConstantValue::Floating(value) => value as i128 as u128,
                ConstantValue::NullPointer => 0,
                ConstantValue::Address(_) => return None,
            };
            let raw = truncate_to_width(raw, width);
            if self.integer_kind_is_signed(kind) {
                Some(ConstantValue::Signed(sign_extend(raw, width)))
            } else {
                Some(ConstantValue::Unsigned(raw))
            }
        } else if matches!(
            self.types.builtin_type(target),
            Some(BuiltinType::Float | BuiltinType::Double | BuiltinType::LongDouble)
        ) {
            let value = match value {
                ConstantValue::Signed(value) => value as f64,
                ConstantValue::Unsigned(value) => value as f64,
                ConstantValue::Floating(value) => value,
                ConstantValue::NullPointer | ConstantValue::Address(_) => return None,
            };
            Some(ConstantValue::Floating(value))
        } else {
            None
        }
    }

    fn integer_fits(&self, kind: BuiltinType, value: u128) -> bool {
        let width = self.integer_width(kind);
        if self.integer_kind_is_signed(kind) {
            value <= signed_max(width)
        } else {
            value <= unsigned_max(width)
        }
    }

    fn integer_width(&self, kind: BuiltinType) -> u8 {
        let layout = &self.config.target.data_layout;
        match kind {
            BuiltinType::Bool => layout.bool_width,
            BuiltinType::Char | BuiltinType::SignedChar | BuiltinType::UnsignedChar => {
                layout.char_width
            }
            BuiltinType::Short | BuiltinType::UnsignedShort => layout.short_width,
            BuiltinType::Int | BuiltinType::UnsignedInt => layout.int_width,
            BuiltinType::Long | BuiltinType::UnsignedLong => layout.long_width,
            BuiltinType::LongLong | BuiltinType::UnsignedLongLong => layout.long_long_width,
            BuiltinType::Void
            | BuiltinType::Float
            | BuiltinType::Double
            | BuiltinType::LongDouble => 0,
        }
    }

    fn integer_representation(&self, ty: TypeId) -> Option<BuiltinType> {
        if let Some(kind) = self.types.builtin_type(ty) {
            return Some(kind);
        }
        let TypeKind::Enum(id) = self.types.try_kind(ty)? else {
            return None;
        };
        let underlying = self.types.enumeration(*id)?.body.as_ref()?.underlying;
        self.types.builtin_type(underlying)
    }

    fn enum_underlying_type(&self, enumerators: &[ccc_types::Enumerator]) -> Option<TypeId> {
        let minimum = enumerators
            .iter()
            .map(|enumerator| enumerator.value)
            .min()
            .unwrap_or(0);
        let maximum = enumerators
            .iter()
            .map(|enumerator| enumerator.value)
            .max()
            .unwrap_or(0);

        self.integer_type_for_range(minimum, maximum)
    }

    fn integer_type_for_range(&self, minimum: i128, maximum: i128) -> Option<TypeId> {
        if self.signed_range_fits(BuiltinType::Int, minimum, maximum) {
            return Some(TypeId::INT);
        }
        if minimum >= 0 {
            for candidate in [
                BuiltinType::UnsignedInt,
                BuiltinType::UnsignedLong,
                BuiltinType::UnsignedLongLong,
            ] {
                if (maximum as u128) <= unsigned_max(self.integer_width(candidate)) {
                    return Some(self.types.builtin(candidate));
                }
            }
        } else {
            for candidate in [BuiltinType::Long, BuiltinType::LongLong] {
                if self.signed_range_fits(candidate, minimum, maximum) {
                    return Some(self.types.builtin(candidate));
                }
            }
        }
        None
    }

    fn signed_range_fits(&self, kind: BuiltinType, minimum: i128, maximum: i128) -> bool {
        let width = self.integer_width(kind);
        minimum >= signed_min(width) && maximum <= signed_max(width) as i128
    }

    fn pointer_pointee(&self, ty: TypeId) -> Option<QualifiedType> {
        match self.types.try_kind(ty)? {
            TypeKind::Pointer(pointer) => Some(pointer.pointee),
            _ => None,
        }
    }

    fn pointer_types_compatible(&self, left: TypeId, right: TypeId) -> bool {
        let (Some(left), Some(right)) = (self.pointer_pointee(left), self.pointer_pointee(right))
        else {
            return false;
        };
        self.type_ids_compatible(left.ty, right.ty)
    }

    fn pointers_assignment_compatible(&self, target: TypeId, source: TypeId) -> bool {
        let (Some(target), Some(source)) =
            (self.pointer_pointee(target), self.pointer_pointee(source))
        else {
            return false;
        };
        let pointee_compatible = self.type_ids_compatible(target.ty, source.ty)
            || target.ty == TypeId::VOID
            || source.ty == TypeId::VOID;
        pointee_compatible && target.qualifiers.contains(source.qualifiers)
    }

    fn types_compatible(&self, left: QualifiedType, right: QualifiedType) -> bool {
        left.qualifiers == right.qualifiers && self.type_ids_compatible(left.ty, right.ty)
    }

    fn type_ids_compatible(&self, left: TypeId, right: TypeId) -> bool {
        if left == right {
            return true;
        }
        match (self.types.try_kind(left), self.types.try_kind(right)) {
            (Some(TypeKind::Pointer(left)), Some(TypeKind::Pointer(right))) => {
                self.types_compatible(left.pointee, right.pointee)
            }
            (Some(TypeKind::Array(left)), Some(TypeKind::Array(right))) => {
                self.types_compatible(left.element, right.element)
                    && (left.length == right.length
                        || matches!(left.length, ArrayLength::Incomplete)
                        || matches!(right.length, ArrayLength::Incomplete))
            }
            _ => match (
                self.types.function_signature(left),
                self.types.function_signature(right),
            ) {
                (Some(left), Some(right)) => {
                    self.types_compatible(left.result, right.result)
                        && left.variadic == right.variadic
                        && match (&left.parameters, &right.parameters) {
                            (FunctionParameters::Unspecified, FunctionParameters::Unspecified) => {
                                true
                            }
                            (FunctionParameters::Unspecified, FunctionParameters::Prototype(_)) => {
                                self.prototype_compatible_with_unspecified(&right)
                            }
                            (FunctionParameters::Prototype(_), FunctionParameters::Unspecified) => {
                                self.prototype_compatible_with_unspecified(&left)
                            }
                            (
                                FunctionParameters::Prototype(left),
                                FunctionParameters::Prototype(right),
                            ) => {
                                left.len() == right.len()
                                    && left
                                        .iter()
                                        .zip(right.iter())
                                        .all(|(left, right)| self.types_compatible(*left, *right))
                            }
                        }
                }
                _ => false,
            },
        }
    }

    fn composite_type(
        &mut self,
        left: QualifiedType,
        right: QualifiedType,
    ) -> Option<QualifiedType> {
        if left.qualifiers != right.qualifiers {
            return None;
        }
        Some(QualifiedType::new(
            self.composite_type_id(left.ty, right.ty)?,
            left.qualifiers,
        ))
    }

    fn composite_type_id(&mut self, left: TypeId, right: TypeId) -> Option<TypeId> {
        if left == right {
            return Some(left);
        }

        if let (Some(left_signature), Some(right_signature)) = (
            self.types.function_signature(left),
            self.types.function_signature(right),
        ) {
            if left_signature.variadic != right_signature.variadic {
                return None;
            }
            let result = self.composite_type(left_signature.result, right_signature.result)?;
            let parameters = match (&left_signature.parameters, &right_signature.parameters) {
                (FunctionParameters::Unspecified, FunctionParameters::Unspecified) => {
                    FunctionParameters::Unspecified
                }
                (FunctionParameters::Unspecified, FunctionParameters::Prototype(parameters)) => {
                    if !self.prototype_compatible_with_unspecified(&right_signature) {
                        return None;
                    }
                    FunctionParameters::Prototype(parameters.clone())
                }
                (FunctionParameters::Prototype(parameters), FunctionParameters::Unspecified) => {
                    if !self.prototype_compatible_with_unspecified(&left_signature) {
                        return None;
                    }
                    FunctionParameters::Prototype(parameters.clone())
                }
                (
                    FunctionParameters::Prototype(left_parameters),
                    FunctionParameters::Prototype(right_parameters),
                ) => {
                    if left_parameters.len() != right_parameters.len() {
                        return None;
                    }
                    FunctionParameters::Prototype(
                        left_parameters
                            .iter()
                            .zip(right_parameters)
                            .map(|(left, right)| self.composite_type(*left, *right))
                            .collect::<Option<Vec<_>>>()?,
                    )
                }
            };
            return Some(self.types.function_type(FunctionType {
                result,
                parameters,
                variadic: left_signature.variadic,
            }));
        }

        match (
            self.types.try_kind(left).cloned(),
            self.types.try_kind(right).cloned(),
        ) {
            (Some(TypeKind::Pointer(left)), Some(TypeKind::Pointer(right))) => {
                let pointee = self.composite_type(left.pointee, right.pointee)?;
                Some(self.types.pointer(pointee))
            }
            (Some(TypeKind::Array(left)), Some(TypeKind::Array(right))) => {
                let element = self.composite_type(left.element, right.element)?;
                let length = match (left.length, right.length) {
                    (ArrayLength::Incomplete, length) | (length, ArrayLength::Incomplete) => length,
                    (left, right) if left == right => left,
                    _ => return None,
                };
                Some(self.types.array(ArrayType { element, length }))
            }
            _ => None,
        }
    }

    fn prototype_compatible_with_unspecified(&self, signature: &FunctionType) -> bool {
        if signature.variadic {
            return false;
        }
        let FunctionParameters::Prototype(parameters) = &signature.parameters else {
            return true;
        };
        parameters.iter().all(|parameter| {
            if !parameter.qualifiers.is_empty() {
                return false;
            }
            match self.types.builtin_type(parameter.ty) {
                Some(BuiltinType::Float) => false,
                Some(kind) if kind.is_integer() => {
                    self.promoted_integer_type(parameter.ty) == parameter.ty
                }
                _ => !matches!(self.types.try_kind(parameter.ty), Some(TypeKind::Enum(_))),
            }
        })
    }

    fn validate_object_type(
        &mut self,
        ty: QualifiedType,
        span: Span,
        require_complete: bool,
    ) -> AnalysisResult<()> {
        if ty.ty == TypeId::VOID || self.types.function_signature(ty.ty).is_some() {
            return self.fail("CCC2341", span, "an object must have object type");
        }
        match self.types.layout_of(ty.ty, self.config) {
            Ok(_) => Ok(()),
            Err(ccc_types::LayoutError::IncompleteArray(_)) if !require_complete => Ok(()),
            Err(ccc_types::LayoutError::VariableLengthArray { .. }) => Ok(()),
            Err(error) => self.fail("CCC2342", span, error.to_string()),
        }
    }

    fn is_variably_modified(&self, ty: TypeId) -> bool {
        match self.types.try_kind(ty) {
            Some(TypeKind::Array(array)) => {
                matches!(array.length, ArrayLength::Variable(_))
                    || self.is_variably_modified(array.element.ty)
            }
            Some(TypeKind::Pointer(pointer)) => self.is_variably_modified(pointer.pointee.ty),
            _ => false,
        }
    }

    fn is_modifiable_type(&self, ty: QualifiedType) -> bool {
        !ty.qualifiers.contains(TypeQualifiers::CONST)
            && !matches!(
                self.types.try_kind(ty.ty),
                Some(TypeKind::Array(_) | TypeKind::Function(_))
            )
    }

    fn object_place(&self, base: PlaceBase, ty: QualifiedType, addressable: bool) -> Place {
        Place {
            base,
            projections: Vec::new(),
            access: access_semantics(ty),
            modifiable: self.is_modifiable_type(ty),
            addressable,
            bitfield: None,
        }
    }

    fn is_signed_integer(&self, ty: TypeId) -> bool {
        self.integer_representation(ty)
            .is_none_or(|kind| self.integer_kind_is_signed(kind))
    }

    fn integer_kind_is_signed(&self, kind: BuiltinType) -> bool {
        if kind == BuiltinType::Char {
            self.config.target.data_layout.char_is_signed
        } else {
            is_signed_builtin(kind)
        }
    }

    fn size_type(&self) -> TypeId {
        self.unsigned_integer_for_width(self.config.target.data_layout.pointer_width)
    }

    fn ptrdiff_type(&self) -> TypeId {
        self.signed_integer_for_width(self.config.target.data_layout.pointer_width)
    }

    fn wchar_type(&self) -> TypeId {
        let width = self.config.target.data_layout.wchar_width;
        if self.config.target.data_layout.wchar_is_signed {
            self.signed_integer_for_width(width)
        } else {
            self.unsigned_integer_for_width(width)
        }
    }

    fn signed_integer_for_width(&self, width: u8) -> TypeId {
        let layout = &self.config.target.data_layout;
        if width == layout.int_width {
            TypeId::INT
        } else if width == layout.long_width {
            TypeId::LONG
        } else if width == layout.long_long_width {
            TypeId::LONG_LONG
        } else if width == layout.short_width {
            TypeId::SHORT
        } else {
            TypeId::SIGNED_CHAR
        }
    }

    fn unsigned_integer_for_width(&self, width: u8) -> TypeId {
        let layout = &self.config.target.data_layout;
        if width == layout.int_width {
            TypeId::UNSIGNED_INT
        } else if width == layout.long_width {
            TypeId::UNSIGNED_LONG
        } else if width == layout.long_long_width {
            TypeId::UNSIGNED_LONG_LONG
        } else if width == layout.short_width {
            TypeId::UNSIGNED_SHORT
        } else {
            TypeId::UNSIGNED_CHAR
        }
    }

    fn reject_long_double_operation(
        &mut self,
        types: &[QualifiedType],
        span: Span,
    ) -> AnalysisResult<()> {
        if types.iter().any(|ty| ty.ty == TypeId::LONG_DOUBLE) {
            self.fail(
                "CCC2343",
                span,
                "long double layout is supported, but this operation requires unavailable arithmetic or ABI support",
            )
        } else {
            Ok(())
        }
    }

    fn validate_attributes(
        &mut self,
        attributes: &[syntax::Attribute],
    ) -> AnalysisResult<Vec<FullTypedAttribute>> {
        attributes
            .iter()
            .map(|attribute| self.validate_attribute(attribute))
            .collect()
    }

    fn validate_attribute(
        &mut self,
        attribute: &syntax::Attribute,
    ) -> AnalysisResult<FullTypedAttribute> {
        let state = self
            .config
            .capabilities
            .state(CapabilityKind::Attribute, &attribute.name.name);
        if matches!(
            state,
            CapabilityState::ParseOnly | CapabilityState::Unsupported
        ) {
            let qualifier = if state == CapabilityState::ParseOnly {
                "parse-only"
            } else {
                "unsupported"
            };
            return self.fail(
                "CCC2345",
                attribute.span,
                format!(
                    "attribute `{}` is {qualifier} in the effective capability configuration",
                    attribute.name.name
                ),
            );
        }
        Ok(FullTypedAttribute {
            introducer: attribute.introducer.clone(),
            name: attribute.name.name.clone(),
            arguments: attribute
                .arguments
                .iter()
                .map(|token| token.spelling.clone())
                .collect(),
            capability: state,
        })
    }

    fn resolve_asm_label(
        &mut self,
        label: Option<&syntax::AsmLabel>,
    ) -> AnalysisResult<Option<FullTypedAsmLabel>> {
        let Some(label) = label else {
            return Ok(None);
        };
        let state = self
            .config
            .capabilities
            .state(CapabilityKind::Extension, "gnu-declaration-asm-labels");
        if state != CapabilityState::Implemented {
            return self.fail(
                "CCC2346",
                label.span,
                "assembly labels are retained by parsing but require implemented symbol semantics",
            );
        }
        if label.literal.prefix != StringLiteralPrefix::None {
            return self.fail(
                "CCC2347",
                label.span,
                "an assembly label must use an ordinary string literal",
            );
        }
        let bytes = label
            .literal
            .code_units
            .iter()
            .map(|unit| u8::try_from(*unit))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| {
                self.emit(
                    "CCC2348",
                    label.span,
                    "assembly label contains a code unit outside the byte range",
                );
            })?;
        if bytes.contains(&0) {
            return self.fail(
                "CCC2349",
                label.span,
                "assembly label cannot contain a null byte",
            );
        }
        let symbol = String::from_utf8(bytes).map_err(|_| {
            self.emit("CCC2350", label.span, "assembly label is not valid UTF-8");
        })?;
        Ok(Some(FullTypedAsmLabel {
            keyword_spelling: label.keyword_spelling.clone(),
            literal_spelling: label.literal_spelling.clone(),
            symbol,
        }))
    }

    fn apply_emission_attributes(
        &mut self,
        emission: &mut GlobalEmission,
        attributes: &[FullTypedAttribute],
        span: Span,
    ) -> AnalysisResult<()> {
        for attribute in attributes {
            match attribute.name.as_str() {
                "aligned" => {
                    let Some(value) = attribute_argument_number(&attribute.arguments) else {
                        return self.fail(
                            "CCC2351",
                            span,
                            "implemented `aligned` requires an integer argument",
                        );
                    };
                    if !value.is_power_of_two() {
                        return self.fail(
                            "CCC2352",
                            span,
                            "requested alignment must be a power of two",
                        );
                    }
                    emission.requested_alignment = Some(value);
                }
                "section" => {
                    emission.section = attribute_argument_string(&attribute.arguments);
                }
                "visibility" => {
                    emission.visibility =
                        match attribute_argument_string(&attribute.arguments).as_deref() {
                            Some("hidden") => SymbolVisibility::Hidden,
                            Some("protected") => SymbolVisibility::Protected,
                            Some("internal") => SymbolVisibility::Internal,
                            Some("default") => SymbolVisibility::Default,
                            _ => {
                                return self.fail(
                                    "CCC2353",
                                    span,
                                    "implemented `visibility` has an invalid argument",
                                );
                            }
                        };
                }
                "tls_model" => {
                    emission.tls = match attribute_argument_string(&attribute.arguments).as_deref()
                    {
                        Some("global-dynamic") => Some(TlsModel::GeneralDynamic),
                        Some("local-dynamic") => Some(TlsModel::LocalDynamic),
                        Some("initial-exec") => Some(TlsModel::InitialExec),
                        Some("local-exec") => Some(TlsModel::LocalExec),
                        _ => {
                            return self.fail(
                                "CCC2354",
                                span,
                                "implemented `tls_model` has an invalid argument",
                            );
                        }
                    };
                }
                "common" => emission.definition = ObjectDefinitionPolicy::TentativeCommon,
                "nocommon" if emission.definition == ObjectDefinitionPolicy::TentativeCommon => {
                    emission.definition = ObjectDefinitionPolicy::Definition
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn handle_pragma(&mut self, pragma: &PragmaEvent) -> AnalysisResult<()> {
        match pragma {
            PragmaEvent::Pack { payload, span } => self.handle_pack(payload, *span),
            PragmaEvent::Unknown { text, span } => {
                let name = text.split_whitespace().next().unwrap_or(text);
                let state = self.config.capabilities.state(CapabilityKind::Pragma, name);
                if matches!(
                    state,
                    CapabilityState::Implemented | CapabilityState::BehaviorCompatibleNoOp
                ) {
                    Ok(())
                } else {
                    self.fail(
                        "CCC2355",
                        *span,
                        format!("pragma `{name}` has no supported semantic behavior"),
                    )
                }
            }
            PragmaEvent::Once { .. }
            | PragmaEvent::SystemHeader { .. }
            | PragmaEvent::Diagnostic { .. } => Ok(()),
        }
    }

    fn handle_pack(&mut self, payload: &[ccc_pp::PpToken], span: Span) -> AnalysisResult<()> {
        let spellings = payload
            .iter()
            .map(|token| token.spelling.as_str())
            .collect::<Vec<_>>();
        if spellings.first() != Some(&"(") || spellings.last() != Some(&")") {
            return self.fail("CCC2356", span, "malformed pack pragma payload");
        }
        let inner = &spellings[1..spellings.len() - 1];
        if inner.is_empty() {
            self.packing.current = PackingPolicy::NATIVE;
            return Ok(());
        }
        let parts = split_pack_parts(inner).ok_or_else(|| {
            self.emit("CCC2357", span, "malformed comma placement in pack pragma");
        })?;
        match parts.first().map(Vec::as_slice) {
            Some(["push"]) => {
                let mut label = None;
                let mut maximum = None;
                for part in &parts[1..] {
                    if let Some(number) = parse_pack_number(part) {
                        maximum = Some(number);
                    } else if part.len() == 1 && label.is_none() {
                        label = Some(part[0].to_owned());
                    } else {
                        return self.fail("CCC2358", span, "invalid pack push argument");
                    }
                }
                self.packing.stack.push(PackingFrame {
                    policy: self.packing.current,
                    label,
                });
                if let Some(maximum) = maximum {
                    self.packing.current = pack_policy(maximum);
                }
            }
            Some(["pop"]) => {
                let label = parts.get(1).and_then(|part| {
                    (part.len() == 1 && parse_pack_number(part).is_none()).then_some(part[0])
                });
                let frame = if let Some(label) = label {
                    let Some(index) = self
                        .packing
                        .stack
                        .iter()
                        .rposition(|frame| frame.label.as_deref() == Some(label))
                    else {
                        return self.fail(
                            "CCC2359",
                            span,
                            format!("pack pop names unknown stack label `{label}`"),
                        );
                    };
                    let frame = self.packing.stack[index].clone();
                    self.packing.stack.truncate(index);
                    frame
                } else {
                    self.packing.stack.pop().ok_or_else(|| {
                        self.emit("CCC2360", span, "pack pop has no matching push");
                    })?
                };
                self.packing.current = frame.policy;
                if let Some(maximum) = parts
                    .iter()
                    .skip(1)
                    .find_map(|part| parse_pack_number(part))
                {
                    self.packing.current = pack_policy(maximum);
                }
            }
            _ if parts.len() == 1 => {
                let Some(maximum) = parse_pack_number(&parts[0]) else {
                    return self.fail("CCC2361", span, "invalid pack alignment argument");
                };
                self.packing.current = pack_policy(maximum);
            }
            _ => return self.fail("CCC2362", span, "unsupported pack pragma form"),
        }
        Ok(())
    }

    fn collect_labels(&mut self, statement: &syntax::Statement) {
        use syntax::StatementKind as S;
        match &statement.kind {
            S::Label {
                label, statement, ..
            } => {
                let labels = &mut self
                    .function
                    .as_mut()
                    .expect("label collection occurs inside a function")
                    .labels;
                labels.reserve_definition(&label.name);
                self.collect_labels(statement);
            }
            S::Case { statement, .. }
            | S::Default(statement)
            | S::Switch { statement, .. }
            | S::While { statement, .. }
            | S::DoWhile { statement, .. }
            | S::For { statement, .. } => self.collect_labels(statement),
            S::Compound(items) => {
                for item in items {
                    if let syntax::BlockItem::Statement(statement) = item {
                        self.collect_labels(statement);
                    }
                }
            }
            S::If {
                then_statement,
                else_statement,
                ..
            } => {
                self.collect_labels(then_statement);
                if let Some(statement) = else_statement {
                    self.collect_labels(statement);
                }
            }
            S::Expression(_) | S::Goto(_) | S::Continue | S::Break | S::Return(_) => {}
        }
    }

    fn validate_labels(&mut self) {
        let Some(function) = &self.function else {
            return;
        };
        let missing = function.labels.undefined_uses();
        for (name, span) in missing {
            self.emit(
                "CCC2363",
                span,
                format!("goto uses undefined label `{name}`"),
            );
        }
    }

    fn enter_loop(&mut self) {
        self.function
            .as_mut()
            .expect("loops occur inside functions")
            .loop_depth += 1;
    }

    fn leave_loop(&mut self) {
        self.function
            .as_mut()
            .expect("loops occur inside functions")
            .loop_depth -= 1;
    }

    fn fresh_local(&mut self) -> FullLocalId {
        let function = self
            .function
            .as_mut()
            .expect("locals are allocated inside functions");
        let id = FullLocalId(function.next_local);
        function.next_local = function
            .next_local
            .checked_add(1)
            .expect("local identifier space exhausted");
        id
    }

    fn push_scope(&mut self) {
        self.scopes.push();
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn bind_current(
        &mut self,
        name: String,
        symbol: OrdinarySymbol,
        span: Span,
    ) -> AnalysisResult<()> {
        match self.scopes.bind_current_ordinary(name.clone(), symbol) {
            Ok(()) => Ok(()),
            Err(OrdinaryBindingConflict::CurrentScope) => self.fail(
                "CCC2364",
                span,
                format!("ordinary identifier `{name}` is declared more than once in this scope"),
            ),
            Err(OrdinaryBindingConflict::FileScope) => {
                unreachable!("binding in the current scope cannot report a file-scope conflict")
            }
        }
    }

    fn bind_file(
        &mut self,
        name: String,
        symbol: OrdinarySymbol,
        span: Span,
    ) -> AnalysisResult<()> {
        match self.scopes.bind_file_ordinary(name.clone(), symbol) {
            Ok(()) => Ok(()),
            Err(OrdinaryBindingConflict::FileScope) => self.fail(
                "CCC2365",
                span,
                format!("ordinary identifier `{name}` already has a file-scope binding"),
            ),
            Err(OrdinaryBindingConflict::CurrentScope) => {
                unreachable!("binding in file scope cannot report a current-scope conflict")
            }
        }
    }

    fn lookup_ordinary(&self, name: &str) -> Option<&OrdinarySymbol> {
        self.scopes.lookup_ordinary(name)
    }

    fn lookup_file_ordinary(&self, name: &str) -> Option<&OrdinarySymbol> {
        self.scopes.lookup_file_ordinary(name)
    }

    fn bind_tag_current(&mut self, name: String, tag: TagSymbol, span: Span) -> AnalysisResult<()> {
        if self.scopes.bind_current_tag(name.clone(), tag).is_err() {
            return self.fail("CCC2366", span, format!("tag `{name}` is redeclared"));
        }
        Ok(())
    }

    fn lookup_tag(&self, name: &str) -> Option<TagSymbol> {
        self.scopes.lookup_tag(name)
    }

    fn emit(&mut self, code: &'static str, span: Span, message: impl Into<String>) {
        self.diagnostics
            .push(Diagnostic::error(code, message.into()).with_primary_span(span));
    }

    fn fail<T>(
        &mut self,
        code: &'static str,
        span: Span,
        message: impl Into<String>,
    ) -> AnalysisResult<T> {
        self.emit(code, span, message);
        Err(())
    }
}

fn qualifiers(qualifiers: &[syntax::TypeQualifier]) -> TypeQualifiers {
    qualifiers
        .iter()
        .fold(TypeQualifiers::NONE, |bits, qualifier| {
            bits | qualifier_bits(*qualifier)
        })
}

fn qualifier_bits(qualifier: syntax::TypeQualifier) -> TypeQualifiers {
    match qualifier {
        syntax::TypeQualifier::Const => TypeQualifiers::CONST,
        syntax::TypeQualifier::Restrict => TypeQualifiers::RESTRICT,
        syntax::TypeQualifier::Volatile => TypeQualifiers::VOLATILE,
        syntax::TypeQualifier::Atomic => TypeQualifiers::ATOMIC,
    }
}

fn parameter_array_qualifiers(declarator: &Option<syntax::Declarator>) -> TypeQualifiers {
    fn direct(node: &syntax::DirectDeclarator) -> Option<TypeQualifiers> {
        match node {
            syntax::DirectDeclarator::Array {
                inner,
                qualifiers: array_qualifiers,
                ..
            } => direct(inner).or_else(|| Some(qualifiers(array_qualifiers))),
            syntax::DirectDeclarator::Parenthesized(declarator, _) => direct(&declarator.direct),
            syntax::DirectDeclarator::Identifier(_)
            | syntax::DirectDeclarator::Abstract(_)
            | syntax::DirectDeclarator::Function { .. } => None,
        }
    }

    declarator
        .as_ref()
        .and_then(|declarator| direct(&declarator.direct))
        .unwrap_or(TypeQualifiers::NONE)
}

fn access_semantics(ty: QualifiedType) -> AccessSemantics {
    AccessSemantics {
        volatile: ty.qualifiers.contains(TypeQualifiers::VOLATILE),
        atomic: ty.qualifiers.contains(TypeQualifiers::ATOMIC),
    }
}

fn direct_function_reference(expression: &FullTypedExpression) -> Option<FullFunctionId> {
    match &expression.kind {
        FullTypedExpressionKind::DeclRef(SymbolReference::Function(id)) => Some(*id),
        FullTypedExpressionKind::Conversion {
            kind: ConversionKind::FunctionToPointer,
            expression,
        } => direct_function_reference(expression),
        _ => None,
    }
}

fn initializer_is_static(initializer: &FullTypedInitializer) -> bool {
    match &initializer.kind {
        FullTypedInitializerKind::Scalar(expression) => expression.constant.is_some(),
        FullTypedInitializerKind::Aggregate(entries) => entries
            .iter()
            .all(|entry| initializer_is_static(&entry.initializer)),
        FullTypedInitializerKind::String(_) | FullTypedInitializerKind::Zero => true,
    }
}

fn is_signed_builtin(kind: BuiltinType) -> bool {
    matches!(
        kind,
        BuiltinType::Char
            | BuiltinType::SignedChar
            | BuiltinType::Short
            | BuiltinType::Int
            | BuiltinType::Long
            | BuiltinType::LongLong
    )
}

fn integer_rank(kind: BuiltinType) -> u8 {
    match kind {
        BuiltinType::Bool => 0,
        BuiltinType::Char | BuiltinType::SignedChar | BuiltinType::UnsignedChar => 1,
        BuiltinType::Short | BuiltinType::UnsignedShort => 2,
        BuiltinType::Int | BuiltinType::UnsignedInt => 3,
        BuiltinType::Long | BuiltinType::UnsignedLong => 4,
        BuiltinType::LongLong | BuiltinType::UnsignedLongLong => 5,
        BuiltinType::Void | BuiltinType::Float | BuiltinType::Double | BuiltinType::LongDouble => 0,
    }
}

fn unsigned_counterpart(kind: BuiltinType) -> BuiltinType {
    match kind {
        BuiltinType::Char | BuiltinType::SignedChar => BuiltinType::UnsignedChar,
        BuiltinType::Short => BuiltinType::UnsignedShort,
        BuiltinType::Int => BuiltinType::UnsignedInt,
        BuiltinType::Long => BuiltinType::UnsignedLong,
        BuiltinType::LongLong => BuiltinType::UnsignedLongLong,
        other => other,
    }
}

fn truncate_to_width(value: u128, width: u8) -> u128 {
    if width >= 128 {
        value
    } else {
        value & ((1_u128 << width) - 1)
    }
}

fn sign_extend(value: u128, width: u8) -> i128 {
    if width >= 128 {
        value as i128
    } else {
        let sign = 1_u128 << (width - 1);
        if value & sign == 0 {
            value as i128
        } else {
            (value | (!0_u128 << width)) as i128
        }
    }
}

fn signed_max(width: u8) -> u128 {
    if width >= 128 {
        i128::MAX as u128
    } else {
        (1_u128 << (width - 1)) - 1
    }
}

fn signed_min(width: u8) -> i128 {
    if width >= 128 {
        i128::MIN
    } else {
        -(1_i128 << (width - 1))
    }
}

fn unsigned_max(width: u8) -> u128 {
    if width >= 128 {
        u128::MAX
    } else {
        (1_u128 << width) - 1
    }
}

fn evaluate_binary_constant(
    operator: syntax::BinaryOperator,
    left: Option<ConstantValue>,
    right: Option<ConstantValue>,
) -> Option<ConstantValue> {
    use syntax::BinaryOperator as B;
    let left = left?;
    let right = right?;
    let boolean = |value: bool| Some(ConstantValue::Signed(i128::from(value)));
    match (left, right) {
        (ConstantValue::Signed(left), ConstantValue::Signed(right)) => match operator {
            B::Multiply => left.checked_mul(right).map(ConstantValue::Signed),
            B::Divide => (right != 0)
                .then(|| left.checked_div(right))
                .flatten()
                .map(ConstantValue::Signed),
            B::Remainder => (right != 0)
                .then(|| left.checked_rem(right))
                .flatten()
                .map(ConstantValue::Signed),
            B::Add => left.checked_add(right).map(ConstantValue::Signed),
            B::Subtract => left.checked_sub(right).map(ConstantValue::Signed),
            B::LeftShift => u32::try_from(right)
                .ok()
                .and_then(|right| left.checked_shl(right))
                .map(ConstantValue::Signed),
            B::RightShift => u32::try_from(right)
                .ok()
                .and_then(|right| left.checked_shr(right))
                .map(ConstantValue::Signed),
            B::Less => boolean(left < right),
            B::LessEqual => boolean(left <= right),
            B::Greater => boolean(left > right),
            B::GreaterEqual => boolean(left >= right),
            B::Equal => boolean(left == right),
            B::NotEqual => boolean(left != right),
            B::BitwiseAnd => Some(ConstantValue::Signed(left & right)),
            B::BitwiseXor => Some(ConstantValue::Signed(left ^ right)),
            B::BitwiseOr => Some(ConstantValue::Signed(left | right)),
            B::LogicalAnd => boolean(left != 0 && right != 0),
            B::LogicalOr => boolean(left != 0 || right != 0),
        },
        (ConstantValue::Unsigned(left), ConstantValue::Unsigned(right)) => match operator {
            B::Multiply => Some(ConstantValue::Unsigned(left.wrapping_mul(right))),
            B::Divide => (right != 0).then_some(ConstantValue::Unsigned(left / right)),
            B::Remainder => (right != 0).then_some(ConstantValue::Unsigned(left % right)),
            B::Add => Some(ConstantValue::Unsigned(left.wrapping_add(right))),
            B::Subtract => Some(ConstantValue::Unsigned(left.wrapping_sub(right))),
            B::LeftShift => u32::try_from(right)
                .ok()
                .and_then(|right| left.checked_shl(right))
                .map(ConstantValue::Unsigned),
            B::RightShift => u32::try_from(right)
                .ok()
                .and_then(|right| left.checked_shr(right))
                .map(ConstantValue::Unsigned),
            B::Less => boolean(left < right),
            B::LessEqual => boolean(left <= right),
            B::Greater => boolean(left > right),
            B::GreaterEqual => boolean(left >= right),
            B::Equal => boolean(left == right),
            B::NotEqual => boolean(left != right),
            B::BitwiseAnd => Some(ConstantValue::Unsigned(left & right)),
            B::BitwiseXor => Some(ConstantValue::Unsigned(left ^ right)),
            B::BitwiseOr => Some(ConstantValue::Unsigned(left | right)),
            B::LogicalAnd => boolean(left != 0 && right != 0),
            B::LogicalOr => boolean(left != 0 || right != 0),
        },
        (ConstantValue::Floating(left), ConstantValue::Floating(right)) => match operator {
            B::Multiply => Some(ConstantValue::Floating(left * right)),
            B::Divide => Some(ConstantValue::Floating(left / right)),
            B::Add => Some(ConstantValue::Floating(left + right)),
            B::Subtract => Some(ConstantValue::Floating(left - right)),
            B::Less => boolean(left < right),
            B::LessEqual => boolean(left <= right),
            B::Greater => boolean(left > right),
            B::GreaterEqual => boolean(left >= right),
            B::Equal => boolean(left == right),
            B::NotEqual => boolean(left != right),
            B::LogicalAnd => boolean(left != 0.0 && right != 0.0),
            B::LogicalOr => boolean(left != 0.0 || right != 0.0),
            _ => None,
        },
        (ConstantValue::NullPointer, ConstantValue::NullPointer) => match operator {
            B::Equal => boolean(true),
            B::NotEqual => boolean(false),
            _ => None,
        },
        (ConstantValue::Address(left), ConstantValue::Address(right)) => match operator {
            B::Equal => boolean(left == right),
            B::NotEqual => boolean(left != right),
            _ => None,
        },
        (ConstantValue::Address(_), ConstantValue::NullPointer)
        | (ConstantValue::NullPointer, ConstantValue::Address(_)) => match operator {
            B::Equal => boolean(false),
            B::NotEqual => boolean(true),
            _ => None,
        },
        _ => None,
    }
}

fn split_pack_parts<'a>(tokens: &'a [&'a str]) -> Option<Vec<Vec<&'a str>>> {
    let mut parts = vec![Vec::new()];
    for token in tokens {
        if *token == "," {
            if parts.last().is_none_or(Vec::is_empty) {
                return None;
            }
            parts.push(Vec::new());
        } else {
            parts.last_mut()?.push(*token);
        }
    }
    (!parts.last().is_none_or(Vec::is_empty)).then_some(parts)
}

fn parse_pack_number(tokens: &[&str]) -> Option<u64> {
    if tokens.len() != 1 {
        return None;
    }
    let value = tokens[0].parse::<u64>().ok()?;
    matches!(value, 0 | 1 | 2 | 4 | 8 | 16).then_some(value)
}

fn pack_policy(maximum: u64) -> PackingPolicy {
    if maximum == 0 {
        PackingPolicy::NATIVE
    } else {
        PackingPolicy::with_maximum_field_alignment(maximum)
    }
}

fn attribute_argument_number(arguments: &[String]) -> Option<u64> {
    arguments
        .iter()
        .find_map(|argument| argument.parse::<u64>().ok())
}

fn attribute_argument_string(arguments: &[String]) -> Option<String> {
    arguments.iter().find_map(|argument| {
        argument
            .strip_prefix('"')
            .and_then(|argument| argument.strip_suffix('"'))
            .map(str::to_owned)
    })
}
