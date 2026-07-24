use std::collections::{HashMap, HashSet};

use ccc_diag::Diagnostic;
use ccc_pp::{
    CharacterConstantPrefix, FloatingConstant, FloatingConstantSuffix, PragmaEvent,
    StringLiteralPrefix, canonicalize_identifier,
};
use ccc_session::Span;
use ccc_syntax::frontend as syntax;
use ccc_target::{
    AbiIdentity, CapabilityKind, CapabilityState, EffectiveCompilationConfig, LanguageMode,
    LongDoubleFormat, PackingPolicy, TargetBuiltinType,
};
use ccc_types::{
    ArrayLength, ArrayType, BuiltinType, Field, FunctionParameters, FunctionType, LayoutShape,
    QualifiedType, RecordKind, TypeId, TypeKind, TypeQualifiers, TypeStore, VariableLengthId,
};
use rustc_apfloat::ieee::{Double, Half, Quad, Single, X87DoubleExtended};
use rustc_apfloat::{Float, FloatConvert, Round, Status};

use super::model::*;
use super::scopes::{
    DetachedSemanticScope, LabelScope, OrdinaryBindingConflict, OrdinarySymbol, ScopeStack,
    TagCategory, TagSymbol,
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
    variable_length_bounds: Vec<FullTypedVariableLengthBound>,
    span: Span,
    addressable: bool,
    va_start_restriction: Option<&'static str>,
}

#[derive(Clone, Debug)]
struct ResolvedDeclarator {
    name: Option<String>,
    name_span: Span,
    ty: QualifiedType,
    parameters: Vec<ResolvedParameter>,
    parameter_list_span: Option<Span>,
    parameter_scope: Option<DetachedSemanticScope>,
    variable_length_bounds: Vec<FullTypedVariableLengthBound>,
    attributes: Vec<FullTypedAttribute>,
}

struct DeclaratorContext {
    parameters: Vec<ResolvedParameter>,
    parameter_list_span: Option<Span>,
    parameter_scope: Option<DetachedSemanticScope>,
    variable_length_bounds: Vec<FullTypedVariableLengthBound>,
    prototype_parameter: bool,
}

impl DeclaratorContext {
    fn new(prototype_parameter: bool) -> Self {
        Self {
            parameters: Vec::new(),
            parameter_list_span: None,
            parameter_scope: None,
            variable_length_bounds: Vec::new(),
            prototype_parameter,
        }
    }
}

#[derive(Clone, Debug)]
struct DeclarationInfo {
    base: QualifiedType,
    variable_length_bounds: Vec<FullTypedVariableLengthBound>,
    storage: Option<syntax::StorageClass>,
    thread_local: bool,
    properties: FunctionProperties,
    attributes: Vec<FullTypedAttribute>,
    has_alignment_specifier: bool,
    requested_alignment: Option<u64>,
}

#[derive(Default)]
struct SwitchState {
    cases: HashMap<u128, Span>,
    default: Option<Span>,
    entry_variably_modified_path: Vec<VariablyModifiedScopeEntry>,
    controlling_type: Option<QualifiedType>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum VariablyModifiedScopeEntry {
    Object(FullLocalId),
    Typedef(TypedefId),
}

struct VariablyModifiedGoto {
    label: LabelId,
    span: Span,
    source_path: Vec<VariablyModifiedScopeEntry>,
}

/// Alignment facts retained while an atomic-object address is still expressed
/// directly in the typed tree.
///
/// `Mixed` keeps a statically known alternative alongside one or more
/// alternatives whose provenance is unavailable. This distinction matters for
/// conditionals: an arbitrary pointer remains subject to the caller's natural
/// alignment contract, but it must not erase a reachable packed-member address
/// whose alignment is already known to be too weak.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PointerAlignmentProvenance {
    Unknown,
    Known(u64),
    Mixed { known_minimum: u64 },
}

impl PointerAlignmentProvenance {
    fn known(alignment: u64) -> Self {
        Self::Known(alignment)
    }

    fn known_minimum(self) -> Option<u64> {
        match self {
            Self::Unknown => None,
            Self::Known(alignment) => Some(alignment),
            Self::Mixed { known_minimum } => Some(known_minimum),
        }
    }

    fn map_known(self, map: impl FnOnce(u64) -> u64) -> Self {
        match self {
            Self::Unknown => Self::Unknown,
            Self::Known(alignment) => Self::Known(map(alignment)),
            Self::Mixed { known_minimum } => Self::Mixed {
                known_minimum: map(known_minimum),
            },
        }
    }

    fn merge(self, other: Self) -> Self {
        match (self, other) {
            (Self::Unknown, Self::Unknown) => Self::Unknown,
            (Self::Known(alignment), Self::Unknown) | (Self::Unknown, Self::Known(alignment)) => {
                Self::Mixed {
                    known_minimum: alignment,
                }
            }
            (Self::Mixed { known_minimum }, Self::Unknown)
            | (Self::Unknown, Self::Mixed { known_minimum }) => Self::Mixed { known_minimum },
            (Self::Known(left), Self::Known(right)) => Self::Known(left.min(right)),
            (Self::Known(alignment), Self::Mixed { known_minimum })
            | (Self::Mixed { known_minimum }, Self::Known(alignment)) => Self::Mixed {
                known_minimum: alignment.min(known_minimum),
            },
            (
                Self::Mixed {
                    known_minimum: left,
                },
                Self::Mixed {
                    known_minimum: right,
                },
            ) => Self::Mixed {
                known_minimum: left.min(right),
            },
        }
    }
}

struct FunctionState {
    id: FullFunctionId,
    name: String,
    predefined_name_code_units: Vec<u32>,
    predefined_name_string: Option<StringId>,
    return_ty: QualifiedType,
    next_local: u32,
    unaddressable_locals: HashSet<FullLocalId>,
    static_duration_locals: HashMap<FullLocalId, StorageDuration>,
    labels: LabelScope,
    active_variably_modified_path: Vec<VariablyModifiedScopeEntry>,
    variably_modified_scope_starts: Vec<usize>,
    variably_modified_label_paths: HashMap<LabelId, (Span, Vec<VariablyModifiedScopeEntry>)>,
    variably_modified_gotos: Vec<VariablyModifiedGoto>,
    computed_gotos: Vec<Span>,
    has_variably_modified_declaration: bool,
    loop_depth: usize,
    switches: Vec<SwitchState>,
    variadic: bool,
    last_named_parameter: Option<FullLocalId>,
    last_named_parameter_restriction: Option<&'static str>,
}

#[derive(Clone, Copy, Debug, Default)]
struct GlobalStandardAlignment {
    explicit: Option<u64>,
    definition: Option<Option<u64>>,
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
    analyze_frontend_with_error_limit(unit, config, None)
}

pub fn analyze_frontend_with_error_limit(
    unit: &syntax::TranslationUnit,
    config: &EffectiveCompilationConfig,
    error_limit: Option<usize>,
) -> Result<FullTypedTranslationUnit, Vec<Diagnostic>> {
    analyze_frontend_with_recovery_limit(unit, config, error_limit, &[])
}

pub fn analyze_frontend_with_recovery_limit(
    unit: &syntax::TranslationUnit,
    config: &EffectiveCompilationConfig,
    error_limit: Option<usize>,
    poisoned_bindings: &[syntax::PoisonedBinding],
) -> Result<FullTypedTranslationUnit, Vec<Diagnostic>> {
    let mut analyzer = Analyzer::new(config, error_limit, poisoned_bindings);
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
    global_standard_alignments: HashMap<GlobalId, GlobalStandardAlignment>,
    scopes: ScopeStack,
    function: Option<FunctionState>,
    parameter_scope_depth: usize,
    definition_parameter_list: Option<Span>,
    packing: PackingState,
    diagnostics: Vec<Diagnostic>,
    error_limit: Option<usize>,
    poisoned_bindings: &'a [syntax::PoisonedBinding],
}

impl<'a> Analyzer<'a> {
    fn new(
        config: &'a EffectiveCompilationConfig,
        error_limit: Option<usize>,
        poisoned_bindings: &'a [syntax::PoisonedBinding],
    ) -> Self {
        Self {
            config,
            types: ccc_types::TypeStore::default(),
            external_items: Vec::new(),
            globals: Vec::new(),
            functions: Vec::new(),
            typedefs: Vec::new(),
            strings: Vec::new(),
            string_pool: HashMap::new(),
            global_standard_alignments: HashMap::new(),
            scopes: ScopeStack::new(),
            function: None,
            parameter_scope_depth: 0,
            definition_parameter_list: None,
            packing: PackingState::default(),
            diagnostics: Vec::new(),
            error_limit,
            poisoned_bindings,
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
            if self.error_limit_reached() {
                break;
            }
            match item {
                syntax::ExternalItem::Pragma(pragma) => {
                    if self.handle_pragma(pragma).is_ok() {
                        self.external_items
                            .push(FullTypedExternalItem::Pragma(pragma.clone()));
                    }
                }
                syntax::ExternalItem::Declaration(declaration) => {
                    if self.analyze_file_declaration(declaration).is_err() {
                        self.poison_declaration_bindings(declaration, true);
                    }
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
        self.reject_packed_attribute(&info.attributes, declaration.span)?;
        if declaration.declarators.is_empty() {
            self.reject_alignment_specifier(&info, declaration.span, "a type declaration")?;
            self.reject_weak_attribute(&info.attributes, declaration.span, "a type declaration")?;
            self.reject_function_inlining_attribute(
                &info.attributes,
                declaration.span,
                "a type declaration",
            )?;
            self.external_items
                .push(FullTypedExternalItem::TypeDeclaration {
                    ty: info.base.ty,
                    span: declaration.span,
                });
            return Ok(());
        }

        for (declarator_index, init) in declaration.declarators.iter().enumerate() {
            let mut resolved = self.resolve_declarator(info.base, &init.declarator)?;
            if declarator_index == 0 {
                let mut bounds = info.variable_length_bounds.clone();
                bounds.extend(resolved.variable_length_bounds);
                resolved.variable_length_bounds = bounds;
            }
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
            self.reject_packed_attribute(&attributes, init.span)?;
            if info.storage == Some(syntax::StorageClass::Typedef) {
                if info.thread_local {
                    return self.fail(
                        "CCC2374",
                        init.span,
                        "a typedef cannot have thread-local storage duration",
                    );
                }
                self.reject_alignment_specifier(&info, init.span, "a typedef")?;
                if init.initializer.is_some() || init.asm_label.is_some() {
                    return self.fail(
                        "CCC2202",
                        init.span,
                        "a typedef cannot have an initializer or assembly label",
                    );
                }
                let ty = self.apply_file_typedef_attributes(
                    resolved.ty,
                    &attributes,
                    declaration.declarators.len() == 1
                        && defines_inline_anonymous_record(&declaration.specifiers),
                    init.span,
                )?;
                self.declare_typedef(name, ty, attributes, init.span)?;
            } else if self.types.function_signature(resolved.ty.ty).is_some() {
                if info.thread_local {
                    return self.fail(
                        "CCC2374",
                        init.span,
                        "a function cannot have thread-local storage duration",
                    );
                }
                self.reject_transparent_union_attribute(
                    &attributes,
                    init.span,
                    "a function declaration",
                )?;
                self.reject_alignment_specifier(&info, init.span, "a function")?;
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
                self.reject_transparent_union_attribute(
                    &attributes,
                    init.span,
                    "an object declaration",
                )?;
                let asm_label = self.resolve_asm_label(init.asm_label.as_ref())?;
                let id = self.declare_global(
                    name,
                    resolved.ty,
                    info.storage,
                    info.thread_local,
                    attributes,
                    info.requested_alignment,
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
        if info.thread_local {
            return self.fail(
                "CCC2374",
                definition.span,
                "a function cannot have thread-local storage duration",
            );
        }
        self.reject_packed_attribute(&info.attributes, definition.specifiers.span)?;
        self.reject_alignment_specifier(&info, definition.specifiers.span, "a function")?;
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
        let previous_definition_parameter_list = self.definition_parameter_list;
        self.definition_parameter_list =
            defining_function_parameter_list_span(&definition.declarator);
        let resolved = self.resolve_declarator(info.base, &definition.declarator);
        self.definition_parameter_list = previous_definition_parameter_list;
        let mut resolved = resolved?;
        let parameter_scope = resolved.parameter_scope.take();
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
        self.reject_int128_boundary_type(
            signature.result.ty,
            definition.declarator.span,
            "function return",
        )?;
        if let FunctionParameters::Prototype(parameters) = &signature.parameters {
            for parameter in parameters {
                self.reject_int128_boundary_type(
                    parameter.ty,
                    definition.declarator.span,
                    "function parameter",
                )?;
            }
        }
        let mut attributes = info.attributes;
        attributes.extend(resolved.attributes);
        self.reject_packed_attribute(&attributes, definition.declarator.span)?;
        self.reject_transparent_union_attribute(
            &attributes,
            definition.declarator.span,
            "a function definition",
        )?;
        let id = self.declare_function(
            name.clone(),
            resolved.ty.ty,
            info.storage,
            info.properties,
            attributes,
            None,
            definition.span,
        )?;
        if self.functions[id.0 as usize].binding == SymbolBinding::Weak && signature.variadic {
            return self.fail(
                "CCC2423",
                definition.span,
                format!(
                    "weak variadic definition of function `{name}` requires unsupported generated-entry binding semantics"
                ),
            );
        }
        if self.functions[id.0 as usize].body.is_some() {
            return self.fail(
                "CCC2208",
                definition.span,
                format!("function `{name}` is defined more than once"),
            );
        }

        let mut predefined_name_code_units = canonicalize_identifier(&name)
            .expect("the lexer validates universal character names in identifiers")
            .into_bytes()
            .into_iter()
            .map(u32::from)
            .collect::<Vec<_>>();
        predefined_name_code_units.push(0);
        let return_ty = signature.result;
        self.function = Some(FunctionState {
            id,
            name: name.clone(),
            predefined_name_code_units,
            predefined_name_string: None,
            return_ty,
            next_local: 0,
            unaddressable_locals: HashSet::new(),
            static_duration_locals: HashMap::new(),
            labels: LabelScope::default(),
            active_variably_modified_path: Vec::new(),
            variably_modified_scope_starts: Vec::new(),
            variably_modified_label_paths: HashMap::new(),
            variably_modified_gotos: Vec::new(),
            computed_gotos: Vec::new(),
            has_variably_modified_declaration: false,
            loop_depth: 0,
            switches: Vec::new(),
            variadic: signature.variadic,
            last_named_parameter: None,
            last_named_parameter_restriction: None,
        });
        self.collect_labels(&definition.body);
        let Some(parameter_scope) = parameter_scope else {
            self.function = None;
            return self.fail(
                "CCC2207",
                definition.declarator.span,
                "a function definition declarator must have a parameter scope",
            );
        };
        self.scopes.push_detached(parameter_scope);

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
            debug_assert_eq!(
                local.0,
                u32::try_from(typed_parameters.len())
                    .expect("parameter identifier space exhausted"),
                "resolved parameter bounds must use the definition's stable local IDs",
            );
            if !parameter.addressable {
                self.function
                    .as_mut()
                    .expect("parameter analysis occurs inside a function")
                    .unaddressable_locals
                    .insert(local);
            }
            self.scopes.replace_current_ordinary(
                parameter_name.clone(),
                OrdinarySymbol::Local(local, parameter.ty),
            );
            typed_parameters.push(FullTypedParameter {
                local,
                name: parameter_name,
                ty: parameter.ty,
                variable_length_bounds: parameter.variable_length_bounds.clone(),
                span: parameter.span,
            });
            self.function
                .as_mut()
                .expect("parameter analysis occurs inside a function")
                .last_named_parameter = Some(local);
            self.function
                .as_mut()
                .expect("parameter analysis occurs inside a function")
                .last_named_parameter_restriction = parameter.va_start_restriction;
        }

        let function_name_spellings: &[&str] = if self.config.language.mode == LanguageMode::Gnu11 {
            &["__func__", "__FUNCTION__", "__PRETTY_FUNCTION__"]
        } else {
            &["__func__"]
        };
        for spelling in function_name_spellings {
            if self
                .bind_current(
                    (*spelling).to_owned(),
                    OrdinarySymbol::PredefinedFunctionName,
                    resolved.name_span,
                )
                .is_err()
            {
                self.pop_scope();
                self.function = None;
                return Err(());
            }
        }

        let body = self.analyze_function_body(&definition.body);
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
        let mut thread_local = false;
        let mut properties = FunctionProperties::default();
        let mut qualifiers = TypeQualifiers::NONE;
        let mut type_specifiers = Vec::new();
        let mut attributes = Vec::new();
        let mut alignment_specifiers = Vec::new();

        for item in &specifiers.items {
            match item {
                syntax::DeclarationSpecifier::StorageClass(candidate) => {
                    if matches!(
                        candidate,
                        syntax::StorageClass::ThreadLocal | syntax::StorageClass::GnuThreadLocal
                    ) {
                        if thread_local {
                            return self.fail(
                                "CCC2210",
                                specifiers.span,
                                "a declaration repeats a thread-local storage specifier",
                            );
                        }
                        thread_local = true;
                    } else if storage.replace(*candidate).is_some() {
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
                syntax::DeclarationSpecifier::Alignment(specifier) => {
                    alignment_specifiers.push(specifier)
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
        let (mut base, variable_length_bounds) =
            self.resolve_type_specifiers(&type_specifiers, specifiers.span)?;
        base.qualifiers |= qualifiers;
        if base.qualifiers.contains(TypeQualifiers::ATOMIC)
            && self.type_contains_int128(base.ty, &mut HashSet::new())
        {
            return self.fail(
                "CCC2443",
                specifiers.span,
                "atomic 128-bit integers and aggregates containing them are not enabled",
            );
        }
        self.reject_weakened_atomic_alignment(base, specifiers.span)?;
        let mut requested_alignment = None;
        for specifier in &alignment_specifiers {
            if let Some(alignment) = self.resolve_alignment_specifier(specifier, specifiers.span)? {
                requested_alignment = Some(
                    requested_alignment.map_or(alignment, |current: u64| current.max(alignment)),
                );
            }
        }
        Ok(DeclarationInfo {
            base,
            variable_length_bounds,
            storage,
            thread_local,
            properties,
            attributes,
            has_alignment_specifier: !alignment_specifiers.is_empty(),
            requested_alignment,
        })
    }

    fn resolve_alignment_specifier(
        &mut self,
        specifier: &syntax::AlignmentSpecifier,
        span: Span,
    ) -> AnalysisResult<Option<u64>> {
        let value = match specifier {
            syntax::AlignmentSpecifier::Type(type_name) => {
                let (ty, _) = self.resolve_type_name_with_bounds(type_name)?;
                let ty = innermost_array_element(&self.types, ty);
                self.types
                    .layout_of(ty.ty, self.config)
                    .map_err(|error| self.emit("CCC2437", span, error.to_string()))?
                    .align
            }
            syntax::AlignmentSpecifier::Expression(expression) => {
                let value = self.evaluate_integer_constant(expression)?;
                u64::try_from(value).map_err(|_| {
                    self.emit(
                        "CCC2437",
                        span,
                        "an alignment must be a nonnegative integer constant",
                    );
                })?
            }
        };
        if value == 0 {
            return Ok(None);
        }
        if !supported_object_alignment(value) {
            return self.fail(
                "CCC2437",
                span,
                "a nonzero alignment must be a backend-supported power of two",
            );
        }
        Ok(Some(value))
    }

    fn reject_alignment_specifier(
        &mut self,
        info: &DeclarationInfo,
        span: Span,
        subject: &str,
    ) -> AnalysisResult<()> {
        if info.has_alignment_specifier {
            self.fail(
                "CCC2437",
                span,
                format!("an alignment specifier cannot be applied to {subject}"),
            )
        } else {
            Ok(())
        }
    }

    fn resolve_type_specifiers(
        &mut self,
        specifiers: &[&syntax::TypeSpecifier],
        span: Span,
    ) -> AnalysisResult<(QualifiedType, Vec<FullTypedVariableLengthBound>)> {
        if specifiers.len() == 1 {
            match specifiers[0] {
                syntax::TypeSpecifier::Struct(record) => {
                    return self
                        .resolve_record_specifier(RecordKind::Struct, record)
                        .map(QualifiedType::unqualified)
                        .map(|ty| (ty, Vec::new()));
                }
                syntax::TypeSpecifier::Union(record) => {
                    return self
                        .resolve_record_specifier(RecordKind::Union, record)
                        .map(QualifiedType::unqualified)
                        .map(|ty| (ty, Vec::new()));
                }
                syntax::TypeSpecifier::Enum(enumeration) => {
                    return self
                        .resolve_enum_specifier(enumeration)
                        .map(QualifiedType::unqualified)
                        .map(|ty| (ty, Vec::new()));
                }
                syntax::TypeSpecifier::TypedefName(identifier) => {
                    return match self.lookup_ordinary(&identifier.name) {
                        Some(OrdinarySymbol::Typedef(_, ty)) => Ok((*ty, Vec::new())),
                        Some(OrdinarySymbol::Poisoned) => {
                            Ok((QualifiedType::unqualified(TypeId::INT), Vec::new()))
                        }
                        _ => self.fail(
                            "CCC2213",
                            identifier.span,
                            format!("`{}` is not a typedef name", identifier.name),
                        ),
                    };
                }
                syntax::TypeSpecifier::Atomic(type_name) => {
                    let (mut ty, bounds) = self.resolve_type_name_with_bounds(type_name)?;
                    ty.qualifiers |= TypeQualifiers::ATOMIC;
                    return Ok((ty, bounds));
                }
                syntax::TypeSpecifier::Typeof(_) => {
                    return self.fail(
                        "CCC2214",
                        span,
                        "`typeof` is parse-only and has no supported semantic meaning",
                    );
                }
                syntax::TypeSpecifier::BuiltinVaList => {
                    let ty = self
                        .types
                        .target_builtin(TargetBuiltinType::VaList, self.config)
                        .map_err(|error| {
                            self.emit("CCC2408", span, error.to_string());
                        })?;
                    return Ok((QualifiedType::unqualified(ty), Vec::new()));
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
                    | syntax::TypeSpecifier::BuiltinVaList
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
        let float16 = count(|item| matches!(item, syntax::TypeSpecifier::Float16));
        let int128 = count(|item| matches!(item, syntax::TypeSpecifier::Int128));
        let int128_t = count(|item| matches!(item, syntax::TypeSpecifier::Int128T));
        let uint128_t = count(|item| matches!(item, syntax::TypeSpecifier::UInt128T));

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
            || float16 > 1
            || int128 > 1
            || int128_t > 1
            || uint128_t > 1
        {
            return self.fail("CCC2217", span, "invalid repetition of type specifiers");
        }
        let total = specifiers.len();
        let builtin = if float16 == 1 && total == 1 {
            BuiltinType::Float16
        } else if int128_t == 1 && total == 1 {
            BuiltinType::Int128
        } else if uint128_t == 1 && total == 1 {
            BuiltinType::UnsignedInt128
        } else if int128 == 1 && total == int128 + signed + unsigned {
            if unsigned == 1 {
                BuiltinType::UnsignedInt128
            } else {
                BuiltinType::Int128
            }
        } else if void == 1 && total == 1 {
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
        Ok((
            QualifiedType::unqualified(self.types.builtin(builtin)),
            Vec::new(),
        ))
    }

    fn resolve_type_name_with_bounds(
        &mut self,
        type_name: &syntax::TypeName,
    ) -> AnalysisResult<(QualifiedType, Vec<FullTypedVariableLengthBound>)> {
        let info = self.resolve_declaration_specifiers(&type_name.specifiers)?;
        self.reject_packed_attribute(&info.attributes, type_name.specifiers.span)?;
        self.reject_transparent_union_attribute(&info.attributes, type_name.span, "a type name")?;
        self.reject_function_inlining_attribute(&info.attributes, type_name.span, "a type name")?;
        self.reject_alignment_specifier(&info, type_name.span, "a type name")?;
        if info.storage.is_some()
            || info.thread_local
            || info.properties != FunctionProperties::default()
        {
            return self.fail(
                "CCC2219",
                type_name.span,
                "a type name cannot contain storage-class or function specifiers",
            );
        }
        match &type_name.declarator {
            Some(declarator) => {
                let resolved = self.resolve_declarator(info.base, declarator)?;
                self.reject_function_inlining_attribute(
                    &resolved.attributes,
                    type_name.span,
                    "a type name",
                )?;
                if resolved.name.is_some() {
                    return self.fail(
                        "CCC2220",
                        type_name.span,
                        "a type name cannot declare an identifier",
                    );
                }
                let mut bounds = info.variable_length_bounds;
                bounds.extend(resolved.variable_length_bounds);
                Ok((resolved.ty, bounds))
            }
            None => Ok((info.base, info.variable_length_bounds)),
        }
    }

    fn resolve_declarator(
        &mut self,
        ty: QualifiedType,
        declarator: &syntax::Declarator,
    ) -> AnalysisResult<ResolvedDeclarator> {
        self.resolve_declarator_in_context(ty, declarator, false)
    }

    fn resolve_declarator_in_context(
        &mut self,
        mut ty: QualifiedType,
        declarator: &syntax::Declarator,
        prototype_parameter: bool,
    ) -> AnalysisResult<ResolvedDeclarator> {
        // Prototype array rules follow the parameter's declarator structure.
        // Declarators reached through specifiers or bound expressions enter
        // through `resolve_declarator` and therefore do not inherit them.
        let mut attributes = self.validate_attributes(&declarator.attributes)?;
        for pointer in &declarator.pointers {
            attributes.extend(self.validate_attributes(&pointer.attributes)?);
            let pointer_ty = self.types.pointer(ty);
            ty = QualifiedType::new(pointer_ty, qualifiers(&pointer.qualifiers));
        }
        self.reject_packed_attribute(&attributes, declarator.span)?;
        let mut context = DeclaratorContext::new(prototype_parameter);
        let (name, name_span, ty) =
            self.resolve_direct_declarator(ty, &declarator.direct, &mut context)?;
        let ty = self.apply_declarator_type_attributes(ty, &attributes, declarator.span)?;
        Ok(ResolvedDeclarator {
            name,
            name_span,
            ty,
            parameters: context.parameters,
            parameter_list_span: context.parameter_list_span,
            parameter_scope: context.parameter_scope,
            variable_length_bounds: context.variable_length_bounds,
            attributes,
        })
    }

    fn resolve_direct_declarator(
        &mut self,
        ty: QualifiedType,
        direct: &syntax::DirectDeclarator,
        context: &mut DeclaratorContext,
    ) -> AnalysisResult<(Option<String>, Span, QualifiedType)> {
        match direct {
            syntax::DirectDeclarator::Identifier(identifier) => {
                Ok((Some(identifier.name.clone()), identifier.span, ty))
            }
            syntax::DirectDeclarator::Abstract(span) => Ok((None, *span, ty)),
            syntax::DirectDeclarator::Parenthesized(inner, _) => {
                let resolved =
                    self.resolve_declarator_in_context(ty, inner, context.prototype_parameter)?;
                if resolved.parameter_list_span.is_some() {
                    context.parameters = resolved.parameters;
                    context.parameter_list_span = resolved.parameter_list_span;
                    context.parameter_scope = resolved.parameter_scope;
                }
                context
                    .variable_length_bounds
                    .extend(resolved.variable_length_bounds);
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
                if self.type_contains_flexible_array_member(ty.ty) {
                    return self.fail(
                        "CCC2370",
                        *span,
                        "an array element cannot contain a flexible array member",
                    );
                }
                let length = match size {
                    syntax::ArraySize::Unspecified => ArrayLength::Incomplete,
                    syntax::ArraySize::Star => {
                        if !context.prototype_parameter {
                            return self.fail(
                                "CCC2223",
                                *span,
                                "`[*]` is only permitted in function prototype scope",
                            );
                        }
                        ArrayLength::UnspecifiedVariable(self.types.fresh_variable_length())
                    }
                    syntax::ArraySize::Expression(expression) => {
                        let (typed, constant) =
                            self.analyze_integer_constant_candidate(expression)?;
                        match constant {
                            Some(value) if value > 0 => ArrayLength::Constant(value as u64),
                            Some(0) if self.config.language.mode.accepts_gnu_extensions() => {
                                ArrayLength::Constant(0)
                            }
                            Some(_) => {
                                return self.fail(
                                    "CCC2223",
                                    expression.span,
                                    "an array bound must be greater than zero",
                                );
                            }
                            None => {
                                if context.prototype_parameter {
                                    ArrayLength::UnspecifiedVariable(
                                        self.types.fresh_variable_length(),
                                    )
                                } else {
                                    if self.function.is_none() && self.parameter_scope_depth == 0 {
                                        return self.fail(
                                            "CCC2223",
                                            expression.span,
                                            "a file-scope array bound must be constant",
                                        );
                                    }
                                    let id = self.types.fresh_variable_length();
                                    context.variable_length_bounds.push(
                                        FullTypedVariableLengthBound {
                                            id,
                                            expression: typed,
                                        },
                                    );
                                    ArrayLength::Variable(id)
                                }
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
                if self.has_runtime_and_zero_array_dimensions(array.ty) {
                    return self.fail(
                        "CCC2456",
                        *span,
                        "an array type cannot combine a runtime bound with a zero-length dimension",
                    );
                }
                self.resolve_direct_declarator(array, inner, context)
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
                let is_prototype_scope = self.definition_parameter_list != Some(*span);
                let (mut resolved_parameters, parameter_scope) =
                    self.resolve_parameter_list(parameters, is_prototype_scope)?;
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
                let signature = if !has_parameter_type_list
                    && resolved_parameters.is_empty()
                    && is_prototype_scope
                {
                    FunctionType::unspecified(ty)
                } else if *variadic {
                    FunctionType::variadic(ty, parameter_types)
                } else {
                    FunctionType::prototype(ty, parameter_types)
                };
                let function_ty = QualifiedType::unqualified(self.types.function_type(signature));
                if context.parameter_list_span.is_none() {
                    context.parameters = resolved_parameters;
                    context.parameter_list_span = Some(*span);
                    context.parameter_scope = (!is_prototype_scope).then_some(parameter_scope);
                }
                self.resolve_direct_declarator(function_ty, inner, context)
            }
        }
    }

    fn resolve_parameter_list(
        &mut self,
        parameters: &[syntax::ParameterDeclaration],
        is_prototype_scope: bool,
    ) -> AnalysisResult<(Vec<ResolvedParameter>, DetachedSemanticScope)> {
        self.parameter_scope_depth += 1;
        // Declarator parameter scopes do not participate in statement-level
        // control-flow paths, even when resolved inside a function body.
        self.scopes.push();
        let resolved = (|| {
            let mut resolved = Vec::with_capacity(parameters.len());
            for (index, parameter) in parameters.iter().enumerate() {
                let parameter = self.resolve_parameter(parameter, is_prototype_scope)?;
                if let Some(name) = &parameter.name {
                    let local = FullLocalId(
                        u32::try_from(index).expect("parameter identifier space exhausted"),
                    );
                    self.bind_current(
                        name.clone(),
                        OrdinarySymbol::TemporaryParameter(
                            local,
                            parameter.ty,
                            parameter.addressable,
                        ),
                        parameter.span,
                    )?;
                }
                resolved.push(parameter);
            }
            Ok(resolved)
        })();
        let scope = self.scopes.pop_detached();
        self.parameter_scope_depth -= 1;
        resolved.map(|resolved| (resolved, scope))
    }

    fn resolve_parameter(
        &mut self,
        parameter: &syntax::ParameterDeclaration,
        is_prototype_scope: bool,
    ) -> AnalysisResult<ResolvedParameter> {
        let info = self.resolve_declaration_specifiers(&parameter.specifiers)?;
        self.reject_weak_attribute(&info.attributes, parameter.span, "a parameter")?;
        self.reject_packed_attribute(&info.attributes, parameter.span)?;
        self.reject_function_inlining_attribute(&info.attributes, parameter.span, "a parameter")?;
        self.reject_transparent_union_attribute(
            &info.attributes,
            parameter.span,
            "a parameter declaration",
        )?;
        self.reject_alignment_specifier(&info, parameter.span, "a parameter")?;
        if info.thread_local || !matches!(info.storage, None | Some(syntax::StorageClass::Register))
        {
            return self.fail(
                "CCC2227",
                parameter.span,
                "a parameter may only use the `register` storage class",
            );
        }
        let register = info.storage == Some(syntax::StorageClass::Register);
        let (name, mut ty, variable_length_bounds, span) =
            if let Some(declarator) = &parameter.declarator {
                let resolved =
                    self.resolve_declarator_in_context(info.base, declarator, is_prototype_scope)?;
                self.reject_weak_attribute(&resolved.attributes, parameter.span, "a parameter")?;
                self.reject_function_inlining_attribute(
                    &resolved.attributes,
                    parameter.span,
                    "a parameter",
                )?;
                (
                    resolved.name,
                    resolved.ty,
                    {
                        let mut bounds = info.variable_length_bounds.clone();
                        bounds.extend(resolved.variable_length_bounds);
                        bounds
                    },
                    resolved.name_span,
                )
            } else {
                (
                    None,
                    info.base,
                    info.variable_length_bounds.clone(),
                    parameter.span,
                )
            };
        let declared_kind = self.types.try_kind(ty.ty).cloned();
        if matches!(&declared_kind, Some(TypeKind::Array(_)))
            && self.type_contains_flexible_array_member(ty.ty)
        {
            return self.fail(
                "CCC2370",
                parameter.span,
                "an array parameter element cannot contain a flexible array member",
            );
        }
        let va_start_restriction = if register {
            Some("it has `register` storage class")
        } else if matches!(
            &declared_kind,
            Some(TypeKind::Array(_) | TypeKind::Function(_))
        ) {
            Some("it is declared with array or function type")
        } else if self.types.builtin_type(ty.ty) == Some(BuiltinType::Float)
            || self.types.is_integer(ty.ty) && self.integer_promotion_changes_type(ty.ty)
        {
            Some("its type is changed by the default argument promotions")
        } else {
            None
        };
        ty = match declared_kind {
            Some(TypeKind::Array(array)) => {
                let pointer = self.types.pointer(array.element);
                QualifiedType::new(pointer, parameter_array_qualifiers(&parameter.declarator))
            }
            Some(TypeKind::Function(_)) => QualifiedType::unqualified(self.types.pointer(ty)),
            _ => ty,
        };
        Ok(ResolvedParameter {
            name,
            ty,
            variable_length_bounds,
            span,
            addressable: !register,
            va_start_restriction,
        })
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
        let existing = tag.as_deref().and_then(|name| {
            if specifier.items.is_some() {
                self.current_tag(name)
            } else {
                self.lookup_tag(name)
            }
        });
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
        self.reject_weak_attribute(&record_attributes, specifier.span, "a record type")?;
        self.reject_function_inlining_attribute(
            &record_attributes,
            specifier.span,
            "a record type",
        )?;
        self.reject_transparent_union_attribute(
            &record_attributes,
            specifier.span,
            "a record specifier",
        )?;
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
            .any(|attribute| attribute_has_name(attribute, "packed"))
        {
            self.packing.current.combine(PackingPolicy::PACKED)
        } else {
            self.packing.current
        };
        let mut fields = Vec::new();
        let mut field_names = HashSet::new();
        let mut flexible_members = Vec::new();
        for item in items {
            match item {
                syntax::RecordItem::Pragma(pragma) => self.handle_pragma(pragma)?,
                syntax::RecordItem::StaticAssert(assertion) => {
                    let _ = self.analyze_static_assert(assertion)?;
                }
                syntax::RecordItem::Declaration(declaration) => {
                    let info = self.resolve_declaration_specifiers(&declaration.specifiers)?;
                    self.reject_weak_attribute(
                        &info.attributes,
                        declaration.span,
                        "a record member",
                    )?;
                    self.reject_function_inlining_attribute(
                        &info.attributes,
                        declaration.span,
                        "a record member",
                    )?;
                    self.reject_packed_attribute(&info.attributes, declaration.span)?;
                    self.reject_transparent_union_attribute(
                        &info.attributes,
                        declaration.span,
                        "a record member",
                    )?;
                    if info.storage.is_some()
                        || info.thread_local
                        || info.properties != FunctionProperties::default()
                    {
                        return self.fail(
                            "CCC2232",
                            declaration.span,
                            "a record member cannot have storage-class or function specifiers",
                        );
                    }
                    if declaration.declarators.is_empty() {
                        if !info.variable_length_bounds.is_empty()
                            || self.is_variably_modified(info.base.ty)
                        {
                            return self.fail(
                                "CCC2235",
                                declaration.span,
                                "a record member cannot have variably modified type",
                            );
                        }
                        if !matches!(self.types.try_kind(info.base.ty), Some(TypeKind::Record(_))) {
                            return self.fail(
                                "CCC2233",
                                declaration.span,
                                "an unnamed record member must have struct or union type",
                            );
                        }
                        let requested_alignment = self.object_requested_alignment(
                            info.base,
                            info.requested_alignment,
                            &info.attributes,
                            declaration.span,
                        )?;
                        fields.push(
                            Field::anonymous(info.base)
                                .with_requested_alignment(requested_alignment),
                        );
                        continue;
                    }
                    for member in &declaration.declarators {
                        let member_attributes = self.validate_attributes(&member.attributes)?;
                        self.reject_weak_attribute(
                            &member_attributes,
                            member.span,
                            "a record member",
                        )?;
                        self.reject_function_inlining_attribute(
                            &member_attributes,
                            member.span,
                            "a record member",
                        )?;
                        self.reject_packed_attribute(&member_attributes, member.span)?;
                        self.reject_transparent_union_attribute(
                            &member_attributes,
                            member.span,
                            "a record member",
                        )?;
                        let (name, field_ty, has_variable_length_bounds, declarator_attributes) =
                            if let Some(declarator) = &member.declarator {
                                let resolved = self.resolve_declarator(info.base, declarator)?;
                                self.reject_weak_attribute(
                                    &resolved.attributes,
                                    member.span,
                                    "a record member",
                                )?;
                                self.reject_function_inlining_attribute(
                                    &resolved.attributes,
                                    member.span,
                                    "a record member",
                                )?;
                                (
                                    resolved.name,
                                    resolved.ty,
                                    !info.variable_length_bounds.is_empty()
                                        || !resolved.variable_length_bounds.is_empty(),
                                    resolved.attributes,
                                )
                            } else {
                                (
                                    None,
                                    info.base,
                                    !info.variable_length_bounds.is_empty(),
                                    Vec::new(),
                                )
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
                        if has_variable_length_bounds || self.is_variably_modified(field_ty.ty) {
                            return self.fail(
                                "CCC2235",
                                member.span,
                                "a record member cannot have variably modified type",
                            );
                        }
                        let flexible = matches!(
                            self.types.try_kind(field_ty.ty),
                            Some(TypeKind::Array(ArrayType {
                                length: ArrayLength::Incomplete,
                                ..
                            }))
                        );
                        if kind == RecordKind::Struct
                            && self.type_contains_flexible_array_member(field_ty.ty)
                        {
                            return self.fail(
                                "CCC2370",
                                member.span,
                                "a structure member cannot contain a flexible array member",
                            );
                        }
                        let mut alignment_attributes = info.attributes.clone();
                        alignment_attributes.extend(declarator_attributes);
                        alignment_attributes.extend(member_attributes);
                        let field = if let Some(width) = &member.bit_width {
                            if field_ty.qualifiers.contains(TypeQualifiers::ATOMIC) {
                                return self.fail(
                                    "CCC2453",
                                    member.span,
                                    "atomic bit-fields are not enabled",
                                );
                            }
                            if info.has_alignment_specifier {
                                return self.fail(
                                    "CCC2437",
                                    member.span,
                                    "an alignment specifier cannot be applied to a bit-field",
                                );
                            }
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
                            let requested_alignment = self.object_requested_alignment(
                                field_ty,
                                info.requested_alignment,
                                &alignment_attributes,
                                member.span,
                            )?;
                            self.reject_packed_atomic_field(
                                field_ty,
                                requested_alignment,
                                applied_packing,
                                member.span,
                            )?;
                            Field::new(name, field_ty).with_requested_alignment(requested_alignment)
                        };
                        if flexible {
                            flexible_members.push((fields.len(), member.span));
                        }
                        fields.push(field);
                    }
                }
            }
        }
        for (index, span) in flexible_members {
            let field = &fields[index];
            if kind != RecordKind::Struct
                || index + 1 != fields.len()
                || field.name.is_none()
                || !fields[..index]
                    .iter()
                    .any(|field| self.field_contributes_named_member(field))
            {
                return self.fail(
                    "CCC2370",
                    span,
                    "a flexible array must be the named final member of a structure with another named member",
                );
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
        let existing = tag.as_deref().and_then(|name| {
            if specifier.enumerators.is_some() {
                self.current_tag(name)
            } else {
                self.lookup_tag(name)
            }
        });
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
        let enum_attributes = self.validate_attributes(&specifier.attributes)?;
        self.reject_weak_attribute(&enum_attributes, specifier.span, "an enum type")?;
        self.reject_function_inlining_attribute(&enum_attributes, specifier.span, "an enum type")?;
        self.reject_packed_attribute(&enum_attributes, specifier.span)?;
        self.reject_transparent_union_attribute(
            &enum_attributes,
            specifier.span,
            "an enum specifier",
        )?;
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
            let enumerator_attributes = self.validate_attributes(&enumerator.attributes)?;
            self.reject_weak_attribute(&enumerator_attributes, enumerator.span, "an enumerator")?;
            self.reject_function_inlining_attribute(
                &enumerator_attributes,
                enumerator.span,
                "an enumerator",
            )?;
            self.reject_packed_attribute(&enumerator_attributes, enumerator.span)?;
            self.reject_transparent_union_attribute(
                &enumerator_attributes,
                enumerator.span,
                "an enumerator",
            )?;
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
        self.reject_tls_model_attribute(&attributes, span)?;
        self.reject_weak_attribute(&attributes, span, "a typedef")?;
        self.reject_function_inlining_attribute(&attributes, span, "a typedef")?;
        if matches!(self.types.try_kind(ty.ty), Some(TypeKind::Array(_)))
            && self.type_contains_flexible_array_member(ty.ty)
        {
            return self.fail(
                "CCC2370",
                span,
                "an array element cannot contain a flexible array member",
            );
        }
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
            variable_length_bounds: Vec::new(),
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
        mut properties: FunctionProperties,
        attributes: Vec<FullTypedAttribute>,
        asm_label: Option<FullTypedAsmLabel>,
        span: Span,
    ) -> AnalysisResult<FullFunctionId> {
        self.reject_tls_model_attribute(&attributes, span)?;
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
        properties.no_return |= attributes
            .iter()
            .any(|attribute| attribute_has_name(attribute, "noreturn"));
        properties.always_inline |= attributes
            .iter()
            .any(|attribute| attribute_has_name(attribute, "always_inline"));
        properties.no_inline |= attributes
            .iter()
            .any(|attribute| attribute_has_name(attribute, "noinline"));
        if properties.always_inline && properties.no_inline {
            return self.fail(
                "CCC2457",
                span,
                format!(
                    "function `{name}` cannot be declared with both `always_inline` and `noinline`"
                ),
            );
        }
        properties.returns_twice |= attributes
            .iter()
            .any(|attribute| attribute_has_name(attribute, "returns_twice"));
        properties.returns_twice |=
            storage != Some(syntax::StorageClass::Static) && is_known_returns_twice_function(&name);
        let declared_binding = attributes
            .iter()
            .any(|attribute| attribute_has_name(attribute, "weak"))
            .then_some(SymbolBinding::Weak);
        if declared_binding.is_some() && storage == Some(syntax::StorageClass::Static) {
            return self.fail(
                "CCC2423",
                span,
                format!("weak declaration of function `{name}` must have external linkage"),
            );
        }
        let declared_visibility = self.function_visibility(&attributes, span)?;
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
                if declared_binding.is_some() && existing_linkage != Linkage::External {
                    return self.fail(
                        "CCC2423",
                        span,
                        format!(
                            "weak declaration of function `{name}` conflicts with its internal linkage"
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
                if let (Some(existing), Some(incoming)) =
                    (&self.functions[id.0 as usize].asm_label, asm_label.as_ref())
                    && existing.symbol != incoming.symbol
                {
                    let existing_symbol = existing.symbol.clone();
                    let incoming_symbol = incoming.symbol.clone();
                    return self.fail(
                        "CCC2419",
                        span,
                        format!(
                            "function `{name}` has conflicting assembly labels `{existing_symbol}` and `{incoming_symbol}`"
                        ),
                    );
                }
                let existing_properties = self.functions[id.0 as usize].properties;
                if (existing_properties.always_inline || properties.always_inline)
                    && (existing_properties.no_inline || properties.no_inline)
                {
                    return self.fail(
                        "CCC2457",
                        span,
                        format!(
                            "function `{name}` has conflicting `always_inline` and `noinline` declarations"
                        ),
                    );
                }
                let function = &mut self.functions[id.0 as usize];
                function.signature = composite;
                function.properties.inline |= properties.inline;
                function.properties.always_inline |= properties.always_inline;
                function.properties.no_inline |= properties.no_inline;
                function.properties.no_return |= properties.no_return;
                function.properties.returns_twice |= properties.returns_twice;
                if let Some(binding) = declared_binding {
                    function.binding = binding;
                }
                if let Some(asm_label) = asm_label {
                    function.asm_label = Some(asm_label);
                }
                if let Some(visibility) = declared_visibility {
                    function.visibility = visibility;
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
            binding: declared_binding.unwrap_or_default(),
            visibility: declared_visibility.unwrap_or_default(),
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
        thread_local: bool,
        attributes: Vec<FullTypedAttribute>,
        standard_alignment: Option<u64>,
        asm_label: Option<FullTypedAsmLabel>,
        initializer: Option<&syntax::Initializer>,
        span: Span,
    ) -> AnalysisResult<GlobalId> {
        self.reject_function_inlining_attribute(&attributes, span, "an object declaration")?;
        if !thread_local {
            self.reject_tls_model_attribute(&attributes, span)?;
        }
        let is_definition = initializer.is_some();
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
        self.reject_unavailable_thread_storage(thread_local, span)?;
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
            if self.initializer_references_thread_storage(&typed) {
                return self.fail(
                    "CCC2344",
                    span,
                    "a file-scope initializer cannot contain the address of a thread-local object",
                );
            }
            Some(typed)
        } else {
            None
        };
        let requested_alignment =
            self.object_requested_alignment(ty, standard_alignment, &attributes, span)?;
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
        let duration = if thread_local {
            StorageDuration::Thread
        } else {
            StorageDuration::Static
        };
        let semantic_storage = if thread_local {
            SemanticStorageClass::ThreadLocal
        } else if storage == Some(syntax::StorageClass::Static) {
            SemanticStorageClass::Static
        } else {
            SemanticStorageClass::Extern
        };
        let declared_binding = attributes
            .iter()
            .any(|attribute| attribute_has_name(attribute, "weak"))
            .then_some(SymbolBinding::Weak);
        if declared_binding.is_some() && linkage != Linkage::External {
            return self.fail(
                "CCC2423",
                span,
                format!("weak declaration of object `{name}` must have external linkage"),
            );
        }

        if let Some(existing) = self.lookup_file_ordinary(&name).cloned() {
            if let OrdinarySymbol::Global(id, existing_ty) = existing {
                if self.globals[id.0 as usize].duration != duration {
                    return self.fail(
                        "CCC2374",
                        span,
                        format!(
                            "object `{name}` is redeclared with different thread-local storage duration"
                        ),
                    );
                }
                self.register_global_standard_alignment(
                    id,
                    standard_alignment,
                    is_definition,
                    span,
                )?;
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
                if declared_binding.is_some() && existing_linkage != Linkage::External {
                    return self.fail(
                        "CCC2423",
                        span,
                        format!(
                            "weak declaration of object `{name}` conflicts with its internal linkage"
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
                if let (Some(existing), Some(incoming)) =
                    (&self.globals[id.0 as usize].asm_label, asm_label.as_ref())
                    && existing.symbol != incoming.symbol
                {
                    let existing_symbol = existing.symbol.clone();
                    let incoming_symbol = incoming.symbol.clone();
                    return self.fail(
                        "CCC2419",
                        span,
                        format!(
                            "object `{name}` has conflicting assembly labels `{existing_symbol}` and `{incoming_symbol}`"
                        ),
                    );
                }
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
                if let Some(asm_label) = asm_label {
                    global.emission.symbol_name = asm_label.symbol.clone();
                    global.emission.symbol_name_is_exact = true;
                    global.asm_label = Some(asm_label);
                }
                if let Some(binding) = declared_binding {
                    global.emission.binding = binding;
                }
                if global.emission.binding == SymbolBinding::Weak
                    && global.emission.definition == ObjectDefinitionPolicy::TentativeCommon
                {
                    global.tentative = false;
                    global.emission.definition = ObjectDefinitionPolicy::Definition;
                }
                global.emission.requested_alignment =
                    strongest_alignment(global.emission.requested_alignment, requested_alignment);
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
        self.register_global_standard_alignment(id, standard_alignment, is_definition, span)?;
        let symbol_name = asm_label
            .as_ref()
            .map_or_else(|| name.clone(), |label| label.symbol.clone());
        let mut emission = GlobalEmission {
            symbol_name,
            symbol_name_is_exact: asm_label.is_some(),
            binding: declared_binding.unwrap_or_default(),
            visibility: SymbolVisibility::Default,
            section: None,
            requested_alignment,
            tls: (duration == StorageDuration::Thread).then_some(TlsModel::GeneralDynamic),
            definition,
        };
        self.apply_emission_attributes(&mut emission, &attributes, span)?;
        let tentative = tentative && emission.binding != SymbolBinding::Weak;
        if emission.binding == SymbolBinding::Weak
            && emission.definition == ObjectDefinitionPolicy::TentativeCommon
        {
            emission.definition = ObjectDefinitionPolicy::Definition;
        }
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

    fn register_global_standard_alignment(
        &mut self,
        id: GlobalId,
        incoming: Option<u64>,
        definition: bool,
        span: Span,
    ) -> AnalysisResult<()> {
        let current = self
            .global_standard_alignments
            .get(&id)
            .copied()
            .unwrap_or_default();
        if let (Some(existing), Some(incoming)) = (current.explicit, incoming)
            && existing != incoming
        {
            return self.fail(
                "CCC2437",
                span,
                "object redeclarations specify different standard alignments",
            );
        }
        if let Some(definition_alignment) = current.definition {
            match (definition_alignment, incoming) {
                (None, Some(_)) => {
                    return self.fail(
                        "CCC2437",
                        span,
                        "an aligned declaration follows a definition without an alignment specifier",
                    );
                }
                (Some(expected), Some(incoming)) if expected != incoming => {
                    return self.fail(
                        "CCC2437",
                        span,
                        "object declaration alignment differs from its definition",
                    );
                }
                _ => {}
            }
        }
        if definition
            && let Some(expected) = current.explicit
            && incoming != Some(expected)
        {
            return self.fail(
                "CCC2437",
                span,
                "an object definition must repeat the standard alignment from an earlier declaration",
            );
        }
        self.global_standard_alignments.insert(
            id,
            GlobalStandardAlignment {
                explicit: current.explicit.or(incoming),
                definition: if definition {
                    Some(incoming)
                } else {
                    current.definition
                },
            },
        );
        Ok(())
    }

    fn analyze_block_declaration(
        &mut self,
        declaration: &syntax::Declaration,
    ) -> AnalysisResult<Vec<FullTypedBlockItem>> {
        let info = self.resolve_declaration_specifiers(&declaration.specifiers)?;
        self.reject_packed_attribute(&info.attributes, declaration.span)?;
        if declaration.declarators.is_empty() {
            self.reject_alignment_specifier(&info, declaration.span, "a type declaration")?;
            self.reject_function_inlining_attribute(
                &info.attributes,
                declaration.span,
                "a type declaration",
            )?;
        }
        let mut output = Vec::new();
        for (declarator_index, init) in declaration.declarators.iter().enumerate() {
            let mut resolved = self.resolve_declarator(info.base, &init.declarator)?;
            if declarator_index == 0 {
                let mut bounds = info.variable_length_bounds.clone();
                bounds.extend(resolved.variable_length_bounds);
                resolved.variable_length_bounds = bounds;
            }
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
            self.reject_packed_attribute(&attributes, init.span)?;
            if info.storage == Some(syntax::StorageClass::Typedef) {
                if info.thread_local {
                    return self.fail(
                        "CCC2374",
                        init.span,
                        "a typedef cannot have thread-local storage duration",
                    );
                }
                self.reject_alignment_specifier(&info, init.span, "a typedef")?;
                self.reject_transparent_union_attribute(
                    &attributes,
                    init.span,
                    "a block-scope typedef",
                )?;
                self.reject_weak_attribute(&attributes, init.span, "a typedef")?;
                self.reject_function_inlining_attribute(&attributes, init.span, "a typedef")?;
                if init.initializer.is_some() || init.asm_label.is_some() {
                    return self.fail(
                        "CCC2255",
                        init.span,
                        "a typedef cannot have an initializer or assembly label",
                    );
                }
                let id = TypedefId(self.typedefs.len() as u32);
                let ty = self.apply_typedef_alignment(
                    resolved.ty,
                    &attributes,
                    declaration.declarators.len() == 1
                        && defines_inline_anonymous_record(&declaration.specifiers),
                    init.span,
                )?;
                let variably_modified =
                    !resolved.variable_length_bounds.is_empty() || self.is_variably_modified(ty.ty);
                let typed = FullTypedTypedef {
                    id,
                    name: name.clone(),
                    ty,
                    variable_length_bounds: resolved.variable_length_bounds,
                    attributes,
                    span: init.span,
                };
                self.bind_current(name, OrdinarySymbol::Typedef(id, typed.ty), init.span)?;
                self.typedefs.push(typed.clone());
                if variably_modified {
                    let function = self
                        .function
                        .as_mut()
                        .expect("block-scope variably modified typedefs occur inside functions");
                    function
                        .active_variably_modified_path
                        .push(VariablyModifiedScopeEntry::Typedef(id));
                    function.has_variably_modified_declaration = true;
                }
                output.push(FullTypedBlockItem::Typedef(Box::new(typed)));
                continue;
            }
            if self.types.function_signature(resolved.ty.ty).is_some() {
                if info.thread_local {
                    return self.fail(
                        "CCC2374",
                        init.span,
                        "a function cannot have thread-local storage duration",
                    );
                }
                self.reject_transparent_union_attribute(
                    &attributes,
                    init.span,
                    "a block-scope function declaration",
                )?;
                self.reject_alignment_specifier(&info, init.span, "a function")?;
                if !resolved.variable_length_bounds.is_empty()
                    || self.is_variably_modified(resolved.ty.ty)
                {
                    return self.fail(
                        "CCC2418",
                        init.span,
                        "a block-scope function declaration cannot have variably modified type",
                    );
                }
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
            self.reject_function_inlining_attribute(
                &attributes,
                init.span,
                "an object declaration",
            )?;
            self.reject_unavailable_thread_storage(info.thread_local, init.span)?;
            if info.storage == Some(syntax::StorageClass::Extern) {
                self.reject_transparent_union_attribute(
                    &attributes,
                    init.span,
                    "an object declaration",
                )?;
                if init.initializer.is_some() {
                    return self.fail(
                        "CCC2256",
                        init.span,
                        "a block-scope extern declaration cannot have an initializer",
                    );
                }
                if !resolved.variable_length_bounds.is_empty()
                    || self.is_variably_modified(resolved.ty.ty)
                {
                    return self.fail(
                        "CCC2415",
                        init.span,
                        "a block-scope extern declaration cannot have variably modified type",
                    );
                }
                let asm_label = self.resolve_asm_label(init.asm_label.as_ref())?;
                let id = self.declare_global(
                    name.clone(),
                    resolved.ty,
                    info.storage,
                    info.thread_local,
                    attributes,
                    info.requested_alignment,
                    asm_label,
                    None,
                    init.span,
                )?;
                let ty = self.globals[id.0 as usize].ty;
                self.bind_current(name, OrdinarySymbol::Global(id, ty), init.span)?;
                output.push(FullTypedBlockItem::ExternalObject(id));
                continue;
            }
            self.reject_weak_attribute(
                &attributes,
                init.span,
                "an automatic or block-scope static object",
            )?;
            self.reject_transparent_union_attribute(
                &attributes,
                init.span,
                "an object declaration",
            )?;
            if !info.thread_local {
                self.reject_tls_model_attribute(&attributes, init.span)?;
            }
            if init.asm_label.is_some() {
                return self.fail(
                    "CCC2257",
                    init.span,
                    "an automatic or static local cannot have an assembly label",
                );
            }
            self.validate_object_type(resolved.ty, init.span, init.initializer.is_none())?;
            if info.thread_local && info.storage != Some(syntax::StorageClass::Static) {
                return self.fail(
                    "CCC2374",
                    init.span,
                    "a block-scope thread-local object must also be declared `static` or `extern`",
                );
            }
            if self.requires_runtime_sized_storage(resolved.ty.ty)
                && info.storage == Some(syntax::StorageClass::Static)
            {
                return self.fail(
                    "CCC2258",
                    init.span,
                    "a variable-length array object must have automatic storage duration",
                );
            }
            let local = self.fresh_local();
            self.bind_current(
                name.clone(),
                OrdinarySymbol::Local(local, resolved.ty),
                init.span,
            )?;
            let (storage, duration) = match info.storage {
                Some(syntax::StorageClass::Static) if info.thread_local => {
                    (SemanticStorageClass::ThreadLocal, StorageDuration::Thread)
                }
                Some(syntax::StorageClass::Static) => {
                    (SemanticStorageClass::Static, StorageDuration::Static)
                }
                Some(syntax::StorageClass::Register) => {
                    (SemanticStorageClass::Register, StorageDuration::Automatic)
                }
                None | Some(syntax::StorageClass::Auto) => {
                    (SemanticStorageClass::Automatic, StorageDuration::Automatic)
                }
                Some(syntax::StorageClass::Extern | syntax::StorageClass::Typedef) => {
                    unreachable!("handled above")
                }
                Some(syntax::StorageClass::ThreadLocal | syntax::StorageClass::GnuThreadLocal) => {
                    unreachable!("thread-local specifiers are tracked separately")
                }
            };
            if duration != StorageDuration::Automatic {
                self.function
                    .as_mut()
                    .expect("block objects occur inside a function")
                    .static_duration_locals
                    .insert(local, duration);
            }
            if storage == SemanticStorageClass::Register {
                self.reject_alignment_specifier(
                    &info,
                    init.span,
                    "an object declared with `register` storage class",
                )?;
                self.function
                    .as_mut()
                    .expect("block objects occur inside a function")
                    .unaddressable_locals
                    .insert(local);
            }
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
                    if duration != StorageDuration::Automatic
                        && self.initializer_references_thread_storage(&typed)
                    {
                        return self.fail(
                            "CCC2367",
                            init.span,
                            "a static- or thread-duration initializer cannot contain the address of a thread-local object",
                        );
                    }
                    (Some(typed), completed)
                }
                None => (None, resolved.ty),
            };
            self.validate_object_type(completed_ty, init.span, true)?;
            let requested_alignment = self.object_requested_alignment(
                completed_ty,
                info.requested_alignment,
                &attributes,
                init.span,
            )?;
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
                    symbol_name_is_exact: false,
                    binding: SymbolBinding::Strong,
                    visibility: SymbolVisibility::Internal,
                    section: None,
                    requested_alignment,
                    tls: (duration == StorageDuration::Thread).then_some(TlsModel::GeneralDynamic),
                    definition: ObjectDefinitionPolicy::Definition,
                };
                self.apply_emission_attributes(&mut emission, &attributes, init.span)?;
                Some(emission)
            };
            if duration == StorageDuration::Automatic
                && (!resolved.variable_length_bounds.is_empty()
                    || self.is_variably_modified(completed_ty.ty))
            {
                let function = self
                    .function
                    .as_mut()
                    .expect("variably modified locals occur inside a function");
                function
                    .active_variably_modified_path
                    .push(VariablyModifiedScopeEntry::Object(local));
                function.has_variably_modified_declaration = true;
            }
            output.push(FullTypedBlockItem::Declaration(Box::new(
                FullTypedLocalDeclaration {
                    local,
                    name,
                    ty: completed_ty,
                    storage,
                    duration,
                    variable_length_bounds: resolved.variable_length_bounds,
                    initializer,
                    attributes,
                    requested_alignment,
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
                    let attributes = self.validate_attributes(attributes)?;
                    self.reject_weak_attribute(&attributes, statement.span, "a statement label")?;
                    self.reject_function_inlining_attribute(
                        &attributes,
                        statement.span,
                        "a statement label",
                    )?;
                    self.reject_packed_attribute(&attributes, statement.span)?;
                    self.reject_transparent_union_attribute(
                        &attributes,
                        statement.span,
                        "a statement label",
                    )?;
                    let variably_modified_path = self
                        .function
                        .as_ref()
                        .expect("labels only occur inside functions")
                        .active_variably_modified_path
                        .clone();
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
                    self.function
                        .as_mut()
                        .expect("labels only occur inside functions")
                        .variably_modified_label_paths
                        .insert(id, (label.span, variably_modified_path));
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
                    let controlling_type = self
                        .function
                        .as_ref()
                        .and_then(|function| function.switches.last())
                        .and_then(|switch| switch.controlling_type)
                        .ok_or_else(|| {
                            self.diagnostics.push(
                                Diagnostic::error(
                                    "CCC2260",
                                    "a `case` label must be inside a switch statement",
                                )
                                .with_primary(statement.span, "case outside switch"),
                            );
                        })?;
                    let value = self.evaluate_switch_case(value, controlling_type)?;
                    self.reject_switch_variably_modified_ingress(statement.span)?;
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
                    self.reject_switch_variably_modified_ingress(statement.span)?;
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
                    let entry_variably_modified_path = self
                        .function
                        .as_ref()
                        .expect("switches only occur inside functions")
                        .active_variably_modified_path
                        .clone();
                    self.function
                        .as_mut()
                        .expect("switches only occur inside functions")
                        .switches
                        .push(SwitchState {
                            entry_variably_modified_path,
                            controlling_type: Some(expression.ty),
                            ..SwitchState::default()
                        });
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
                    let source_path = self
                        .function
                        .as_ref()
                        .expect("gotos only occur inside functions")
                        .active_variably_modified_path
                        .clone();
                    let labels = &mut self
                        .function
                        .as_mut()
                        .expect("gotos only occur inside functions")
                        .labels;
                    let id = labels.note_use(&label.name, label.span);
                    self.function
                        .as_mut()
                        .expect("gotos only occur inside functions")
                        .variably_modified_gotos
                        .push(VariablyModifiedGoto {
                            label: id,
                            span: label.span,
                            source_path,
                        });
                    FullTypedStatementKind::Goto {
                        label: id,
                        name: label.name.clone(),
                    }
                }
                S::ComputedGoto(expression) => {
                    let expression = self.analyze_expression(expression)?;
                    let expression = self.value_conversion(expression)?;
                    if self.pointer_pointee(expression.ty.ty).is_none() {
                        return self.fail(
                            "CCC2424",
                            expression.span,
                            "a computed goto target must have pointer type",
                        );
                    }
                    if let Some(ConstantValue::Address(RelocatableAddress {
                        base: RelocatableBase::Label { function, .. },
                        ..
                    })) = expression.constant
                        && Some(function) != self.function.as_ref().map(|state| state.id)
                    {
                        return self.fail(
                            "CCC2426",
                            expression.span,
                            "a computed goto cannot use a label from another function",
                        );
                    }
                    self.function
                        .as_mut()
                        .expect("computed gotos only occur inside functions")
                        .computed_gotos
                        .push(statement.span);
                    FullTypedStatementKind::ComputedGoto(expression)
                }
                S::Asm(asm) => {
                    FullTypedStatementKind::InlineAsm(Box::new(self.analyze_inline_asm(asm)?))
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

    fn analyze_inline_asm(
        &mut self,
        asm: &syntax::AsmStatement,
    ) -> AnalysisResult<FullTypedInlineAsm> {
        if self.config.target.abi != AbiIdentity::SysvAmd64Lp64 {
            return self.fail(
                "CCC2454",
                asm.span,
                "inline assembly is not certified for this target ABI",
            );
        }
        if asm.qualifiers.iter().any(|qualifier| {
            matches!(
                qualifier.kind,
                syntax::AsmQualifierKind::Inline | syntax::AsmQualifierKind::Goto
            )
        }) || !asm.goto_labels.is_empty()
        {
            return self.fail(
                "CCC2454",
                asm.span,
                "asm inline and asm goto are outside the certified inline-assembly forms",
            );
        }
        let volatile_count = asm
            .qualifiers
            .iter()
            .filter(|qualifier| qualifier.kind == syntax::AsmQualifierKind::Volatile)
            .count();
        if volatile_count > 1 {
            return self.fail(
                "CCC2454",
                asm.span,
                "an inline assembly qualifier may not be repeated",
            );
        }
        let volatile = volatile_count == 1 || asm.colon_group_count == 0;
        let Some(template) = ordinary_asm_text(&asm.template) else {
            return self.fail(
                "CCC2454",
                asm.span,
                "an assembly template must be an ordinary narrow string without null code units",
            );
        };
        let constraints = asm
            .outputs
            .iter()
            .chain(&asm.inputs)
            .map(|operand| ordinary_asm_text(&operand.constraint.literal))
            .collect::<Option<Vec<_>>>();
        let Some(constraints) = constraints else {
            return self.fail(
                "CCC2454",
                asm.span,
                "assembly constraints must be ordinary narrow strings without null code units",
            );
        };
        let clobbers = asm
            .clobbers
            .iter()
            .map(|clobber| ordinary_asm_text(&clobber.literal))
            .collect::<Option<Vec<_>>>();
        let Some(clobbers) = clobbers else {
            return self.fail(
                "CCC2454",
                asm.span,
                "assembly clobbers must be ordinary narrow strings without null code units",
            );
        };
        if asm
            .outputs
            .iter()
            .chain(&asm.inputs)
            .any(|operand| operand.symbolic_name.is_some())
        {
            return self.fail(
                "CCC2454",
                asm.span,
                "symbolic assembly operands are not part of a certified form",
            );
        }

        let outputs = asm.outputs.len();
        let inputs = asm.inputs.len();
        let output_constraints = &constraints[..outputs];
        let input_constraints = &constraints[outputs..];
        let no_qualifiers = asm.qualifiers.is_empty();
        let kind = if no_qualifiers
            && asm.colon_group_count == 0
            && outputs == 0
            && inputs == 0
            && clobbers.is_empty()
        {
            match template.as_str() {
                "" => FullTypedInlineAsmKind::CompilerBarrier { memory: false },
                "nop" => FullTypedInlineAsmKind::CodeLayoutHint(CodeLayoutHint::Nop),
                ".p2align 3" => {
                    FullTypedInlineAsmKind::CodeLayoutHint(CodeLayoutHint::AlignToPowerOfTwo(3))
                }
                ".p2align 4" => {
                    FullTypedInlineAsmKind::CodeLayoutHint(CodeLayoutHint::AlignToPowerOfTwo(4))
                }
                ".p2align 5" => {
                    FullTypedInlineAsmKind::CodeLayoutHint(CodeLayoutHint::AlignToPowerOfTwo(5))
                }
                ".p2align 6" => {
                    FullTypedInlineAsmKind::CodeLayoutHint(CodeLayoutHint::AlignToPowerOfTwo(6))
                }
                _ => return self.unsupported_inline_asm(asm, &template),
            }
        } else if volatile_count == 1
            && template.is_empty()
            && asm.colon_group_count == 3
            && outputs == 0
            && inputs == 0
            && clobbers == ["memory"]
        {
            FullTypedInlineAsmKind::CompilerBarrier { memory: true }
        } else if no_qualifiers
            && template.is_empty()
            && asm.colon_group_count == 1
            && output_constraints == ["+r"]
            && inputs == 0
            && clobbers.is_empty()
        {
            let target = self.analyze_asm_output(&asm.outputs[0], "opaque register operand")?;
            self.require_asm_scalar(&target, false, "opaque register operand")?;
            FullTypedInlineAsmKind::OpaqueScalar { target }
        } else if no_qualifiers
            && template == "cpuid"
            && asm.colon_group_count == 3
            && clobbers == ["ebx", "ecx", "edx"]
            && output_constraints == ["=a"]
            && input_constraints == ["a"]
        {
            self.analyze_cpuid(asm, &[X86CpuidRegister::Eax], false)?
        } else if no_qualifiers
            && template == "cpuid"
            && asm.colon_group_count == 3
            && clobbers == ["ebx"]
            && output_constraints == ["=a", "=c", "=d"]
            && input_constraints == ["a"]
        {
            self.analyze_cpuid(
                asm,
                &[
                    X86CpuidRegister::Eax,
                    X86CpuidRegister::Ecx,
                    X86CpuidRegister::Edx,
                ],
                false,
            )?
        } else if no_qualifiers
            && template == "cpuid"
            && asm.colon_group_count == 3
            && clobbers == ["edx"]
            && output_constraints == ["=a", "=b", "=c"]
            && input_constraints == ["a", "c"]
        {
            self.analyze_cpuid(
                asm,
                &[
                    X86CpuidRegister::Eax,
                    X86CpuidRegister::Ebx,
                    X86CpuidRegister::Ecx,
                ],
                true,
            )?
        } else if volatile_count == 1
            && template == "rdtsc"
            && asm.colon_group_count == 1
            && output_constraints == ["=a", "=d"]
            && inputs == 0
            && clobbers.is_empty()
        {
            let low = self.analyze_asm_u32_output(&asm.outputs[0], "RDTSC low output")?;
            let high = self.analyze_asm_u32_output(&asm.outputs[1], "RDTSC high output")?;
            FullTypedInlineAsmKind::X86Rdtsc { low, high }
        } else if no_qualifiers
            && template == "cmp %1, %2\ncmova %3, %0\n"
            && asm.colon_group_count == 2
            && output_constraints == ["+r"]
            && input_constraints == ["r", "r", "r"]
            && clobbers.is_empty()
        {
            self.analyze_conditional_move_above(asm)?
        } else if volatile_count == 1
            && template == "lock; xchgq %0, %1"
            && asm.colon_group_count == 1
            && output_constraints == ["+q", "+m"]
            && inputs == 0
            && clobbers.is_empty()
        {
            self.analyze_atomic_exchange(asm, None)?
        } else if volatile_count == 1
            && template == "lock; xchgq %1, %2"
            && asm.colon_group_count == 1
            && output_constraints == ["=r", "+q", "+m"]
            && inputs == 0
            && clobbers.is_empty()
        {
            self.analyze_atomic_exchange(asm, Some(0))?
        } else if volatile_count == 1
            && template == "lock; cmpxchgq %2, %1"
            && asm.colon_group_count == 2
            && output_constraints == ["=a", "+m"]
            && input_constraints == ["q", "0"]
            && clobbers.is_empty()
        {
            self.analyze_atomic_compare_exchange(asm)?
        } else {
            return self.unsupported_inline_asm(asm, &template);
        };
        Ok(FullTypedInlineAsm {
            kind,
            template,
            volatile,
        })
    }

    fn unsupported_inline_asm<T>(
        &mut self,
        asm: &syntax::AsmStatement,
        template: &str,
    ) -> AnalysisResult<T> {
        self.fail(
            "CCC2454",
            asm.span,
            format!(
                "inline assembly form is not certified for this target (template `{}`)",
                template.escape_debug()
            ),
        )
    }

    fn analyze_asm_output(
        &mut self,
        operand: &syntax::AsmOperand,
        description: &str,
    ) -> AnalysisResult<FullTypedExpression> {
        let expression = self.analyze_expression(&operand.expression)?;
        let Some(place) = expression.place.as_ref() else {
            return self.fail(
                "CCC2454",
                operand.expression.span,
                format!("{description} must be an lvalue"),
            );
        };
        if !place.modifiable {
            return self.fail(
                "CCC2454",
                operand.expression.span,
                format!("{description} must be a modifiable lvalue"),
            );
        }
        if place.bitfield.is_some() {
            return self.fail(
                "CCC2454",
                operand.expression.span,
                format!("{description} may not be a bit-field"),
            );
        }
        Ok(expression)
    }

    fn analyze_asm_u32_output(
        &mut self,
        operand: &syntax::AsmOperand,
        description: &str,
    ) -> AnalysisResult<FullTypedExpression> {
        let expression = self.analyze_asm_output(operand, description)?;
        if expression.ty.ty != TypeId::UNSIGNED_INT {
            return self.fail(
                "CCC2454",
                expression.span,
                format!("{description} must have type `unsigned int`"),
            );
        }
        Ok(expression)
    }

    fn analyze_asm_input_as(
        &mut self,
        operand: &syntax::AsmOperand,
        target: QualifiedType,
    ) -> AnalysisResult<FullTypedExpression> {
        let expression = self.analyze_expression(&operand.expression)?;
        self.assignment_conversion(expression, target, operand.expression.span)
    }

    fn analyze_cpuid(
        &mut self,
        asm: &syntax::AsmStatement,
        registers: &[X86CpuidRegister],
        has_subleaf: bool,
    ) -> AnalysisResult<FullTypedInlineAsmKind> {
        let mut outputs = Vec::with_capacity(registers.len());
        for (operand, register) in asm.outputs.iter().zip(registers) {
            outputs.push(X86CpuidOutput {
                register: *register,
                target: self.analyze_asm_u32_output(operand, "CPUID output")?,
            });
        }
        let u32_ty = QualifiedType::unqualified(TypeId::UNSIGNED_INT);
        let leaf = self.analyze_asm_input_as(&asm.inputs[0], u32_ty)?;
        let subleaf = has_subleaf
            .then(|| self.analyze_asm_input_as(&asm.inputs[1], u32_ty))
            .transpose()?;
        Ok(FullTypedInlineAsmKind::X86Cpuid {
            leaf,
            subleaf,
            outputs,
        })
    }

    fn analyze_conditional_move_above(
        &mut self,
        asm: &syntax::AsmStatement,
    ) -> AnalysisResult<FullTypedInlineAsmKind> {
        let target = self.analyze_asm_output(&asm.outputs[0], "conditional-move output")?;
        if self.pointer_pointee(target.ty.ty).is_none() {
            return self.fail(
                "CCC2454",
                target.span,
                "the certified conditional-move output must have pointer type",
            );
        }
        let u32_ty = QualifiedType::unqualified(TypeId::UNSIGNED_INT);
        let index = self.analyze_asm_input_as(&asm.inputs[0], u32_ty)?;
        let low_limit = self.analyze_asm_input_as(&asm.inputs[1], u32_ty)?;
        let backup =
            self.analyze_asm_input_as(&asm.inputs[2], QualifiedType::unqualified(target.ty.ty))?;
        Ok(FullTypedInlineAsmKind::X86ConditionalMoveAbove {
            target,
            index,
            low_limit,
            backup,
        })
    }

    fn analyze_atomic_exchange(
        &mut self,
        asm: &syntax::AsmStatement,
        result_index: Option<usize>,
    ) -> AnalysisResult<FullTypedInlineAsmKind> {
        let value_index = usize::from(result_index.is_some());
        let object_index = value_index + 1;
        let value = self.analyze_asm_output(&asm.outputs[value_index], "exchange value operand")?;
        let object =
            self.analyze_asm_output(&asm.outputs[object_index], "exchange memory operand")?;
        self.require_asm_atomic64_object(&object)?;
        if value.ty.ty != object.ty.ty {
            return self.fail(
                "CCC2454",
                value.span,
                "exchange value and memory operands must have the same type",
            );
        }
        let result = result_index
            .map(|index| self.analyze_asm_output(&asm.outputs[index], "exchange result output"))
            .transpose()?;
        if result
            .as_ref()
            .is_some_and(|result| result.ty.ty != object.ty.ty)
        {
            return self.fail(
                "CCC2454",
                result.as_ref().unwrap().span,
                "exchange result and memory operands must have the same type",
            );
        }
        Ok(FullTypedInlineAsmKind::X86AtomicExchange {
            object,
            value,
            result,
        })
    }

    fn analyze_atomic_compare_exchange(
        &mut self,
        asm: &syntax::AsmStatement,
    ) -> AnalysisResult<FullTypedInlineAsmKind> {
        let original = self.analyze_asm_output(&asm.outputs[0], "compare-exchange output")?;
        let object = self.analyze_asm_output(&asm.outputs[1], "compare-exchange memory operand")?;
        self.require_asm_atomic64_object(&object)?;
        if original.ty.ty != object.ty.ty {
            return self.fail(
                "CCC2454",
                original.span,
                "compare-exchange output and memory operands must have the same type",
            );
        }
        let object_ty = QualifiedType::unqualified(object.ty.ty);
        let desired = self.analyze_asm_input_as(&asm.inputs[0], object_ty)?;
        let expected = self.analyze_asm_input_as(&asm.inputs[1], object_ty)?;
        Ok(FullTypedInlineAsmKind::X86AtomicCompareExchange {
            object,
            expected,
            desired,
            original,
        })
    }

    fn require_asm_scalar(
        &mut self,
        expression: &FullTypedExpression,
        require_64_bits: bool,
        description: &str,
    ) -> AnalysisResult<()> {
        let scalar = self.types.is_integer(expression.ty.ty)
            || self.pointer_pointee(expression.ty.ty).is_some();
        let size = self
            .types
            .layout_of(expression.ty.ty, self.config)
            .map(|layout| layout.size)
            .unwrap_or(0);
        let valid_size = if require_64_bits {
            size == 8
        } else {
            matches!(size, 1 | 2 | 4 | 8)
        };
        if !scalar || !valid_size {
            return self.fail(
                "CCC2454",
                expression.span,
                format!(
                    "{description} requires a {} integer or pointer representation",
                    if require_64_bits {
                        "64-bit"
                    } else {
                        "1, 2, 4, or 8-byte"
                    }
                ),
            );
        }
        Ok(())
    }

    fn require_asm_atomic64_object(
        &mut self,
        expression: &FullTypedExpression,
    ) -> AnalysisResult<()> {
        self.require_asm_scalar(expression, true, "locked assembly memory operand")
    }

    fn analyze_function_body(
        &mut self,
        statement: &syntax::Statement,
    ) -> AnalysisResult<FullTypedStatement> {
        let syntax::StatementKind::Compound(items) = &statement.kind else {
            unreachable!("the parser requires a compound statement for a function body")
        };
        Ok(FullTypedStatement {
            kind: FullTypedStatementKind::Compound(self.analyze_compound_items(items)?),
            span: statement.span,
        })
    }

    fn analyze_compound_items(
        &mut self,
        items: &[syntax::BlockItem],
    ) -> AnalysisResult<Vec<FullTypedBlockItem>> {
        let mut output = Vec::new();
        for item in items {
            if self.error_limit_reached() {
                break;
            }
            let analyzed = match item {
                syntax::BlockItem::Declaration(declaration) => {
                    self.analyze_block_declaration(declaration)
                }
                syntax::BlockItem::StaticAssert(assertion) => {
                    self.analyze_static_assert(assertion).map(|value| {
                        vec![FullTypedBlockItem::StaticAssert {
                            value,
                            span: assertion.span,
                        }]
                    })
                }
                syntax::BlockItem::Statement(statement) => self
                    .analyze_statement(statement)
                    .map(|statement| vec![FullTypedBlockItem::Statement(Box::new(statement))]),
                syntax::BlockItem::Pragma(pragma) => self
                    .handle_pragma(pragma)
                    .map(|()| vec![FullTypedBlockItem::Pragma(pragma.clone())]),
            };
            if let Ok(items) = analyzed {
                output.extend(items);
            } else if let syntax::BlockItem::Declaration(declaration) = item {
                self.poison_declaration_bindings(declaration, false);
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
                FullTypedForInitializer::Expression(Box::new(self.analyze_expression(expression)?))
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
            E::LabelAddress(label) => self.analyze_label_address(label, expression.span),
            E::Integer(integer) => self.analyze_integer_literal(*integer, expression.span),
            E::Floating(floating) => self.analyze_floating_literal(floating, expression.span),
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
            E::StatementExpression(items) => {
                self.analyze_statement_expression(items, expression.span)
            }
            E::GenericSelection {
                controlling,
                associations,
            } => self.analyze_generic_selection(controlling, associations, expression.span),
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
            E::CompoundLiteral { ty, initializer } => {
                self.analyze_compound_literal(ty, initializer, expression.span)
            }
            E::Unary { operator, operand } => {
                self.analyze_unary(*operator, operand, expression.span)
            }
            E::SizeofExpression(operand) => {
                let operand = self.analyze_expression(operand)?;
                let operand_ty = operand.ty;
                let evaluated =
                    runtime_sized_array(&self.types, operand_ty.ty).then(|| Box::new(operand));
                self.analyze_sizeof(operand_ty, evaluated, expression.span)
            }
            E::SizeofType(type_name) => {
                let (ty, bounds) = self.resolve_type_name_with_bounds(type_name)?;
                let bounds = bounds_for_runtime_type(bounds, ty, &self.types);
                let sized = self.analyze_sizeof(ty, None, expression.span)?;
                Ok(self.with_variable_length_bounds(bounds, sized, expression.span))
            }
            E::AlignofType(type_name) => {
                let (ty, _) = self.resolve_type_name_with_bounds(type_name)?;
                let layout_ty = innermost_array_element(&self.types, ty).ty;
                let layout = self
                    .types
                    .layout_of(layout_ty, self.config)
                    .map_err(|error| {
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
                    constant_expression_kind: ConstantExpressionKind::Integer,
                    span: expression.span,
                })
            }
            E::Cast {
                ty: type_name,
                expression: operand,
            } => {
                let (target, bounds) = self.resolve_type_name_with_bounds(type_name)?;
                let operand = self.analyze_expression(operand)?;
                let operand = self.value_conversion(operand)?;
                let converted = self.explicit_conversion(operand, target, expression.span)?;
                Ok(self.with_variable_length_bounds(bounds, converted, expression.span))
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
                for item in expressions {
                    let item = self.analyze_expression(item)?;
                    typed.push(self.value_conversion(item)?);
                }
                let Some(last) = typed.last() else {
                    return self.fail("CCC2273", expression.span, "an empty comma expression");
                };
                let operands = typed.iter().collect::<Vec<_>>();
                let constant_expression_kind =
                    unevaluated_operator_constant_expression_kind(&operands);
                Ok(FullTypedExpression {
                    kind: FullTypedExpressionKind::Comma(typed.clone()),
                    ty: last.ty,
                    category: ValueCategory::Value,
                    place: None,
                    constant: last.constant,
                    constant_expression_kind,
                    span: expression.span,
                })
            }
            E::Extension(inner) => self.analyze_expression(inner),
            E::BuiltinOffsetof { ty, designator } => {
                self.analyze_offsetof(ty, designator, expression.span)
            }
            E::BuiltinVaStart {
                list,
                last_named_parameter,
            } => self.analyze_va_start(list, last_named_parameter, expression.span),
            E::BuiltinVaArg { list, ty } => self.analyze_va_arg(list, ty, expression.span),
            E::BuiltinVaCopy {
                destination,
                source,
            } => self.analyze_va_copy(destination, source, expression.span),
            E::BuiltinVaEnd { list } => self.analyze_va_end(list, expression.span),
            E::BuiltinExpect { value, expected } => {
                self.analyze_builtin_expect(value, expected, expression.span)
            }
            E::BuiltinHugeVal => self.analyze_builtin_huge_val(expression.span),
            E::BuiltinInfF => self.analyze_builtin_inff(expression.span),
            E::BuiltinNanF { payload } => self.analyze_builtin_nanf(payload, expression.span),
            E::BuiltinIntegerIntrinsic { operation, operand } => {
                self.analyze_integer_intrinsic(*operation, operand, expression.span)
            }
            E::BuiltinMemoryOperation {
                operation,
                arguments,
            } => self.analyze_memory_builtin(*operation, arguments, expression.span),
            E::BuiltinPrefetch { arguments } => {
                self.analyze_builtin_prefetch(arguments, expression.span)
            }
            E::BuiltinSyncOperation {
                operation,
                arguments,
            } => self.analyze_sync_operation(*operation, arguments, expression.span),
            E::BuiltinSyncSynchronize => self.analyze_sync_synchronize(expression.span),
            E::BuiltinAtomicOperation {
                operation,
                arguments,
            } => self.analyze_atomic_operation(*operation, arguments, expression.span),
        }
    }

    fn analyze_statement_expression(
        &mut self,
        items: &[syntax::BlockItem],
        span: Span,
    ) -> AnalysisResult<FullTypedExpression> {
        if self.function.is_none() {
            return self.fail(
                "CCC2452",
                span,
                "a statement expression is only supported inside a function",
            );
        }

        self.push_scope();
        let meaningful = items
            .iter()
            .enumerate()
            .filter(|(_, item)| {
                !matches!(
                    item,
                    syntax::BlockItem::Statement(statement)
                        if matches!(statement.kind, syntax::StatementKind::Expression(None))
                )
            })
            .collect::<Vec<_>>();

        let transparent = match meaningful.as_slice() {
            [(index, syntax::BlockItem::Statement(statement))] => match &statement.kind {
                syntax::StatementKind::Expression(Some(expression)) => Some((*index, expression)),
                _ => None,
            },
            _ => None,
        };

        let analyzed = (|| {
            if let Some((result_index, result_expression)) = transparent {
                let result = self.analyze_expression(result_expression)?;
                let result = if matches!(
                    self.types.try_kind(result.ty.ty),
                    Some(TypeKind::Array(_) | TypeKind::Function(_))
                ) || (result.category == ValueCategory::Lvalue
                    && !result.ty.qualifiers.is_empty())
                {
                    self.value_conversion(result)?
                } else {
                    result
                };
                let mut output = Vec::new();
                for (index, item) in items.iter().enumerate() {
                    if index == result_index {
                        continue;
                    }
                    if let syntax::BlockItem::Statement(statement) = item {
                        output.push(FullTypedBlockItem::Statement(Box::new(
                            self.analyze_statement(statement)?,
                        )));
                    }
                }
                Ok((output, Some(result)))
            } else {
                let mut output = self.analyze_compound_items(items)?;
                let result_index = output.iter().rposition(|item| {
                    !matches!(
                        item,
                        FullTypedBlockItem::Statement(statement)
                            if matches!(statement.kind, FullTypedStatementKind::Expression(None))
                    )
                });
                let result = result_index.and_then(|index| {
                    let FullTypedBlockItem::Statement(statement) = &mut output[index] else {
                        return None;
                    };
                    let FullTypedStatementKind::Expression(expression) = &mut statement.kind else {
                        return None;
                    };
                    expression.take().map(|expression| (index, expression))
                });
                let result = result.map(|(index, expression)| {
                    output.remove(index);
                    expression
                });
                let result = result
                    .map(|result| self.value_conversion(result))
                    .transpose()?;
                Ok((output, result))
            }
        })();
        self.pop_scope();
        let (items, result) = analyzed?;
        let ty = result
            .as_ref()
            .map_or(QualifiedType::unqualified(TypeId::VOID), |result| result.ty);
        let category = result
            .as_ref()
            .map_or(ValueCategory::Value, |result| result.category);
        let place = result.as_ref().and_then(|result| result.place.clone());
        Ok(FullTypedExpression {
            kind: FullTypedExpressionKind::StatementExpression {
                items,
                result: result.map(Box::new),
            },
            ty,
            category,
            place,
            constant: None,
            constant_expression_kind: ConstantExpressionKind::Invalid,
            span,
        })
    }

    fn analyze_compound_literal(
        &mut self,
        type_name: &syntax::TypeName,
        initializer: &syntax::Initializer,
        span: Span,
    ) -> AnalysisResult<FullTypedExpression> {
        let (declared_ty, variable_length_bounds) =
            self.resolve_type_name_with_bounds(type_name)?;
        if !variable_length_bounds.is_empty() || self.is_variably_modified(declared_ty.ty) {
            return self.fail(
                "CCC2430",
                span,
                "a compound literal may not have variably modified type",
            );
        }
        let (initializer, completed_ty) = self.analyze_initializer(declared_ty, initializer)?;
        self.validate_object_type(completed_ty, span, true)?;
        if self.is_variably_modified(completed_ty.ty) {
            return self.fail(
                "CCC2430",
                span,
                "a compound literal may not have variably modified type",
            );
        }

        let (storage, place_base, constant) = if self.function.is_some() {
            let local = self.fresh_local();
            (
                CompoundLiteralStorage::Automatic(local),
                PlaceBase::CompoundLiteral(local),
                None,
            )
        } else {
            if !initializer_is_static(&initializer) {
                return self.fail(
                    "CCC2344",
                    span,
                    "a file-scope compound literal requires a constant or relocatable initializer",
                );
            }
            if self.initializer_references_thread_storage(&initializer) {
                return self.fail(
                    "CCC2344",
                    span,
                    "a file-scope compound literal cannot contain the address of a thread-local object",
                );
            }
            let id = GlobalId(self.globals.len() as u32);
            let symbol_name = format!("__ccc_file_compound_literal.{}", id.0);
            self.globals.push(FullTypedGlobal {
                id,
                name: format!("<compound-literal-{}>", id.0),
                ty: completed_ty,
                storage: SemanticStorageClass::Static,
                linkage: Linkage::Internal,
                duration: StorageDuration::Static,
                initializer: Some(initializer.clone()),
                tentative: false,
                asm_label: None,
                attributes: Vec::new(),
                emission: GlobalEmission {
                    symbol_name,
                    symbol_name_is_exact: false,
                    binding: SymbolBinding::Strong,
                    visibility: SymbolVisibility::Internal,
                    section: None,
                    requested_alignment: None,
                    tls: None,
                    definition: ObjectDefinitionPolicy::Definition,
                },
                span,
            });
            (
                CompoundLiteralStorage::Static(id),
                PlaceBase::Global(id),
                Some(ConstantValue::Address(RelocatableAddress {
                    base: RelocatableBase::Global(id),
                    addend: 0,
                    one_past: false,
                })),
            )
        };
        Ok(FullTypedExpression {
            kind: FullTypedExpressionKind::CompoundLiteral {
                storage,
                initializer: Box::new(initializer),
            },
            ty: completed_ty,
            category: ValueCategory::Lvalue,
            place: Some(self.object_place(place_base, completed_ty, true)),
            constant,
            constant_expression_kind: ConstantExpressionKind::Invalid,
            span,
        })
    }

    fn analyze_generic_selection(
        &mut self,
        controlling: &syntax::Expression,
        associations: &[syntax::GenericAssociation],
        span: Span,
    ) -> AnalysisResult<FullTypedExpression> {
        let controlling = self.analyze_expression(controlling)?;
        let controlling_ty = self.value_conversion(controlling)?.ty;

        let mut typed_associations = Vec::with_capacity(associations.len());
        let mut association_types: Vec<(QualifiedType, Span)> = Vec::new();
        let mut default_index = None;

        for association in associations {
            let association_ty = if let Some(type_name) = &association.ty {
                let (ty, variable_length_bounds) = self.resolve_type_name_with_bounds(type_name)?;
                self.validate_object_type(ty, type_name.span, true)?;
                if !variable_length_bounds.is_empty() || self.is_variably_modified(ty.ty) {
                    return self.fail(
                        "CCC2270",
                        type_name.span,
                        "a generic association may not specify a variably modified type",
                    );
                }
                if let Some((_, previous_span)) = association_types
                    .iter()
                    .find(|(previous, _)| self.types_compatible(*previous, ty))
                {
                    return self.fail(
                        "CCC2270",
                        type_name.span,
                        format!(
                            "generic associations specify compatible type `{}` more than once (first specified at byte {})",
                            self.types.display_qualified(ty),
                            previous_span.start
                        ),
                    );
                }
                association_types.push((ty, type_name.span));
                Some(ty)
            } else {
                if default_index.replace(typed_associations.len()).is_some() {
                    return self.fail(
                        "CCC2270",
                        association.span,
                        "a generic selection may contain at most one default association",
                    );
                }
                None
            };
            let typed = self.analyze_expression(&association.expression)?;
            typed_associations.push((association_ty, typed));
        }

        let matching = typed_associations
            .iter()
            .enumerate()
            .filter_map(|(index, (ty, _))| {
                ty.is_some_and(|ty| self.types_compatible(controlling_ty, ty))
                    .then_some(index)
            })
            .collect::<Vec<_>>();
        let selected_index = match matching.as_slice() {
            [selected] => *selected,
            [] => default_index.ok_or_else(|| {
                self.emit(
                    "CCC2270",
                    span,
                    format!(
                        "no generic association is compatible with controlling type `{}` and no default was provided",
                        self.types.display_qualified(controlling_ty)
                    ),
                );
            })?,
            _ => {
                return self.fail(
                    "CCC2270",
                    span,
                    "the controlling expression matches more than one generic association",
                );
            }
        };

        let selected = typed_associations.swap_remove(selected_index).1;
        let selected_ty = selected.ty;
        let selected_category = selected.category;
        let selected_place = selected.place.clone();
        let selected_constant = selected.constant;
        let selected_constant_expression_kind = selected.constant_expression_kind;
        Ok(FullTypedExpression {
            kind: FullTypedExpressionKind::GenericSelection {
                controlling_ty,
                selected: Box::new(selected),
            },
            ty: selected_ty,
            category: selected_category,
            place: selected_place,
            constant: selected_constant,
            constant_expression_kind: selected_constant_expression_kind,
            span,
        })
    }

    fn analyze_builtin_expect(
        &mut self,
        value: &syntax::Expression,
        expected: &syntax::Expression,
        span: Span,
    ) -> AnalysisResult<FullTypedExpression> {
        self.require_builtin("__builtin_expect", span)?;
        let long = QualifiedType::unqualified(TypeId::LONG);
        let typed_value = self.analyze_expression(value)?;
        let value = self.assignment_conversion(typed_value, long, value.span)?;
        let typed_expected = self.analyze_expression(expected)?;
        let expected = self.assignment_conversion(typed_expected, long, expected.span)?;
        if expected.constant.and_then(ConstantValue::as_i128).is_none()
            || !builtin_expect_folded_constant(&expected)
        {
            return self.fail(
                "CCC2428",
                expected.span,
                "the second argument to `__builtin_expect` must be a compile-time constant",
            );
        }
        let constant = value.constant;
        let constant_expression_kind = value.constant_expression_kind;
        Ok(FullTypedExpression {
            kind: FullTypedExpressionKind::BuiltinExpect {
                value: Box::new(value),
                expected: Box::new(expected),
            },
            ty: long,
            category: ValueCategory::Value,
            place: None,
            constant,
            constant_expression_kind,
            span,
        })
    }

    fn analyze_builtin_huge_val(&mut self, span: Span) -> AnalysisResult<FullTypedExpression> {
        self.require_builtin("__builtin_huge_val", span)?;
        Ok(self.constant_expression(
            ConstantValue::Floating(f64::INFINITY),
            QualifiedType::unqualified(TypeId::DOUBLE),
            span,
        ))
    }

    fn analyze_builtin_inff(&mut self, span: Span) -> AnalysisResult<FullTypedExpression> {
        self.require_builtin("__builtin_inff", span)?;
        Ok(self.constant_expression(
            ConstantValue::Floating(f64::from(f32::INFINITY)),
            QualifiedType::unqualified(TypeId::FLOAT),
            span,
        ))
    }

    fn analyze_builtin_nanf(
        &mut self,
        payload: &syntax::Expression,
        span: Span,
    ) -> AnalysisResult<FullTypedExpression> {
        self.require_builtin("__builtin_nanf", span)?;
        let Some(payload_literal) = initializer_string_literal(payload) else {
            return self.fail(
                "CCC2429",
                payload.span,
                "`__builtin_nanf` requires an empty narrow string-literal payload",
            );
        };
        if !matches!(
            payload_literal.prefix,
            StringLiteralPrefix::None | StringLiteralPrefix::Utf8
        ) || !payload_literal.code_units.is_empty()
        {
            return self.fail(
                "CCC2429",
                payload.span,
                "`__builtin_nanf` requires an empty narrow string-literal payload",
            );
        }
        Ok(self.constant_expression(
            ConstantValue::Floating(f64::from(f32::from_bits(0x7fc0_0000))),
            QualifiedType::unqualified(TypeId::FLOAT),
            span,
        ))
    }

    fn analyze_sync_synchronize(&mut self, span: Span) -> AnalysisResult<FullTypedExpression> {
        self.require_builtin("__sync_synchronize", span)?;
        Ok(FullTypedExpression {
            kind: FullTypedExpressionKind::MemoryFence {
                order: MemoryOrder::SequentiallyConsistent,
            },
            ty: QualifiedType::unqualified(TypeId::VOID),
            category: ValueCategory::Value,
            place: None,
            constant: None,
            constant_expression_kind: ConstantExpressionKind::Invalid,
            span,
        })
    }

    fn analyze_integer_intrinsic(
        &mut self,
        operation: syntax::IntegerBuiltinOperation,
        operand: &syntax::Expression,
        span: Span,
    ) -> AnalysisResult<FullTypedExpression> {
        self.require_builtin(operation.spelling(), span)?;
        let (operation, input_ty, result_ty) = match operation {
            syntax::IntegerBuiltinOperation::ByteSwap64 => (
                IntegerIntrinsicOperation::ByteSwap64,
                TypeId::UNSIGNED_LONG,
                TypeId::UNSIGNED_LONG,
            ),
            syntax::IntegerBuiltinOperation::CountLeadingZerosInt => (
                IntegerIntrinsicOperation::CountLeadingZerosInt,
                TypeId::UNSIGNED_INT,
                TypeId::INT,
            ),
            syntax::IntegerBuiltinOperation::CountLeadingZerosLong => (
                IntegerIntrinsicOperation::CountLeadingZerosLong,
                TypeId::UNSIGNED_LONG,
                TypeId::INT,
            ),
            syntax::IntegerBuiltinOperation::CountLeadingZerosLongLong => (
                IntegerIntrinsicOperation::CountLeadingZerosLongLong,
                TypeId::UNSIGNED_LONG_LONG,
                TypeId::INT,
            ),
            syntax::IntegerBuiltinOperation::CountTrailingZerosLongLong => (
                IntegerIntrinsicOperation::CountTrailingZerosLongLong,
                TypeId::UNSIGNED_LONG_LONG,
                TypeId::INT,
            ),
            syntax::IntegerBuiltinOperation::PopulationCountInt => (
                IntegerIntrinsicOperation::PopulationCountInt,
                TypeId::UNSIGNED_INT,
                TypeId::INT,
            ),
            syntax::IntegerBuiltinOperation::PopulationCountLongLong => (
                IntegerIntrinsicOperation::PopulationCountLongLong,
                TypeId::UNSIGNED_LONG_LONG,
                TypeId::INT,
            ),
            syntax::IntegerBuiltinOperation::CountTrailingZerosInt => (
                IntegerIntrinsicOperation::CountTrailingZerosInt,
                TypeId::UNSIGNED_INT,
                TypeId::INT,
            ),
        };
        let operand = self.analyze_expression(operand)?;
        let operand_span = operand.span;
        let operand = self.assignment_conversion(
            operand,
            QualifiedType::unqualified(input_ty),
            operand_span,
        )?;
        let constant = if operand.constant_expression_kind == ConstantExpressionKind::Integer {
            self.fold_integer_intrinsic(operation, &operand)
        } else {
            None
        };
        let constant_expression_kind = if constant.is_some() {
            ConstantExpressionKind::Integer
        } else {
            ConstantExpressionKind::Invalid
        };
        Ok(FullTypedExpression {
            kind: FullTypedExpressionKind::IntegerIntrinsic {
                operation,
                operand: Box::new(operand),
            },
            ty: QualifiedType::unqualified(result_ty),
            category: ValueCategory::Value,
            place: None,
            constant,
            constant_expression_kind,
            span,
        })
    }

    fn fold_integer_intrinsic(
        &self,
        operation: IntegerIntrinsicOperation,
        operand: &FullTypedExpression,
    ) -> Option<ConstantValue> {
        let raw = match operand.constant? {
            ConstantValue::Signed(value) => value as u128,
            ConstantValue::Unsigned(value) => value,
            ConstantValue::Floating(_)
            | ConstantValue::LongDouble(_)
            | ConstantValue::NullPointer
            | ConstantValue::Address(_) => return None,
        };
        let input = match operation {
            IntegerIntrinsicOperation::CountLeadingZerosInt
            | IntegerIntrinsicOperation::PopulationCountInt
            | IntegerIntrinsicOperation::CountTrailingZerosInt => BuiltinType::UnsignedInt,
            IntegerIntrinsicOperation::ByteSwap64
            | IntegerIntrinsicOperation::CountLeadingZerosLong => BuiltinType::UnsignedLong,
            IntegerIntrinsicOperation::CountLeadingZerosLongLong
            | IntegerIntrinsicOperation::CountTrailingZerosLongLong
            | IntegerIntrinsicOperation::PopulationCountLongLong => BuiltinType::UnsignedLongLong,
        };
        let width = self.integer_width(input);
        let raw = truncate_to_width(raw, width);
        match operation {
            IntegerIntrinsicOperation::ByteSwap64 => {
                Some(ConstantValue::Unsigned((raw as u64).swap_bytes().into()))
            }
            IntegerIntrinsicOperation::CountLeadingZerosInt
            | IntegerIntrinsicOperation::CountLeadingZerosLong
            | IntegerIntrinsicOperation::CountLeadingZerosLongLong => {
                if raw == 0 {
                    return None;
                }
                let leading = raw.leading_zeros() - (u128::BITS - u32::from(width));
                Some(ConstantValue::Signed(i128::from(leading)))
            }
            IntegerIntrinsicOperation::CountTrailingZerosLongLong
            | IntegerIntrinsicOperation::CountTrailingZerosInt => {
                if raw == 0 {
                    return None;
                }
                Some(ConstantValue::Signed(i128::from(raw.trailing_zeros())))
            }
            IntegerIntrinsicOperation::PopulationCountInt
            | IntegerIntrinsicOperation::PopulationCountLongLong => {
                Some(ConstantValue::Signed(i128::from(raw.count_ones())))
            }
        }
    }

    fn analyze_builtin_prefetch(
        &mut self,
        arguments: &[syntax::Expression],
        span: Span,
    ) -> AnalysisResult<FullTypedExpression> {
        self.require_builtin("__builtin_prefetch", span)?;
        debug_assert!((1..=3).contains(&arguments.len()));
        let address = self.analyze_expression(&arguments[0])?;
        let const_void = QualifiedType::new(TypeId::VOID, TypeQualifiers::CONST);
        let target = QualifiedType::unqualified(self.types.pointer(const_void));
        let address = self.assignment_conversion(address, target, arguments[0].span)?;
        let write = if let Some(argument) = arguments.get(1) {
            self.analyze_prefetch_hint(argument, "read/write", 0, 1)? != 0
        } else {
            false
        };
        let locality = if let Some(argument) = arguments.get(2) {
            self.analyze_prefetch_hint(argument, "locality", 0, 3)? as u8
        } else {
            3
        };
        Ok(FullTypedExpression {
            kind: FullTypedExpressionKind::Prefetch {
                address: Box::new(address),
                write,
                locality,
            },
            ty: QualifiedType::unqualified(TypeId::VOID),
            category: ValueCategory::Value,
            place: None,
            constant: None,
            constant_expression_kind: ConstantExpressionKind::Invalid,
            span,
        })
    }

    fn analyze_memory_builtin(
        &mut self,
        operation: syntax::MemoryBuiltinOperation,
        arguments: &[syntax::Expression],
        span: Span,
    ) -> AnalysisResult<FullTypedExpression> {
        self.require_builtin(operation.spelling(), span)?;
        debug_assert_eq!(arguments.len(), 3);

        let void_pointer = QualifiedType::unqualified(
            self.types.pointer(QualifiedType::unqualified(TypeId::VOID)),
        );
        let const_void_pointer = QualifiedType::unqualified(
            self.types
                .pointer(QualifiedType::new(TypeId::VOID, TypeQualifiers::CONST)),
        );
        let destination = self.analyze_expression(&arguments[0])?;
        let destination =
            self.assignment_conversion(destination, void_pointer, arguments[0].span)?;
        let length = self.analyze_expression(&arguments[2])?;
        let length = self.assignment_conversion(
            length,
            QualifiedType::unqualified(self.size_type()),
            arguments[2].span,
        )?;

        let kind = match operation {
            syntax::MemoryBuiltinOperation::Copy | syntax::MemoryBuiltinOperation::Move => {
                let source = self.analyze_expression(&arguments[1])?;
                let source =
                    self.assignment_conversion(source, const_void_pointer, arguments[1].span)?;
                FullTypedExpressionKind::MemoryCopy {
                    destination: Box::new(destination),
                    source: Box::new(source),
                    length: Box::new(length),
                    overlap: operation == syntax::MemoryBuiltinOperation::Move,
                }
            }
            syntax::MemoryBuiltinOperation::Set => {
                let value = self.analyze_expression(&arguments[1])?;
                let value = self.assignment_conversion(
                    value,
                    QualifiedType::unqualified(TypeId::INT),
                    arguments[1].span,
                )?;
                FullTypedExpressionKind::MemorySet {
                    destination: Box::new(destination),
                    value: Box::new(value),
                    length: Box::new(length),
                }
            }
        };
        Ok(FullTypedExpression {
            kind,
            ty: void_pointer,
            category: ValueCategory::Value,
            place: None,
            constant: None,
            constant_expression_kind: ConstantExpressionKind::Invalid,
            span,
        })
    }

    fn analyze_prefetch_hint(
        &mut self,
        argument: &syntax::Expression,
        name: &str,
        minimum: i128,
        maximum: i128,
    ) -> AnalysisResult<i128> {
        let value = self.analyze_expression(argument)?;
        if !self.types.is_integer(value.ty.ty) {
            return self.fail(
                "CCC2436",
                argument.span,
                format!(
                    "the `__builtin_prefetch` {name} hint must be an integer constant from {minimum} through {maximum}"
                ),
            );
        }
        let value = self.assignment_conversion(
            value,
            QualifiedType::unqualified(TypeId::INT),
            argument.span,
        )?;
        let constant = value.constant.and_then(ConstantValue::as_i128);
        let Some(constant) = constant.filter(|value| (minimum..=maximum).contains(value)) else {
            return self.fail(
                "CCC2436",
                argument.span,
                format!(
                    "the `__builtin_prefetch` {name} hint must be an integer constant from {minimum} through {maximum}"
                ),
            );
        };
        if !builtin_expect_folded_constant(&value) {
            return self.fail(
                "CCC2436",
                argument.span,
                format!(
                    "the `__builtin_prefetch` {name} hint must be an integer constant from {minimum} through {maximum}"
                ),
            );
        }
        Ok(constant)
    }

    fn analyze_sync_operation(
        &mut self,
        operation: syntax::SyncBuiltinOperation,
        arguments: &[syntax::Expression],
        span: Span,
    ) -> AnalysisResult<FullTypedExpression> {
        self.require_builtin(operation.spelling(), span)?;
        debug_assert!(arguments.len() >= operation.fixed_arity());

        for protected in &arguments[operation.fixed_arity()..] {
            if matches!(
                &protected.kind,
                syntax::ExpressionKind::Identifier(identifier)
                    if identifier.name == "__sync_synchronize"
            ) {
                self.require_builtin("__sync_synchronize", protected.span)?;
            } else {
                // GCC accepts this historical operand list but does not evaluate
                // it. Analyze names and types for source diagnostics, then omit
                // the expressions from the typed operation.
                let _ = self.analyze_expression(protected)?;
            }
        }

        let pointer = self.analyze_expression(&arguments[0])?;
        let pointer = self.value_conversion(pointer)?;
        let Some(object) = self.pointer_pointee(pointer.ty.ty) else {
            return self.fail(
                "CCC2433",
                arguments[0].span,
                format!(
                    "the first argument to `{}` must point to a modifiable integer or pointer object",
                    operation.spelling()
                ),
            );
        };
        if object.qualifiers.contains(TypeQualifiers::CONST) {
            return self.fail(
                "CCC2433",
                arguments[0].span,
                format!(
                    "the first argument to `{}` points to a const-qualified object",
                    operation.spelling()
                ),
            );
        }
        let integer = self.types.is_integer(object.ty)
            && self.types.builtin_type(object.ty) != Some(BuiltinType::Bool);
        let pointer_object = self.pointer_pointee(object.ty).is_some();
        let native_alignment = match self.native_atomic_object_alignment(object.ty) {
            Some(alignment) if integer || pointer_object => alignment,
            _ => {
                return self.fail(
                    "CCC2434",
                    arguments[0].span,
                    format!(
                        "`{}` requires a 1, 2, 4, or 8-byte integer or pointer object",
                        operation.spelling()
                    ),
                );
            }
        };
        if self
            .pointer_alignment_provenance(&pointer)
            .known_minimum()
            .is_some_and(|alignment| alignment < native_alignment)
        {
            return self.fail(
                "CCC2434",
                arguments[0].span,
                format!(
                    "`{}` cannot operate on an address with weakened packed-member alignment",
                    operation.spelling()
                ),
            );
        }

        let value_ty = QualifiedType::unqualified(object.ty);
        let first = self.analyze_expression(&arguments[1])?;
        let first = self.sync_operand_conversion(first, value_ty, arguments[1].span)?;
        let order = MemoryOrder::SequentiallyConsistent;
        let (kind, ty) = match operation {
            syntax::SyncBuiltinOperation::AddAndFetch => (
                FullTypedExpressionKind::AtomicReadModifyWrite {
                    operation: AtomicReadModifyWriteOperation::Add,
                    pointer: Box::new(pointer),
                    operand: Box::new(first),
                    object,
                    return_new: true,
                    order,
                },
                value_ty,
            ),
            syntax::SyncBuiltinOperation::FetchAndAdd => (
                FullTypedExpressionKind::AtomicReadModifyWrite {
                    operation: AtomicReadModifyWriteOperation::Add,
                    pointer: Box::new(pointer),
                    operand: Box::new(first),
                    object,
                    return_new: false,
                    order,
                },
                value_ty,
            ),
            syntax::SyncBuiltinOperation::SubAndFetch => (
                FullTypedExpressionKind::AtomicReadModifyWrite {
                    operation: AtomicReadModifyWriteOperation::Subtract,
                    pointer: Box::new(pointer),
                    operand: Box::new(first),
                    object,
                    return_new: true,
                    order,
                },
                value_ty,
            ),
            syntax::SyncBuiltinOperation::LockTestAndSet => (
                FullTypedExpressionKind::AtomicReadModifyWrite {
                    operation: AtomicReadModifyWriteOperation::Exchange,
                    pointer: Box::new(pointer),
                    operand: Box::new(first),
                    object,
                    return_new: false,
                    order,
                },
                value_ty,
            ),
            syntax::SyncBuiltinOperation::BoolCompareAndSwap
            | syntax::SyncBuiltinOperation::ValCompareAndSwap => {
                let replacement = self.analyze_expression(&arguments[2])?;
                let replacement =
                    self.sync_operand_conversion(replacement, value_ty, arguments[2].span)?;
                let return_boolean = operation == syntax::SyncBuiltinOperation::BoolCompareAndSwap;
                (
                    FullTypedExpressionKind::AtomicCompareExchange {
                        pointer: Box::new(pointer),
                        expected: Box::new(first),
                        replacement: Box::new(replacement),
                        object,
                        return_boolean,
                        expected_is_pointer: false,
                        order,
                    },
                    if return_boolean {
                        QualifiedType::unqualified(TypeId::BOOL)
                    } else {
                        value_ty
                    },
                )
            }
        };
        Ok(FullTypedExpression {
            kind,
            ty,
            category: ValueCategory::Value,
            place: None,
            constant: None,
            constant_expression_kind: ConstantExpressionKind::Invalid,
            span,
        })
    }

    fn analyze_atomic_operation(
        &mut self,
        operation: syntax::AtomicBuiltinOperation,
        arguments: &[syntax::Expression],
        span: Span,
    ) -> AnalysisResult<FullTypedExpression> {
        self.require_builtin(operation.spelling(), span)?;
        debug_assert_eq!(arguments.len(), operation.arity());

        if matches!(
            operation,
            syntax::AtomicBuiltinOperation::ThreadFence
                | syntax::AtomicBuiltinOperation::SignalFence
        ) {
            let order = self.analyze_atomic_control_argument(
                &arguments[0],
                operation.spelling(),
                "memory order",
            )?;
            self.validate_atomic_order(operation.spelling(), &order, &[0, 1, 2, 3, 4, 5])?;
            let result = FullTypedExpression {
                // A hardware fence is a valid strengthening of a signal fence.
                kind: FullTypedExpressionKind::MemoryFence {
                    order: MemoryOrder::SequentiallyConsistent,
                },
                ty: QualifiedType::unqualified(TypeId::VOID),
                category: ValueCategory::Value,
                place: None,
                constant: None,
                constant_expression_kind: ConstantExpressionKind::Invalid,
                span,
            };
            return Ok(self.sequence_atomic_controls(vec![order], result, span));
        }

        if operation == syntax::AtomicBuiltinOperation::IsLockFree {
            let (pointer, _) =
                self.analyze_atomic_pointer(operation.spelling(), &arguments[0], false)?;
            let result = self.constant_expression(
                ConstantValue::Signed(1),
                QualifiedType::unqualified(TypeId::BOOL),
                span,
            );
            return Ok(self.sequence_atomic_controls(vec![pointer], result, span));
        }

        let modifies = !matches!(operation, syntax::AtomicBuiltinOperation::Load);
        let (pointer, object) =
            self.analyze_atomic_pointer(operation.spelling(), &arguments[0], modifies)?;
        let value_ty = QualifiedType::unqualified(object.ty);
        let sequentially_consistent = MemoryOrder::SequentiallyConsistent;

        match operation {
            syntax::AtomicBuiltinOperation::Load => {
                let order = self.analyze_atomic_control_argument(
                    &arguments[1],
                    operation.spelling(),
                    "memory order",
                )?;
                self.validate_atomic_order(operation.spelling(), &order, &[0, 1, 2, 5])?;
                let result = FullTypedExpression {
                    kind: FullTypedExpressionKind::AtomicLoad {
                        pointer: Box::new(pointer),
                        object,
                        order: sequentially_consistent,
                    },
                    ty: value_ty,
                    category: ValueCategory::Value,
                    place: None,
                    constant: None,
                    constant_expression_kind: ConstantExpressionKind::Invalid,
                    span,
                };
                Ok(self.sequence_atomic_controls(vec![order], result, span))
            }
            syntax::AtomicBuiltinOperation::Store => {
                let value = self.analyze_expression(&arguments[1])?;
                let value = self.sync_operand_conversion(value, value_ty, arguments[1].span)?;
                let order = self.analyze_atomic_control_argument(
                    &arguments[2],
                    operation.spelling(),
                    "memory order",
                )?;
                self.validate_atomic_order(operation.spelling(), &order, &[0, 3, 5])?;
                let result = FullTypedExpression {
                    kind: FullTypedExpressionKind::AtomicStore {
                        pointer: Box::new(pointer),
                        value: Box::new(value),
                        object,
                        order: sequentially_consistent,
                    },
                    ty: QualifiedType::unqualified(TypeId::VOID),
                    category: ValueCategory::Value,
                    place: None,
                    constant: None,
                    constant_expression_kind: ConstantExpressionKind::Invalid,
                    span,
                };
                Ok(self.sequence_atomic_controls(vec![order], result, span))
            }
            syntax::AtomicBuiltinOperation::Exchange
            | syntax::AtomicBuiltinOperation::FetchAdd
            | syntax::AtomicBuiltinOperation::FetchSubtract
            | syntax::AtomicBuiltinOperation::FetchAnd
            | syntax::AtomicBuiltinOperation::FetchOr
            | syntax::AtomicBuiltinOperation::FetchXor
            | syntax::AtomicBuiltinOperation::AddFetch
            | syntax::AtomicBuiltinOperation::SubtractFetch
            | syntax::AtomicBuiltinOperation::AndFetch
            | syntax::AtomicBuiltinOperation::OrFetch
            | syntax::AtomicBuiltinOperation::XorFetch => {
                let bool_object = self.types.builtin_type(object.ty) == Some(BuiltinType::Bool);
                let integer_object = self.types.is_integer(object.ty) && !bool_object;
                let operation_supported = match operation {
                    syntax::AtomicBuiltinOperation::Exchange => true,
                    syntax::AtomicBuiltinOperation::FetchAdd
                    | syntax::AtomicBuiltinOperation::FetchSubtract
                    | syntax::AtomicBuiltinOperation::AddFetch
                    | syntax::AtomicBuiltinOperation::SubtractFetch => integer_object,
                    syntax::AtomicBuiltinOperation::FetchAnd
                    | syntax::AtomicBuiltinOperation::FetchOr
                    | syntax::AtomicBuiltinOperation::FetchXor
                    | syntax::AtomicBuiltinOperation::AndFetch
                    | syntax::AtomicBuiltinOperation::OrFetch
                    | syntax::AtomicBuiltinOperation::XorFetch => integer_object,
                    _ => unreachable!(),
                };
                if !operation_supported {
                    return self.fail(
                        "CCC2455",
                        arguments[0].span,
                        format!(
                            "`{}` does not support this atomic object type",
                            operation.spelling()
                        ),
                    );
                }
                let operand = self.analyze_expression(&arguments[1])?;
                let operand = self.sync_operand_conversion(operand, value_ty, arguments[1].span)?;
                let order = self.analyze_atomic_control_argument(
                    &arguments[2],
                    operation.spelling(),
                    "memory order",
                )?;
                self.validate_atomic_order(operation.spelling(), &order, &[0, 1, 2, 3, 4, 5])?;
                let (operation, return_new) = match operation {
                    syntax::AtomicBuiltinOperation::Exchange => {
                        (AtomicReadModifyWriteOperation::Exchange, false)
                    }
                    syntax::AtomicBuiltinOperation::FetchAdd => {
                        (AtomicReadModifyWriteOperation::Add, false)
                    }
                    syntax::AtomicBuiltinOperation::FetchSubtract => {
                        (AtomicReadModifyWriteOperation::Subtract, false)
                    }
                    syntax::AtomicBuiltinOperation::FetchAnd => {
                        (AtomicReadModifyWriteOperation::BitwiseAnd, false)
                    }
                    syntax::AtomicBuiltinOperation::FetchOr => {
                        (AtomicReadModifyWriteOperation::BitwiseOr, false)
                    }
                    syntax::AtomicBuiltinOperation::FetchXor => {
                        (AtomicReadModifyWriteOperation::BitwiseXor, false)
                    }
                    syntax::AtomicBuiltinOperation::AddFetch => {
                        (AtomicReadModifyWriteOperation::Add, true)
                    }
                    syntax::AtomicBuiltinOperation::SubtractFetch => {
                        (AtomicReadModifyWriteOperation::Subtract, true)
                    }
                    syntax::AtomicBuiltinOperation::AndFetch => {
                        (AtomicReadModifyWriteOperation::BitwiseAnd, true)
                    }
                    syntax::AtomicBuiltinOperation::OrFetch => {
                        (AtomicReadModifyWriteOperation::BitwiseOr, true)
                    }
                    syntax::AtomicBuiltinOperation::XorFetch => {
                        (AtomicReadModifyWriteOperation::BitwiseXor, true)
                    }
                    _ => unreachable!(),
                };
                let result = FullTypedExpression {
                    kind: FullTypedExpressionKind::AtomicReadModifyWrite {
                        operation,
                        pointer: Box::new(pointer),
                        operand: Box::new(operand),
                        object,
                        return_new,
                        order: sequentially_consistent,
                    },
                    ty: value_ty,
                    category: ValueCategory::Value,
                    place: None,
                    constant: None,
                    constant_expression_kind: ConstantExpressionKind::Invalid,
                    span,
                };
                Ok(self.sequence_atomic_controls(vec![order], result, span))
            }
            syntax::AtomicBuiltinOperation::CompareExchange => {
                let expected_pointer = self.analyze_expression(&arguments[1])?;
                let expected_pointer = self.value_conversion(expected_pointer)?;
                let Some(expected_object) = self.pointer_pointee(expected_pointer.ty.ty) else {
                    return self.fail(
                        "CCC2455",
                        arguments[1].span,
                        "the expected-value argument to `__atomic_compare_exchange_n` must be a pointer",
                    );
                };
                if expected_object.qualifiers.contains(TypeQualifiers::CONST)
                    || expected_object
                        .qualifiers
                        .contains(TypeQualifiers::VOLATILE)
                    || expected_object.qualifiers.contains(TypeQualifiers::ATOMIC)
                    || !self.type_ids_compatible(expected_object.ty, object.ty)
                {
                    return self.fail(
                        "CCC2455",
                        arguments[1].span,
                        "the expected-value argument to `__atomic_compare_exchange_n` must point to a modifiable non-atomic object of the compared type",
                    );
                }
                let replacement = self.analyze_expression(&arguments[2])?;
                let replacement =
                    self.sync_operand_conversion(replacement, value_ty, arguments[2].span)?;
                let weak = self.analyze_atomic_control_argument(
                    &arguments[3],
                    operation.spelling(),
                    "weak flag",
                )?;
                let success = self.analyze_atomic_control_argument(
                    &arguments[4],
                    operation.spelling(),
                    "success memory order",
                )?;
                let failure = self.analyze_atomic_control_argument(
                    &arguments[5],
                    operation.spelling(),
                    "failure memory order",
                )?;
                self.validate_atomic_compare_exchange_orders(
                    operation.spelling(),
                    &success,
                    &failure,
                )?;
                let result = FullTypedExpression {
                    kind: FullTypedExpressionKind::AtomicCompareExchange {
                        pointer: Box::new(pointer),
                        expected: Box::new(expected_pointer),
                        replacement: Box::new(replacement),
                        object,
                        return_boolean: true,
                        expected_is_pointer: true,
                        order: sequentially_consistent,
                    },
                    ty: QualifiedType::unqualified(TypeId::BOOL),
                    category: ValueCategory::Value,
                    place: None,
                    constant: None,
                    constant_expression_kind: ConstantExpressionKind::Invalid,
                    span,
                };
                Ok(self.sequence_atomic_controls(vec![weak, success, failure], result, span))
            }
            syntax::AtomicBuiltinOperation::IsLockFree => unreachable!(),
            syntax::AtomicBuiltinOperation::ThreadFence
            | syntax::AtomicBuiltinOperation::SignalFence => unreachable!(),
        }
    }

    fn analyze_atomic_pointer(
        &mut self,
        spelling: &str,
        argument: &syntax::Expression,
        modifies: bool,
    ) -> AnalysisResult<(FullTypedExpression, QualifiedType)> {
        let pointer = self.analyze_expression(argument)?;
        let pointer = self.value_conversion(pointer)?;
        let Some(object) = self.pointer_pointee(pointer.ty.ty) else {
            return self.fail(
                "CCC2455",
                argument.span,
                format!("the first argument to `{spelling}` must point to an atomic scalar object"),
            );
        };
        if modifies && object.qualifiers.contains(TypeQualifiers::CONST) {
            return self.fail(
                "CCC2455",
                argument.span,
                format!("the first argument to `{spelling}` points to a const-qualified object"),
            );
        }
        let integer = self.types.is_integer(object.ty);
        let pointer_object = self.pointer_pointee(object.ty).is_some();
        let native_alignment = match self.native_atomic_object_alignment(object.ty) {
            Some(alignment) if integer || pointer_object => alignment,
            _ => {
                return self.fail(
                    "CCC2455",
                    argument.span,
                    format!(
                        "`{spelling}` requires a naturally aligned 1, 2, 4, or 8-byte integer or pointer object"
                    ),
                );
            }
        };
        if self
            .pointer_alignment_provenance(&pointer)
            .known_minimum()
            .is_some_and(|alignment| alignment < native_alignment)
        {
            return self.fail(
                "CCC2455",
                argument.span,
                format!("the first argument to `{spelling}` has weakened packed-member alignment"),
            );
        }
        Ok((pointer, object))
    }

    /// Returns the native alignment required by the selected scalar atomic
    /// instruction. Alignment-adjusted aliases may strengthen this contract,
    /// but may not weaken the representation's underlying alignment.
    fn native_atomic_object_alignment(&self, ty: TypeId) -> Option<u64> {
        let layout = self.types.layout_of(ty, self.config).ok()?;
        if !matches!(layout.size, 1 | 2 | 4 | 8) {
            return None;
        }
        let native_alignment = match self.types.try_kind(ty) {
            Some(TypeKind::AlignmentAdjusted(adjusted)) => {
                self.types
                    .layout_of(adjusted.underlying, self.config)
                    .ok()?
                    .align
            }
            _ => layout.align,
        };
        (layout.align >= native_alignment).then_some(native_alignment)
    }

    /// Recovers known alignment alternatives from the typed expression.
    /// An arbitrary pointer remains unknown because callers may supply a
    /// correctly aligned object even when its provenance is not locally
    /// visible. A conditional retains known alternatives even when another
    /// branch is unknown, so an arbitrary branch cannot mask a packed member.
    fn pointer_alignment_provenance(
        &self,
        expression: &FullTypedExpression,
    ) -> PointerAlignmentProvenance {
        match &expression.kind {
            FullTypedExpressionKind::GenericSelection { selected, .. } => {
                self.pointer_alignment_provenance(selected)
            }
            FullTypedExpressionKind::AddressOf(object) => self.lvalue_alignment_provenance(object),
            FullTypedExpressionKind::Conversion {
                kind: ConversionKind::ArrayToPointer,
                expression,
            } => self.lvalue_alignment_provenance(expression),
            FullTypedExpressionKind::Conversion {
                kind: ConversionKind::PointerConversion | ConversionKind::QualificationAdjustment,
                expression,
            }
            | FullTypedExpressionKind::BuiltinExpect {
                value: expression, ..
            } => self.pointer_alignment_provenance(expression),
            FullTypedExpressionKind::Binary {
                operator: syntax::BinaryOperator::Add | syntax::BinaryOperator::Subtract,
                left,
                right,
            } => {
                let base = if self.pointer_pointee(left.ty.ty).is_some() {
                    left
                } else if self.pointer_pointee(right.ty.ty).is_some() {
                    right
                } else {
                    return PointerAlignmentProvenance::Unknown;
                };
                let Some(element) = self.pointer_pointee(expression.ty.ty) else {
                    return PointerAlignmentProvenance::Unknown;
                };
                let Ok(layout) = self.types.layout_of(element.ty, self.config) else {
                    return PointerAlignmentProvenance::Unknown;
                };
                let stride = layout.size;
                self.pointer_alignment_provenance(base)
                    .map_known(|alignment| common_power_of_two_alignment(alignment, stride))
            }
            FullTypedExpressionKind::Conditional {
                then_expression,
                else_expression,
                ..
            } => self
                .pointer_alignment_provenance(then_expression)
                .merge(self.pointer_alignment_provenance(else_expression)),
            FullTypedExpressionKind::Comma(expressions) => expressions
                .last()
                .map_or(PointerAlignmentProvenance::Unknown, |expression| {
                    self.pointer_alignment_provenance(expression)
                }),
            _ => PointerAlignmentProvenance::Unknown,
        }
    }

    fn lvalue_alignment_provenance(
        &self,
        expression: &FullTypedExpression,
    ) -> PointerAlignmentProvenance {
        match &expression.kind {
            FullTypedExpressionKind::GenericSelection { selected, .. } => {
                self.lvalue_alignment_provenance(selected)
            }
            FullTypedExpressionKind::Member {
                base,
                field_index,
                indirect,
                ..
            } => {
                let record = if *indirect {
                    let Some(record) = self.pointer_pointee(base.ty.ty) else {
                        return PointerAlignmentProvenance::Unknown;
                    };
                    record.ty
                } else {
                    base.ty.ty
                };
                let Ok(layout) = self.types.layout_of(record, self.config) else {
                    return PointerAlignmentProvenance::Unknown;
                };
                let LayoutShape::Record(record) = layout.shape else {
                    return PointerAlignmentProvenance::Unknown;
                };
                let Some(field) = record.fields.get(*field_index) else {
                    return PointerAlignmentProvenance::Unknown;
                };
                if *indirect {
                    PointerAlignmentProvenance::known(field.align)
                } else {
                    self.lvalue_alignment_provenance(base)
                        .map_known(|base_alignment| {
                            field
                                .align
                                .min(common_power_of_two_alignment(base_alignment, field.offset))
                        })
                }
            }
            FullTypedExpressionKind::Subscript { base, .. }
            | FullTypedExpressionKind::Dereference(base) => self.pointer_alignment_provenance(base),
            FullTypedExpressionKind::DeclRef(_)
            | FullTypedExpressionKind::CompoundLiteral { .. }
            | FullTypedExpressionKind::StringLiteral(_) => self
                .types
                .layout_of(expression.ty.ty, self.config)
                .ok()
                .map_or(PointerAlignmentProvenance::Unknown, |layout| {
                    PointerAlignmentProvenance::known(layout.align)
                }),
            _ => PointerAlignmentProvenance::Unknown,
        }
    }

    fn analyze_atomic_control_argument(
        &mut self,
        argument: &syntax::Expression,
        spelling: &str,
        description: &str,
    ) -> AnalysisResult<FullTypedExpression> {
        let argument = self.analyze_expression(argument)?;
        let argument = self.value_conversion(argument)?;
        if !self.types.is_integer(argument.ty.ty) {
            return self.fail(
                "CCC2455",
                argument.span,
                format!("the {description} argument to `{spelling}` must have integer type"),
            );
        }
        Ok(argument)
    }

    fn sequence_atomic_controls(
        &self,
        mut controls: Vec<FullTypedExpression>,
        result: FullTypedExpression,
        span: Span,
    ) -> FullTypedExpression {
        let ty = result.ty;
        controls.push(result);
        FullTypedExpression {
            kind: FullTypedExpressionKind::Comma(controls),
            ty,
            category: ValueCategory::Value,
            place: None,
            constant: None,
            constant_expression_kind: ConstantExpressionKind::Invalid,
            span,
        }
    }

    fn validate_atomic_order(
        &mut self,
        spelling: &str,
        order: &FullTypedExpression,
        allowed: &[i128],
    ) -> AnalysisResult<()> {
        let Some(value) = order.constant.and_then(ConstantValue::as_i128) else {
            return Ok(());
        };
        if allowed.contains(&value) {
            Ok(())
        } else {
            self.fail(
                "CCC2455",
                order.span,
                format!("`{spelling}` does not permit memory order value {value}"),
            )
        }
    }

    fn validate_atomic_compare_exchange_orders(
        &mut self,
        spelling: &str,
        success: &FullTypedExpression,
        failure: &FullTypedExpression,
    ) -> AnalysisResult<()> {
        self.validate_atomic_order(spelling, success, &[0, 1, 2, 3, 4, 5])?;
        self.validate_atomic_order(spelling, failure, &[0, 1, 2, 5])?;
        let failure_span = failure.span;
        let Some(success) = success.constant.and_then(ConstantValue::as_i128) else {
            return Ok(());
        };
        let Some(failure) = failure.constant.and_then(ConstantValue::as_i128) else {
            return Ok(());
        };
        let permitted = match success {
            0 => matches!(failure, 0),
            1 => matches!(failure, 0 | 1),
            2 => matches!(failure, 0..=2),
            3 => matches!(failure, 0),
            4 => matches!(failure, 0..=2),
            5 => matches!(failure, 0 | 1 | 2 | 5),
            _ => false,
        };
        if permitted {
            Ok(())
        } else {
            self.fail(
                "CCC2455",
                failure_span,
                format!(
                    "`{spelling}` failure memory order {failure} is stronger than success order {success}"
                ),
            )
        }
    }

    fn sync_operand_conversion(
        &mut self,
        expression: FullTypedExpression,
        target: QualifiedType,
        span: Span,
    ) -> AnalysisResult<FullTypedExpression> {
        let expression = self.value_conversion(expression)?;
        let target_pointer = self.pointer_pointee(target.ty).is_some();
        let source_pointer = self.pointer_pointee(expression.ty.ty).is_some();
        if (target_pointer && (source_pointer || self.types.is_integer(expression.ty.ty)))
            || (self.types.is_integer(target.ty) && source_pointer)
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
        self.assignment_conversion(expression, target, span)
    }

    fn analyze_va_start(
        &mut self,
        list: &syntax::Expression,
        last_named_parameter: &syntax::Expression,
        span: Span,
    ) -> AnalysisResult<FullTypedExpression> {
        self.require_builtin("__builtin_va_start", span)?;
        let (variadic, expected, restriction) = self
            .function
            .as_ref()
            .map(|function| {
                (
                    function.variadic,
                    function.last_named_parameter,
                    function.last_named_parameter_restriction,
                )
            })
            .unwrap_or((false, None, None));
        if !variadic {
            return self.fail(
                "CCC2400",
                span,
                "`va_start` is only valid in a variadic function definition",
            );
        }
        let Some(expected) = expected else {
            return self.fail(
                "CCC2401",
                span,
                "`va_start` requires a final named parameter",
            );
        };
        let last = self.analyze_expression(last_named_parameter)?;
        if !matches!(last.kind, FullTypedExpressionKind::DeclRef(SymbolReference::Local(local)) if local == expected)
        {
            return self.fail(
                "CCC2402",
                last_named_parameter.span,
                "the second `va_start` argument must name the final fixed parameter",
            );
        }
        if let Some(restriction) = restriction {
            return self.fail(
                "CCC2413",
                last_named_parameter.span,
                format!(
                    "the final fixed parameter cannot be used by `va_start` because {restriction}"
                ),
            );
        }
        let list = self.analyze_va_list_operand(list, true)?;
        Ok(FullTypedExpression {
            kind: FullTypedExpressionKind::VaStart {
                list: Box::new(list),
                last_named_parameter: expected,
            },
            ty: QualifiedType::unqualified(TypeId::VOID),
            category: ValueCategory::Value,
            place: None,
            constant: None,
            constant_expression_kind: ConstantExpressionKind::Invalid,
            span,
        })
    }

    fn analyze_va_arg(
        &mut self,
        list: &syntax::Expression,
        type_name: &syntax::TypeName,
        span: Span,
    ) -> AnalysisResult<FullTypedExpression> {
        self.require_builtin("__builtin_va_arg", span)?;
        let list = self.analyze_va_list_operand(list, true)?;
        let (requested, variable_length_bounds) = self.resolve_type_name_with_bounds(type_name)?;
        self.validate_object_type(requested, type_name.span, true)?;
        if !variable_length_bounds.is_empty() || self.is_variably_modified(requested.ty) {
            return self.fail(
                "CCC2414",
                type_name.span,
                "`va_arg` cannot request a variably modified type",
            );
        }
        if matches!(self.types.try_kind(requested.ty), Some(TypeKind::Array(_))) {
            return self.fail(
                "CCC2405",
                type_name.span,
                "`va_arg` cannot produce an array type; request the promoted pointer type",
            );
        }
        let changed_by_default_promotions = requested.ty == TypeId::FLOAT
            || (self.types.is_integer(requested.ty)
                && self.integer_promotion_changes_type(requested.ty));
        if requested.qualifiers.contains(TypeQualifiers::ATOMIC) || changed_by_default_promotions {
            return self.fail(
                "CCC2403",
                type_name.span,
                format!(
                    "`va_arg` type `{}` is changed by the default argument promotions; request the promoted type",
                    self.types.display_qualified(requested)
                ),
            );
        }
        if self.config.target.data_layout.long_double_format == LongDoubleFormat::IeeeBinary128
            && self.type_contains_long_double(requested.ty, &mut HashSet::new())
        {
            return self.fail(
                "CCC2404",
                type_name.span,
                "`va_arg` does not support binary128 `long double` or an aggregate containing it",
            );
        }
        if !self.config.target.abi.supports_int128_values()
            && self.type_contains_int128(requested.ty, &mut HashSet::new())
        {
            return self.fail(
                "CCC2443",
                type_name.span,
                "`va_arg` does not support a 128-bit integer or an aggregate containing it",
            );
        }
        let layout = self
            .types
            .layout_of(requested.ty, self.config)
            .expect("a validated, non-variably-modified object type has a layout");
        let maximum_alignment = match self.config.target.abi {
            AbiIdentity::SysvAmd64Lp64 => 16,
            AbiIdentity::Aapcs64Lp64 | AbiIdentity::RiscvLp64d | AbiIdentity::DarwinArm64 => 16,
        };
        if layout.align > maximum_alignment {
            return self.fail(
                "CCC2406",
                type_name.span,
                format!(
                    "`va_arg` type `{}` requires unsupported {}-byte alignment",
                    self.types.display_qualified(requested),
                    layout.align
                ),
            );
        }
        Ok(FullTypedExpression {
            kind: FullTypedExpressionKind::VaArg {
                list: Box::new(list),
                requested,
            },
            ty: QualifiedType::unqualified(requested.ty),
            category: ValueCategory::Value,
            place: None,
            constant: None,
            constant_expression_kind: ConstantExpressionKind::Invalid,
            span,
        })
    }

    fn analyze_va_copy(
        &mut self,
        destination: &syntax::Expression,
        source: &syntax::Expression,
        span: Span,
    ) -> AnalysisResult<FullTypedExpression> {
        self.require_builtin("__builtin_va_copy", span)?;
        let destination = self.analyze_va_list_operand(destination, true)?;
        let source = self.analyze_va_list_operand(source, false)?;
        Ok(FullTypedExpression {
            kind: FullTypedExpressionKind::VaCopy {
                destination: Box::new(destination),
                source: Box::new(source),
            },
            ty: QualifiedType::unqualified(TypeId::VOID),
            category: ValueCategory::Value,
            place: None,
            constant: None,
            constant_expression_kind: ConstantExpressionKind::Invalid,
            span,
        })
    }

    fn analyze_va_end(
        &mut self,
        list: &syntax::Expression,
        span: Span,
    ) -> AnalysisResult<FullTypedExpression> {
        self.require_builtin("__builtin_va_end", span)?;
        let list = self.analyze_va_list_operand(list, true)?;
        Ok(FullTypedExpression {
            kind: FullTypedExpressionKind::VaEnd {
                list: Box::new(list),
            },
            ty: QualifiedType::unqualified(TypeId::VOID),
            category: ValueCategory::Value,
            place: None,
            constant: None,
            constant_expression_kind: ConstantExpressionKind::Invalid,
            span,
        })
    }

    fn require_builtin(&mut self, name: &str, span: Span) -> AnalysisResult<()> {
        if self
            .config
            .capabilities
            .is_available(CapabilityKind::Builtin, name)
        {
            Ok(())
        } else {
            self.fail(
                "CCC2407",
                span,
                format!("compiler builtin `{name}` is unavailable for this configuration"),
            )
        }
    }

    fn analyze_va_list_operand(
        &mut self,
        expression: &syntax::Expression,
        writable: bool,
    ) -> AnalysisResult<FullTypedExpression> {
        let va_list = self
            .types
            .target_builtin(TargetBuiltinType::VaList, self.config)
            .map_err(|error| {
                self.emit("CCC2408", expression.span, error.to_string());
            })?;
        let typed = self.analyze_expression(expression)?;
        if typed.ty.ty == va_list {
            if typed.category != ValueCategory::Lvalue
                || typed.place.as_ref().is_none_or(|place| !place.addressable)
            {
                return self.fail("CCC2409", expression.span, "a `va_list` object is required");
            }
            if writable && typed.ty.qualifiers.contains(TypeQualifiers::CONST) {
                return self.fail(
                    "CCC2410",
                    expression.span,
                    "this `va_list` operand must be modifiable",
                );
            }
            return Ok(typed);
        }
        if let TypeKind::Array(array) = self.types.kind(va_list).clone() {
            let parameter_pointer = self.types.pointer(array.element);
            if typed.ty.ty == parameter_pointer {
                return self.value_conversion(typed);
            }
        }
        self.fail(
            "CCC2411",
            expression.span,
            format!(
                "expected `va_list`, found `{}`",
                self.types.display_qualified(typed.ty)
            ),
        )
    }

    fn type_contains_long_double(&self, ty: TypeId, seen: &mut HashSet<TypeId>) -> bool {
        if ty == TypeId::LONG_DOUBLE {
            return true;
        }
        if !seen.insert(ty) {
            return false;
        }
        match self.types.try_kind(ty) {
            Some(TypeKind::Array(array)) => self.type_contains_long_double(array.element.ty, seen),
            Some(TypeKind::Record(record)) => self
                .types
                .record(*record)
                .and_then(|record| record.fields.as_ref())
                .is_some_and(|fields| {
                    fields
                        .iter()
                        .any(|field| self.type_contains_long_double(field.ty.ty, seen))
                }),
            _ => false,
        }
    }

    fn type_contains_int128(&self, ty: TypeId, seen: &mut HashSet<TypeId>) -> bool {
        if matches!(
            self.types.builtin_type(ty),
            Some(BuiltinType::Int128 | BuiltinType::UnsignedInt128)
        ) {
            return true;
        }
        if !seen.insert(ty) {
            return false;
        }
        match self.types.try_kind(ty) {
            Some(TypeKind::Array(array)) => self.type_contains_int128(array.element.ty, seen),
            Some(TypeKind::Record(record)) => self
                .types
                .record(*record)
                .and_then(|record| record.fields.as_ref())
                .is_some_and(|fields| {
                    fields
                        .iter()
                        .any(|field| self.type_contains_int128(field.ty.ty, seen))
                }),
            _ => false,
        }
    }

    fn analyze_identifier(
        &mut self,
        identifier: &syntax::Identifier,
    ) -> AnalysisResult<FullTypedExpression> {
        let Some(symbol) = self.lookup_ordinary(&identifier.name).cloned() else {
            if self.identifier_is_poisoned(identifier) {
                let constant = ConstantValue::Signed(0);
                return Ok(FullTypedExpression {
                    kind: FullTypedExpressionKind::Constant(constant),
                    ty: QualifiedType::unqualified(TypeId::INT),
                    category: ValueCategory::Value,
                    place: None,
                    constant: Some(constant),
                    constant_expression_kind: ConstantExpressionKind::Invalid,
                    span: identifier.span,
                });
            }
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
                Some(ConstantValue::Address(RelocatableAddress {
                    base: RelocatableBase::Global(id),
                    addend: 0,
                    one_past: false,
                })),
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
            OrdinarySymbol::Local(id, ty) => {
                let function = self.function.as_ref();
                let addressable =
                    function.is_none_or(|function| !function.unaddressable_locals.contains(&id));
                let constant = function.and_then(|function| {
                    function.static_duration_locals.get(&id).map(|_| {
                        ConstantValue::Address(RelocatableAddress {
                            base: RelocatableBase::BlockStatic {
                                function: function.id,
                                local: id,
                            },
                            addend: 0,
                            one_past: false,
                        })
                    })
                });
                (
                    SymbolReference::Local(id),
                    ty,
                    ValueCategory::Lvalue,
                    Some(self.object_place(PlaceBase::Local(id), ty, addressable)),
                    constant,
                )
            }
            OrdinarySymbol::TemporaryParameter(id, ty, addressable) => (
                SymbolReference::Local(id),
                ty,
                ValueCategory::Lvalue,
                Some(self.object_place(PlaceBase::Local(id), ty, addressable)),
                None,
            ),
            OrdinarySymbol::PredefinedFunctionName => {
                let (string, ty) = self.predefined_function_name_string();
                (
                    SymbolReference::PredefinedFunctionName(string),
                    ty,
                    ValueCategory::Lvalue,
                    Some(self.object_place(PlaceBase::String(string), ty, true)),
                    Some(ConstantValue::Address(RelocatableAddress {
                        base: RelocatableBase::String(string),
                        addend: 0,
                        one_past: false,
                    })),
                )
            }
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
            OrdinarySymbol::Poisoned => {
                let constant = ConstantValue::Signed(0);
                return Ok(FullTypedExpression {
                    kind: FullTypedExpressionKind::Constant(constant),
                    ty: QualifiedType::unqualified(TypeId::INT),
                    category: ValueCategory::Value,
                    place: None,
                    constant: Some(constant),
                    constant_expression_kind: ConstantExpressionKind::Invalid,
                    span: identifier.span,
                });
            }
        };
        let constant_expression_kind = if matches!(reference, SymbolReference::Enumerator { .. }) {
            ConstantExpressionKind::Integer
        } else {
            ConstantExpressionKind::Invalid
        };
        Ok(FullTypedExpression {
            kind: FullTypedExpressionKind::DeclRef(reference),
            ty,
            category,
            place,
            constant,
            constant_expression_kind,
            span: identifier.span,
        })
    }

    fn identifier_is_poisoned(&self, identifier: &syntax::Identifier) -> bool {
        self.poisoned_bindings.iter().any(|binding| {
            binding.name == identifier.name
                && binding.binding.file == identifier.span.file
                && identifier.span.start >= binding.binding.end
                && identifier.span.start < binding.scope_end
        })
    }

    fn poison_declaration_bindings(&mut self, declaration: &syntax::Declaration, file: bool) {
        for identifier in declaration
            .declarators
            .iter()
            .filter_map(|declarator| declarator.declarator.identifier())
        {
            let exists = if file {
                self.scopes.lookup_file_ordinary(&identifier.name).is_some()
            } else {
                self.scopes.current_ordinary(&identifier.name).is_some()
            };
            if exists {
                continue;
            }
            let result = if file {
                self.scopes
                    .bind_file_ordinary(identifier.name.clone(), OrdinarySymbol::Poisoned)
            } else {
                self.scopes
                    .bind_current_ordinary(identifier.name.clone(), OrdinarySymbol::Poisoned)
            };
            debug_assert!(result.is_ok());
        }
    }

    fn predefined_function_name_string(&mut self) -> (StringId, QualifiedType) {
        if let Some(string) = self
            .function
            .as_ref()
            .expect("the predefined function name is only visible inside a definition")
            .predefined_name_string
        {
            return (string, self.strings[string.0 as usize].ty);
        }
        let code_units = self
            .function
            .as_ref()
            .expect("the predefined function name is only visible inside a definition")
            .predefined_name_code_units
            .clone();
        let element = QualifiedType::new(TypeId::CHAR, TypeQualifiers::CONST);
        let ty = QualifiedType::unqualified(
            self.types.array(ArrayType {
                element,
                length: ArrayLength::Constant(
                    u64::try_from(code_units.len())
                        .expect("predefined function name length exceeds the C type model"),
                ),
            }),
        );
        let string =
            StringId(u32::try_from(self.strings.len()).expect("string identifier space exhausted"));
        self.strings.push(FullTypedString {
            id: string,
            prefix: StringLiteralPrefix::None,
            code_units,
            ty,
        });
        self.function
            .as_mut()
            .expect("the predefined function name is only visible inside a definition")
            .predefined_name_string = Some(string);
        (string, ty)
    }

    fn analyze_label_address(
        &mut self,
        label: &syntax::Identifier,
        span: Span,
    ) -> AnalysisResult<FullTypedExpression> {
        let (function, label_id) = {
            let Some(function) = self.function.as_mut() else {
                return self.fail(
                    "CCC2427",
                    span,
                    "a label address is only valid inside a function",
                );
            };
            let label_id = function.labels.note_use(&label.name, label.span);
            (function.id, label_id)
        };
        let pointer = self.types.pointer(QualifiedType::unqualified(TypeId::VOID));
        Ok(self.constant_expression(
            ConstantValue::Address(RelocatableAddress {
                base: RelocatableBase::Label {
                    function,
                    label: label_id,
                },
                addend: 0,
                one_past: false,
            }),
            QualifiedType::unqualified(pointer),
            span,
        ))
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
        let mut candidates: Vec<BuiltinType> =
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
        if self.config.target.abi.supports_int128_values() {
            if integer.suffix.unsigned {
                candidates.push(BuiltinType::UnsignedInt128);
            } else if integer.radix == 10 {
                candidates.push(BuiltinType::Int128);
            } else {
                candidates.extend([BuiltinType::Int128, BuiltinType::UnsignedInt128]);
            }
        }
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

    fn analyze_floating_literal(
        &mut self,
        floating: &FloatingConstant,
        span: Span,
    ) -> AnalysisResult<FullTypedExpression> {
        let (constant, ty) = match floating.suffix {
            FloatingConstantSuffix::Float => {
                let value = self.parse_apfloat::<Single>(&floating.number, span)?;
                let bits = u32::try_from(value.to_bits()).expect("binary32 bits fit in u32");
                (
                    ConstantValue::Floating(f64::from(f32::from_bits(bits))),
                    TypeId::FLOAT,
                )
            }
            FloatingConstantSuffix::Double => {
                let value = self.parse_apfloat::<Double>(&floating.number, span)?;
                let bits = u64::try_from(value.to_bits()).expect("binary64 bits fit in u64");
                (
                    ConstantValue::Floating(f64::from_bits(bits)),
                    TypeId::DOUBLE,
                )
            }
            FloatingConstantSuffix::LongDouble => {
                let format = self.config.target.data_layout.long_double_format;
                let constant = match format {
                    LongDoubleFormat::Binary64 => {
                        let value = self.parse_apfloat::<Double>(&floating.number, span)?;
                        let bits =
                            u64::try_from(value.to_bits()).expect("binary64 bits fit in u64");
                        ConstantValue::Floating(f64::from_bits(bits))
                    }
                    LongDoubleFormat::X87Extended => {
                        let value =
                            self.parse_apfloat::<X87DoubleExtended>(&floating.number, span)?;
                        ConstantValue::LongDouble(LongDoubleConstant::from_bits(
                            format,
                            value.to_bits(),
                        ))
                    }
                    LongDoubleFormat::IeeeBinary128 => {
                        let value = self.parse_apfloat::<Quad>(&floating.number, span)?;
                        ConstantValue::LongDouble(LongDoubleConstant::from_bits(
                            format,
                            value.to_bits(),
                        ))
                    }
                };
                (constant, TypeId::LONG_DOUBLE)
            }
        };
        Ok(self.constant_expression(constant, QualifiedType::unqualified(ty), span))
    }

    fn parse_apfloat<F: Float>(&mut self, number: &str, span: Span) -> AnalysisResult<F> {
        let parsed = match F::from_str_r(number, Round::NearestTiesToEven) {
            Ok(parsed) => parsed,
            Err(error) => {
                return self.fail(
                    "CCC2444",
                    span,
                    format!("invalid floating constant: {}", error.0),
                );
            }
        };
        if parsed.status.contains(Status::OVERFLOW) {
            return self.fail(
                "CCC2444",
                span,
                "floating constant is outside the range of its type",
            );
        }
        Ok(parsed.value)
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
            constant_expression_kind: ConstantExpressionKind::Invalid,
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
                let constant = match operand.constant {
                    Some(address @ ConstantValue::Address(_)) => Some(address),
                    _ => None,
                };
                Ok(FullTypedExpression {
                    kind: FullTypedExpressionKind::AddressOf(Box::new(operand)),
                    ty,
                    category: ValueCategory::Value,
                    place: None,
                    constant,
                    constant_expression_kind: ConstantExpressionKind::Invalid,
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
                    constant_expression_kind: ConstantExpressionKind::Invalid,
                    span,
                })
            }
            U::LogicalNot => {
                let operand = self.analyze_condition(operand)?;
                let constant = operand
                    .constant
                    .map(|value| ConstantValue::Signed(i128::from(value.is_zero())));
                let constant_expression_kind = integer_constant_expression_kind(&[&operand]);
                Ok(FullTypedExpression {
                    kind: FullTypedExpressionKind::Unary {
                        operator,
                        operand: Box::new(operand),
                    },
                    ty: QualifiedType::unqualified(TypeId::INT),
                    category: ValueCategory::Value,
                    place: None,
                    constant,
                    constant_expression_kind,
                    span,
                })
            }
            U::Plus | U::Minus | U::BitwiseNot => {
                let operand = self.analyze_expression(operand)?;
                let operand = self.value_conversion(operand)?;
                if has_direct_label_address_provenance(&operand) {
                    return self.fail(
                        "CCC2425",
                        span,
                        "arithmetic on a label address is not supported",
                    );
                }
                let operand =
                    self.integer_or_arithmetic_promotion(operand, operator != U::BitwiseNot, span)?;
                self.reject_long_double_operation(&[operand.ty], span)?;
                self.reject_int128_value_operation(&[operand.ty], span)?;
                let constant = evaluate_unary_constant(
                    operator,
                    operand.constant,
                    self.integer_constant_type(operand.ty.ty),
                );
                let constant_expression_kind = integer_constant_expression_kind(&[&operand]);
                Ok(FullTypedExpression {
                    kind: FullTypedExpressionKind::Unary {
                        operator,
                        operand: Box::new(operand.clone()),
                    },
                    ty: operand.ty,
                    category: ValueCategory::Value,
                    place: None,
                    constant,
                    constant_expression_kind,
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
        if !matches!(
            operator,
            B::Equal | B::NotEqual | B::LogicalAnd | B::LogicalOr
        ) && (has_direct_label_address_provenance(&left)
            || has_direct_label_address_provenance(&right))
        {
            return self.fail(
                "CCC2425",
                span,
                "arithmetic on a label address is not supported",
            );
        }
        if matches!(operator, B::LogicalAnd | B::LogicalOr) {
            let left = self.convert_to_boolean(left)?;
            let right = self.convert_to_boolean(right)?;
            let constant =
                evaluate_binary_constant(operator, left.constant, right.constant, None, false);
            let constant_expression_kind =
                logical_constant_expression_kind(operator, &left, &right);
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
                constant_expression_kind,
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
                let constant_expression_kind = integer_constant_expression_kind(&[&left, &right]);
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
                    constant_expression_kind,
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
                let constant_expression_kind = integer_constant_expression_kind(&[&left, &right]);
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
                    constant_expression_kind,
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
                    constant_expression_kind: ConstantExpressionKind::Invalid,
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
            let constant =
                evaluate_binary_constant(operator, left.constant, right.constant, None, false);
            let constant_expression_kind = integer_constant_expression_kind(&[&left, &right]);
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
                constant_expression_kind,
                span,
            });
        }

        if matches!(operator, B::LeftShift | B::RightShift) {
            if !self.types.is_integer(left.ty.ty) || !self.types.is_integer(right.ty.ty) {
                return self.fail("CCC2280", span, "shift operands must have integer type");
            }
            let left = self.integer_promote(left)?;
            let right = self.integer_promote(right)?;
            let constant = evaluate_binary_constant(
                operator,
                left.constant,
                right.constant,
                self.integer_constant_type(left.ty.ty),
                false,
            );
            let constant_expression_kind = integer_constant_expression_kind(&[&left, &right]);
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
                constant_expression_kind,
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
        self.reject_int128_value_operation(&[common], span)?;
        let result_ty = if matches!(
            operator,
            B::Less | B::LessEqual | B::Greater | B::GreaterEqual | B::Equal | B::NotEqual
        ) {
            QualifiedType::unqualified(TypeId::INT)
        } else {
            common
        };
        let mut constant = evaluate_binary_constant(
            operator,
            left.constant,
            right.constant,
            self.integer_constant_type(common.ty),
            matches!(common.ty, TypeId::FLOAT16 | TypeId::FLOAT),
        );
        if matches!(result_ty.ty, TypeId::FLOAT16 | TypeId::FLOAT) {
            constant = constant.and_then(|value| self.convert_constant(value, result_ty.ty));
        }
        let constant_expression_kind = integer_constant_expression_kind(&[&left, &right]);
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
            constant_expression_kind,
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
        if store.atomic && compound.is_some() {
            use syntax::AssignmentOperator as A;
            let pointer = self.pointer_pointee(target.ty.ty).is_some();
            let integer = self.types.is_integer(target.ty.ty)
                && self.types.builtin_type(target.ty.ty) != Some(BuiltinType::Bool);
            let supported = match operator {
                A::Add | A::Subtract => integer || pointer,
                A::BitwiseAnd | A::BitwiseXor | A::BitwiseOr => integer,
                _ => false,
            };
            if !supported {
                return self.fail(
                    "CCC2455",
                    span,
                    "this atomic compound assignment has no enabled native read-modify-write operation",
                );
            }
        }
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
            constant_expression_kind: ConstantExpressionKind::Invalid,
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
        if has_direct_label_address_provenance(&right) {
            return self.fail(
                "CCC2425",
                span,
                "arithmetic on a label address is not supported",
            );
        }
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
        self.reject_int128_value_operation(&[common], span)?;
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
        self.reject_int128_value_operation(&[operand.ty], span)?;
        let store = place.access;
        if store.atomic
            && !(self.pointer_pointee(operand.ty.ty).is_some()
                || (self.types.is_integer(operand.ty.ty)
                    && self.types.builtin_type(operand.ty.ty) != Some(BuiltinType::Bool)))
        {
            return self.fail(
                "CCC2455",
                span,
                "atomic increment requires a non-boolean integer or pointer object",
            );
        }
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
            constant_expression_kind: ConstantExpressionKind::Invalid,
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
        let constant_expression_kind =
            conditional_constant_expression_kind(&condition, &then_expression, &else_expression);
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
            constant_expression_kind,
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
        if has_direct_label_address_provenance(&base) || has_direct_label_address_provenance(&index)
        {
            return self.fail(
                "CCC2425",
                span,
                "arithmetic on a label address is not supported",
            );
        }
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
        let constant =
            self.evaluate_pointer_arithmetic(base.constant, index.constant, base.ty, false);
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
            constant,
            constant_expression_kind: ConstantExpressionKind::Invalid,
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
        #[derive(Clone)]
        struct MemberStep {
            record_ty: QualifiedType,
            field_index: usize,
            field: Field,
        }

        fn collect_member_paths(
            types: &ccc_types::TypeStore,
            record_ty: QualifiedType,
            member: &str,
            path: &mut Vec<MemberStep>,
            matches: &mut Vec<Vec<MemberStep>>,
        ) {
            let Some(TypeKind::Record(record_id)) = types.try_kind(record_ty.ty) else {
                return;
            };
            let Some(fields) = types
                .record(*record_id)
                .and_then(|record| record.fields.as_ref())
            else {
                return;
            };
            for (field_index, field) in fields.iter().enumerate() {
                let result_ty =
                    QualifiedType::new(field.ty.ty, field.ty.qualifiers | record_ty.qualifiers);
                path.push(MemberStep {
                    record_ty,
                    field_index,
                    field: field.clone(),
                });
                if field.name.as_deref() == Some(member) {
                    matches.push(path.clone());
                } else if field.name.is_none()
                    && matches!(types.try_kind(field.ty.ty), Some(TypeKind::Record(_)))
                {
                    collect_member_paths(types, result_ty, member, path, matches);
                }
                path.pop();
            }
        }

        let base = self.analyze_expression(base)?;
        let (base, record_ty, category, place) = if indirect {
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
        let Some(_fields) = record.fields else {
            return self.fail(
                "CCC2295",
                span,
                "member access uses an incomplete record type",
            );
        };
        let mut paths = Vec::new();
        collect_member_paths(
            &self.types,
            record_ty,
            &member.name,
            &mut Vec::new(),
            &mut paths,
        );
        if paths.is_empty() {
            return self.fail(
                "CCC2296",
                member.span,
                format!("record has no member named `{}`", member.name),
            );
        }
        if paths.len() != 1 {
            return self.fail(
                "CCC2296",
                member.span,
                format!(
                    "member `{}` is ambiguous through anonymous record members",
                    member.name
                ),
            );
        }

        let mut expression = base;
        let mut place = place;
        for (position, step) in paths.pop().unwrap().into_iter().enumerate() {
            let result_ty = QualifiedType::new(
                step.field.ty.ty,
                step.field.ty.qualifiers | step.record_ty.qualifiers,
            );
            let member_access = access_semantics(result_ty);
            let layout = self
                .types
                .layout_of(step.record_ty.ty, self.config)
                .map_err(|error| {
                    self.emit("CCC2297", span, error.to_string());
                })?;
            let LayoutShape::Record(layout) = layout.shape else {
                unreachable!("the queried type is a record")
            };
            let field_layout = &layout.fields[step.field_index];
            let bitfield = if step.field.bitfield.is_some() {
                let shared = field_layout
                    .bitfield
                    .expect("a semantic bitfield has a bitfield layout");
                let storage_offset = shared
                    .storage_offset
                    .checked_sub(field_layout.offset)
                    .expect("bitfield storage begins within its projected field");
                Some(BitfieldPlace {
                    field_index: step.field_index,
                    storage_offset,
                    storage_size: shared.storage_size,
                    storage_align: shared.storage_align,
                    bit_offset: shared.bit_offset,
                    width: shared.width,
                    signed: self.is_signed_integer(step.field.ty.ty),
                    access: member_access,
                })
            } else {
                None
            };
            let constant = if bitfield.is_none() {
                match expression.constant {
                    Some(ConstantValue::Address(mut address)) => {
                        address.addend = address
                            .addend
                            .checked_add(i128::from(field_layout.offset))
                            .ok_or_else(|| {
                                self.emit("CCC2297", span, "member address offset overflows");
                            })?;
                        Some(ConstantValue::Address(address))
                    }
                    _ => None,
                }
            } else {
                None
            };
            if let Some(place) = &mut place {
                place.projections.push(PlaceProjection::Field {
                    index: step.field_index,
                    name: step.field.name.clone(),
                });
                place.access = member_access;
                place.modifiable = self.is_modifiable_type(result_ty);
                if let Some(descriptor) = bitfield {
                    place.bitfield = Some(descriptor);
                    place.addressable = false;
                }
            }
            expression = FullTypedExpression {
                kind: FullTypedExpressionKind::Member {
                    base: Box::new(expression),
                    field_index: step.field_index,
                    name: step.field.name,
                    indirect: indirect && position == 0,
                    bitfield: bitfield.map(Box::new),
                },
                ty: result_ty,
                category,
                place: place.clone(),
                constant,
                constant_expression_kind: ConstantExpressionKind::Invalid,
                span,
            };
        }
        Ok(expression)
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
        self.reject_int128_boundary_type(signature.result.ty, span, "function return")?;
        if let FunctionParameters::Prototype(parameters) = &signature.parameters {
            for parameter in parameters {
                self.reject_int128_boundary_type(parameter.ty, span, "function parameter")?;
            }
        }
        self.reject_long_double_operation(&[signature.result], span)?;
        self.reject_int128_value_operation(&[signature.result], span)?;
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
                    if self.is_transparent_union(parameters[index].ty) {
                        self.transparent_union_argument(argument, parameters[index], argument_span)?
                    } else {
                        self.assignment_conversion(argument, parameters[index], argument_span)?
                    }
                }
                _ => self.default_argument_promotion(argument)?,
            };
            self.reject_long_double_operation(&[converted.ty], converted.span)?;
            self.reject_int128_value_operation(&[converted.ty], converted.span)?;
            self.reject_int128_boundary_type(converted.ty.ty, converted.span, "call argument")?;
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
            constant_expression_kind: ConstantExpressionKind::Invalid,
            span,
        })
    }

    fn is_transparent_union(&self, ty: TypeId) -> bool {
        let Some(TypeKind::Record(record)) = self.types.try_kind(ty) else {
            return false;
        };
        self.types.record(*record).is_some_and(|definition| {
            definition.kind == RecordKind::Union && definition.transparent_union
        })
    }

    fn transparent_union_argument(
        &mut self,
        argument: FullTypedExpression,
        target: QualifiedType,
        span: Span,
    ) -> AnalysisResult<FullTypedExpression> {
        let Some(TypeKind::Record(record_id)) = self.types.try_kind(target.ty).cloned() else {
            unreachable!("the caller checked the transparent union type")
        };
        let fields = self
            .types
            .record(record_id)
            .and_then(|record| record.fields.clone())
            .expect("a validated transparent union is complete");
        let argument = self.value_conversion(argument)?;
        if self.type_ids_compatible(target.ty, argument.ty.ty) {
            return self.assignment_conversion(argument, target, span);
        }
        let selected = fields.iter().enumerate().find(|(_, field)| {
            if argument.constant.is_some_and(ConstantValue::is_zero)
                && self.types.is_integer(argument.ty.ty)
            {
                return true;
            }
            self.pointer_pointee(argument.ty.ty).is_some()
                && self.pointers_assignment_compatible(field.ty.ty, argument.ty.ty)
        });
        let Some((field_index, field)) = selected else {
            return self.fail(
                "CCC2440",
                span,
                "argument is incompatible with every member of the transparent union parameter",
            );
        };
        let converted = self.assignment_conversion(argument, field.ty, span)?;
        let scalar = FullTypedInitializer {
            ty: field.ty,
            kind: FullTypedInitializerKind::Scalar(Box::new(converted)),
            span,
        };
        let initializer = FullTypedInitializer {
            ty: target,
            kind: FullTypedInitializerKind::Aggregate(vec![FullTypedInitializerEntry {
                path: vec![InitializerPathElement::Field {
                    index: field_index,
                    name: field.name.clone(),
                    bitfield: None,
                }],
                initializer: Box::new(scalar),
            }]),
            span,
        };
        let local = self.fresh_local();
        let literal = FullTypedExpression {
            kind: FullTypedExpressionKind::CompoundLiteral {
                storage: CompoundLiteralStorage::Automatic(local),
                initializer: Box::new(initializer),
            },
            ty: target,
            category: ValueCategory::Lvalue,
            place: Some(self.object_place(PlaceBase::CompoundLiteral(local), target, true)),
            constant: None,
            constant_expression_kind: ConstantExpressionKind::Invalid,
            span,
        };
        self.value_conversion(literal)
    }

    fn analyze_sizeof(
        &mut self,
        operand_ty: QualifiedType,
        operand: Option<Box<FullTypedExpression>>,
        span: Span,
    ) -> AnalysisResult<FullTypedExpression> {
        let size = if runtime_sized_array(&self.types, operand_ty.ty) {
            None
        } else {
            Some(
                self.types
                    .layout_of(operand_ty.ty, self.config)
                    .map_err(|error| {
                        self.emit("CCC2301", span, error.to_string());
                    })?
                    .size,
            )
        };
        let result_ty = QualifiedType::unqualified(self.size_type());
        Ok(FullTypedExpression {
            kind: FullTypedExpressionKind::Sizeof {
                operand,
                operand_ty,
                size,
            },
            ty: result_ty,
            category: ValueCategory::Value,
            place: None,
            constant: size.map(|size| ConstantValue::Unsigned(u128::from(size))),
            constant_expression_kind: if size.is_some() {
                ConstantExpressionKind::Integer
            } else {
                ConstantExpressionKind::Invalid
            },
            span,
        })
    }

    fn with_variable_length_bounds(
        &self,
        bounds: Vec<FullTypedVariableLengthBound>,
        expression: FullTypedExpression,
        span: Span,
    ) -> FullTypedExpression {
        if bounds.is_empty() {
            return expression;
        }
        FullTypedExpression {
            kind: FullTypedExpressionKind::VariableLengthBoundEvaluation {
                bounds,
                expression: Box::new(expression.clone()),
            },
            ty: expression.ty,
            category: expression.category,
            place: expression.place,
            constant: None,
            constant_expression_kind: ConstantExpressionKind::Invalid,
            span,
        }
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
        let (record_ty, variable_length_bounds) = self.resolve_type_name_with_bounds(type_name)?;
        if !variable_length_bounds.is_empty() || self.is_variably_modified(record_ty.ty) {
            return self.fail(
                "CCC2302",
                type_name.span,
                "offsetof requires a non-variably-modified record type",
            );
        }
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
            constant_expression_kind: ConstantExpressionKind::Integer,
            span,
        })
    }

    fn analyze_initializer(
        &mut self,
        ty: QualifiedType,
        initializer: &syntax::Initializer,
    ) -> AnalysisResult<(FullTypedInitializer, QualifiedType)> {
        self.reject_int128_value_operation(&[ty], initializer.span())?;
        match initializer {
            syntax::Initializer::Expression(expression) => {
                let expression = self.analyze_expression(expression)?;
                if !matches!(self.types.try_kind(ty.ty), Some(TypeKind::Array(_)))
                    && self.type_ids_compatible(ty.ty, expression.ty.ty)
                    && let Some(initializer) = static_compound_literal_initializer(&expression)
                {
                    let mut initializer = initializer.clone();
                    initializer.ty = ty;
                    return Ok((initializer, ty));
                }
                if let Some(TypeKind::Array(array)) = self.types.try_kind(ty.ty).cloned()
                    && let FullTypedExpressionKind::StringLiteral(string) = expression.kind
                {
                    let literal_ty = self.strings[string.0 as usize].ty;
                    let Some(TypeKind::Array(literal_array)) =
                        self.types.try_kind(literal_ty.ty).cloned()
                    else {
                        unreachable!("a string literal has array type")
                    };
                    let ordinary_character_string = literal_array.element.ty == TypeId::CHAR
                        && matches!(
                            array.element.ty,
                            TypeId::CHAR | TypeId::SIGNED_CHAR | TypeId::UNSIGNED_CHAR
                        );
                    if !ordinary_character_string
                        && !self.type_ids_compatible(array.element.ty, literal_array.element.ty)
                    {
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
                        ArrayLength::Variable(_) | ArrayLength::UnspecifiedVariable(_) => {
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
                        kind: FullTypedInitializerKind::Scalar(Box::new(converted)),
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
                        if let [entry] = entries.as_slice()
                            && entry.designation.is_empty()
                            && let syntax::Initializer::Expression(expression) = &entry.initializer
                            && self.string_literal_matches_array(&array, expression.as_ref())
                        {
                            return self.analyze_initializer(ty, &entry.initializer);
                        }
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

    fn string_literal_matches_array(
        &self,
        array: &ArrayType,
        expression: &syntax::Expression,
    ) -> bool {
        let Some(literal) = initializer_string_literal(expression) else {
            return false;
        };
        let literal_element = match literal.prefix {
            StringLiteralPrefix::None | StringLiteralPrefix::Utf8 => TypeId::CHAR,
            StringLiteralPrefix::Wide => self.wchar_type(),
            StringLiteralPrefix::Utf16 => TypeId::UNSIGNED_SHORT,
            StringLiteralPrefix::Utf32 => TypeId::UNSIGNED_INT,
        };
        matches!(
            literal.prefix,
            StringLiteralPrefix::None | StringLiteralPrefix::Utf8
        ) && matches!(
            array.element.ty,
            TypeId::CHAR | TypeId::SIGNED_CHAR | TypeId::UNSIGNED_CHAR
        ) || self.type_ids_compatible(array.element.ty, literal_element)
    }

    fn analyze_array_initializer(
        &mut self,
        ty: QualifiedType,
        array: ArrayType,
        entries: &[syntax::InitializerEntry],
        span: Span,
    ) -> AnalysisResult<(FullTypedInitializer, QualifiedType)> {
        if matches!(
            array.length,
            ArrayLength::Variable(_) | ArrayLength::UnspecifiedVariable(_)
        ) {
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
            if matches!(
                self.types.try_kind(fields[selected_field].ty.ty),
                Some(TypeKind::Array(ArrayType {
                    length: ArrayLength::Incomplete,
                    ..
                }))
            ) {
                return self.fail(
                    "CCC2431",
                    entry.span,
                    "a flexible array member cannot be initialized",
                );
            }
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
        let field_layout = &layout.fields[field_index];
        let shared = field_layout
            .bitfield
            .expect("a semantic bitfield has a shared layout descriptor");
        let storage_offset = shared
            .storage_offset
            .checked_sub(field_layout.offset)
            .expect("bitfield storage begins within its projected field");
        let field_ty = QualifiedType::new(field.ty.ty, field.ty.qualifiers | record_ty.qualifiers);
        let access = access_semantics(field_ty);
        Ok(Some(BitfieldPlace {
            field_index,
            storage_offset,
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
                self.reject_int128_value_operation(&[expression.ty], expression.span)?;
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
        let target = self.promoted_integer_type_for_expression(&expression);
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
        } else if left.ty.ty == TypeId::FLOAT16 || right.ty.ty == TypeId::FLOAT16 {
            TypeId::FLOAT16
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
            self.reject_int128_value_operation(&[target, expression.ty], span)?;
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
        if self.type_ids_compatible(target.ty, expression.ty.ty) {
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
        let operand_kind = expression.constant_expression_kind;
        let target_is_integer = self.types.is_integer(target.ty);
        let mark_explicit_cast = |mut converted: FullTypedExpression| {
            converted.constant_expression_kind = match (target_is_integer, operand_kind) {
                (
                    true,
                    ConstantExpressionKind::Integer | ConstantExpressionKind::FloatingLiteral,
                ) => ConstantExpressionKind::Integer,
                (true, ConstantExpressionKind::UnevaluatedOnly) => {
                    ConstantExpressionKind::UnevaluatedOnly
                }
                _ => ConstantExpressionKind::Invalid,
            };
            converted
        };
        if target.ty == TypeId::VOID {
            return Ok(mark_explicit_cast(self.conversion(
                ConversionKind::ToVoid,
                expression,
                QualifiedType::unqualified(TypeId::VOID),
                None,
            )));
        }
        self.reject_long_double_operation(&[target, expression.ty], span)?;
        self.reject_int128_value_operation(&[target, expression.ty], span)?;
        if target.ty == TypeId::BOOL
            && (self.types.is_arithmetic(expression.ty.ty)
                || self.pointer_pointee(expression.ty.ty).is_some())
        {
            let constant = expression
                .constant
                .and_then(|value| self.convert_constant(value, target.ty));
            return Ok(mark_explicit_cast(self.conversion(
                ConversionKind::ToBoolean,
                expression,
                QualifiedType::unqualified(target.ty),
                constant,
            )));
        }
        if self.types.is_arithmetic(target.ty) && self.types.is_arithmetic(expression.ty.ty) {
            let converted =
                self.arithmetic_conversion(expression, QualifiedType::unqualified(target.ty))?;
            return Ok(mark_explicit_cast(converted));
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
            return Ok(mark_explicit_cast(self.conversion(
                ConversionKind::PointerConversion,
                expression,
                QualifiedType::unqualified(target.ty),
                constant,
            )));
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
        let constant_expression_kind = match kind {
            ConversionKind::IntegerPromotion
            | ConversionKind::IntegerConversion
            | ConversionKind::ToBoolean => match expression.constant_expression_kind {
                ConstantExpressionKind::Integer => ConstantExpressionKind::Integer,
                ConstantExpressionKind::UnevaluatedOnly => ConstantExpressionKind::UnevaluatedOnly,
                ConstantExpressionKind::Invalid | ConstantExpressionKind::FloatingLiteral => {
                    ConstantExpressionKind::Invalid
                }
            },
            _ => ConstantExpressionKind::Invalid,
        };
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
            constant_expression_kind,
            span,
        }
    }

    fn constant_expression(
        &self,
        constant: ConstantValue,
        ty: QualifiedType,
        span: Span,
    ) -> FullTypedExpression {
        let constant_expression_kind = match constant {
            ConstantValue::Signed(_) | ConstantValue::Unsigned(_) => {
                ConstantExpressionKind::Integer
            }
            ConstantValue::Floating(_) | ConstantValue::LongDouble(_) => {
                ConstantExpressionKind::FloatingLiteral
            }
            ConstantValue::NullPointer | ConstantValue::Address(_) => {
                ConstantExpressionKind::Invalid
            }
        };
        FullTypedExpression {
            kind: FullTypedExpressionKind::Constant(constant),
            ty,
            category: ValueCategory::Value,
            place: None,
            constant: Some(constant),
            constant_expression_kind,
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

    fn evaluate_switch_case(
        &mut self,
        expression: &syntax::Expression,
        controlling_type: QualifiedType,
    ) -> AnalysisResult<u128> {
        let typed = self.analyze_expression(expression)?;
        let typed = self.value_conversion(typed)?;
        if !self.types.is_integer(typed.ty.ty) {
            return self.fail(
                "CCC2339",
                expression.span,
                "case expression must have integer type",
            );
        }
        if typed.constant_expression_kind != ConstantExpressionKind::Integer {
            return self.fail(
                "CCC2338",
                expression.span,
                "case expression cannot be evaluated as an integer constant",
            );
        }
        let converted = self.arithmetic_conversion(typed, controlling_type)?;
        let raw = match converted.constant {
            Some(ConstantValue::Signed(value)) => value as u128,
            Some(ConstantValue::Unsigned(value)) => value,
            _ => {
                return self.fail(
                    "CCC2338",
                    expression.span,
                    "case expression cannot be evaluated as an integer constant",
                );
            }
        };
        let kind = self
            .integer_representation(controlling_type.ty)
            .unwrap_or(BuiltinType::Int);
        Ok(truncate_to_width(raw, self.integer_width(kind)))
    }

    fn try_evaluate_integer_constant(
        &mut self,
        expression: &syntax::Expression,
    ) -> AnalysisResult<Option<i128>> {
        let (_, constant) = self.analyze_integer_constant_candidate(expression)?;
        Ok(constant)
    }

    fn analyze_integer_constant_candidate(
        &mut self,
        expression: &syntax::Expression,
    ) -> AnalysisResult<(FullTypedExpression, Option<i128>)> {
        let typed = self.analyze_expression(expression)?;
        let typed = self.value_conversion(typed)?;
        if !self.types.is_integer(typed.ty.ty) {
            return self.fail(
                "CCC2339",
                expression.span,
                "constant expression must have integer type",
            );
        }
        let constant = if typed.constant_expression_kind == ConstantExpressionKind::Integer {
            let Some(constant) = typed.constant.and_then(ConstantValue::as_i128) else {
                return self.fail(
                    "CCC2338",
                    expression.span,
                    "integer constant expression cannot be evaluated",
                );
            };
            Some(constant)
        } else {
            None
        };
        Ok((typed, constant))
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
        if let Some(target) = self.common_pointer_type(left.ty.ty, right.ty.ty) {
            let left = if left.ty.ty == target.ty {
                left
            } else {
                let constant = left.constant;
                self.conversion(ConversionKind::PointerConversion, left, target, constant)
            };
            let right = if right.ty.ty == target.ty {
                right
            } else {
                let constant = right.constant;
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

    fn common_pointer_type(&mut self, left: TypeId, right: TypeId) -> Option<QualifiedType> {
        let left = self.pointer_pointee(left)?;
        let right = self.pointer_pointee(right)?;
        let pointee = if self.type_ids_compatible(left.ty, right.ty) {
            self.composite_type_id(left.ty, right.ty)?
        } else if left.ty == TypeId::VOID || right.ty == TypeId::VOID {
            TypeId::VOID
        } else {
            return None;
        };
        let pointee = QualifiedType::new(pointee, left.qualifiers | right.qualifiers);
        Some(QualifiedType::unqualified(self.types.pointer(pointee)))
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

    fn promoted_integer_type_for_expression(&self, expression: &FullTypedExpression) -> TypeId {
        let ordinary = self.promoted_integer_type(expression.ty.ty);
        if self.types.builtin_type(expression.ty.ty) == Some(BuiltinType::UnsignedInt)
            && loaded_bitfield(expression).is_some_and(|bitfield| {
                bitfield.width < u32::from(self.integer_width(BuiltinType::Int))
            })
        {
            TypeId::INT
        } else {
            ordinary
        }
    }

    fn integer_promotion_changes_type(&self, ty: TypeId) -> bool {
        let promoted = self.promoted_integer_type(ty);
        match self.types.try_kind(ty) {
            Some(TypeKind::Enum(id)) => self
                .types
                .enumeration(*id)
                .and_then(|enumeration| enumeration.body.as_ref())
                .is_none_or(|body| body.underlying != promoted),
            _ => !self.type_ids_compatible(promoted, ty),
        }
    }

    fn common_arithmetic_type(&self, left: TypeId, right: TypeId) -> TypeId {
        if left == TypeId::LONG_DOUBLE || right == TypeId::LONG_DOUBLE {
            TypeId::LONG_DOUBLE
        } else if left == TypeId::DOUBLE || right == TypeId::DOUBLE {
            TypeId::DOUBLE
        } else if left == TypeId::FLOAT || right == TypeId::FLOAT {
            TypeId::FLOAT
        } else if left == TypeId::FLOAT16 || right == TypeId::FLOAT16 {
            TypeId::FLOAT16
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
                ConstantValue::Floating(value) if self.integer_kind_is_signed(kind) => {
                    value as i128 as u128
                }
                ConstantValue::Floating(value) => value as u128,
                ConstantValue::LongDouble(value) => {
                    long_double_to_integer(value, width, self.integer_kind_is_signed(kind))?
                }
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
            Some(
                BuiltinType::Float16
                    | BuiltinType::Float
                    | BuiltinType::Double
                    | BuiltinType::LongDouble
            )
        ) {
            let target = self.types.builtin_type(target)?;
            if target == BuiltinType::LongDouble {
                match self.config.target.data_layout.long_double_format {
                    LongDoubleFormat::Binary64 => {}
                    format => return constant_to_long_double(value, format),
                }
            }
            let value = match (target, value) {
                (BuiltinType::Float16, ConstantValue::LongDouble(value)) => {
                    return long_double_to_float16(value)
                        .map(|value| ConstantValue::Floating(float16_to_f64(value)));
                }
                (BuiltinType::Float, ConstantValue::Signed(value)) => f64::from(value as f32),
                (BuiltinType::Float, ConstantValue::Unsigned(value)) => f64::from(value as f32),
                (BuiltinType::Float, ConstantValue::Floating(value)) => f64::from(value as f32),
                (BuiltinType::Float, ConstantValue::LongDouble(value)) => {
                    long_double_to_f32(value)?
                }
                (_, ConstantValue::Signed(value)) => value as f64,
                (_, ConstantValue::Unsigned(value)) => value as f64,
                (_, ConstantValue::Floating(value)) => value,
                (_, ConstantValue::LongDouble(value)) => long_double_to_f64(value)?,
                (_, ConstantValue::NullPointer | ConstantValue::Address(_)) => return None,
            };
            let value = if target == BuiltinType::Float16 {
                float16_to_f64(f64_to_float16(value))
            } else {
                value
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
            BuiltinType::Int128 | BuiltinType::UnsignedInt128 => 128,
            BuiltinType::Void
            | BuiltinType::Float16
            | BuiltinType::Float
            | BuiltinType::Double
            | BuiltinType::LongDouble => 0,
        }
    }

    fn integer_constant_type(&self, ty: TypeId) -> Option<IntegerConstantType> {
        let kind = self.integer_representation(ty)?;
        Some(IntegerConstantType {
            width: self.integer_width(kind),
            signed: self.integer_kind_is_signed(kind),
        })
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
        if self.config.target.abi.supports_int128_values()
            && self.signed_range_fits(BuiltinType::Int128, minimum, maximum)
        {
            return Some(TypeId::INT128);
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
        let left = self.types.without_alignment_adjustment(left);
        let right = self.types.without_alignment_adjustment(right);
        if left == right {
            return true;
        }
        match (self.types.try_kind(left), self.types.try_kind(right)) {
            (Some(TypeKind::Pointer(left)), Some(TypeKind::Pointer(right))) => {
                self.types_compatible(left.pointee, right.pointee)
            }
            (Some(TypeKind::Array(left)), Some(TypeKind::Array(right))) => {
                self.types_compatible(left.element, right.element)
                    && match (left.length, right.length) {
                        (ArrayLength::Constant(left), ArrayLength::Constant(right)) => {
                            left == right
                        }
                        _ => true,
                    }
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

        if self.types.without_alignment_adjustment(left)
            == self.types.without_alignment_adjustment(right)
        {
            return Some(right);
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
                    (ArrayLength::Constant(left), ArrayLength::Constant(right)) => {
                        if left != right {
                            return None;
                        }
                        ArrayLength::Constant(left)
                    }
                    (ArrayLength::Constant(length), _) | (_, ArrayLength::Constant(length)) => {
                        ArrayLength::Constant(length)
                    }
                    (ArrayLength::Variable(bound), _) | (_, ArrayLength::Variable(bound)) => {
                        ArrayLength::Variable(bound)
                    }
                    (
                        ArrayLength::UnspecifiedVariable(bound),
                        ArrayLength::UnspecifiedVariable(_),
                    ) => ArrayLength::UnspecifiedVariable(bound),
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
                    self.type_ids_compatible(self.promoted_integer_type(parameter.ty), parameter.ty)
                }
                _ => !matches!(self.types.try_kind(parameter.ty), Some(TypeKind::Enum(_))),
            }
        })
    }

    fn type_contains_flexible_array_member(&self, ty: TypeId) -> bool {
        fn contains(
            types: &ccc_types::TypeStore,
            ty: TypeId,
            active: &mut HashSet<TypeId>,
        ) -> bool {
            if !active.insert(ty) {
                return false;
            }
            let result = match types.try_kind(ty) {
                Some(TypeKind::Array(array)) => contains(types, array.element.ty, active),
                Some(TypeKind::Record(record_id)) => types
                    .record(*record_id)
                    .and_then(|record| record.fields.as_ref())
                    .is_some_and(|fields| {
                        fields.iter().any(|field| {
                            matches!(
                                types.try_kind(field.ty.ty),
                                Some(TypeKind::Array(ArrayType {
                                    length: ArrayLength::Incomplete,
                                    ..
                                }))
                            ) || contains(types, field.ty.ty, active)
                        })
                    }),
                Some(
                    TypeKind::Builtin(_)
                    | TypeKind::Pointer(_)
                    | TypeKind::Function(_)
                    | TypeKind::Enum(_)
                    | TypeKind::AlignmentAdjusted(_),
                )
                | None => false,
            };
            active.remove(&ty);
            result
        }

        contains(&self.types, ty, &mut HashSet::new())
    }

    fn field_contributes_named_member(&self, field: &Field) -> bool {
        fn record_has_named_member(
            types: &ccc_types::TypeStore,
            ty: TypeId,
            active: &mut HashSet<TypeId>,
        ) -> bool {
            let Some(TypeKind::Record(record_id)) = types.try_kind(ty) else {
                return false;
            };
            if !active.insert(ty) {
                return false;
            }
            let result = types
                .record(*record_id)
                .and_then(|record| record.fields.as_ref())
                .is_some_and(|fields| {
                    fields.iter().any(|field| {
                        field.name.is_some() || record_has_named_member(types, field.ty.ty, active)
                    })
                });
            active.remove(&ty);
            result
        }

        field.name.is_some()
            || record_has_named_member(&self.types, field.ty.ty, &mut HashSet::new())
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
        if matches!(self.types.try_kind(ty.ty), Some(TypeKind::Array(_)))
            && self.type_contains_flexible_array_member(ty.ty)
        {
            return self.fail(
                "CCC2370",
                span,
                "an array element cannot contain a flexible array member",
            );
        }
        match self.types.layout_of(ty.ty, self.config) {
            Ok(_) => Ok(()),
            Err(ccc_types::LayoutError::IncompleteArray(_)) if !require_complete => Ok(()),
            Err(ccc_types::LayoutError::VariableLengthArray { .. }) => Ok(()),
            Err(error) => self.fail("CCC2342", span, error.to_string()),
        }
    }

    fn is_variably_modified(&self, ty: TypeId) -> bool {
        fn visit(types: &TypeStore, ty: TypeId, active: &mut HashSet<TypeId>) -> bool {
            if !active.insert(ty) {
                return false;
            }
            let result = match types.try_kind(ty) {
                Some(TypeKind::Array(array)) => {
                    matches!(
                        array.length,
                        ArrayLength::Variable(_) | ArrayLength::UnspecifiedVariable(_)
                    ) || visit(types, array.element.ty, active)
                }
                Some(TypeKind::Pointer(pointer)) => visit(types, pointer.pointee.ty, active),
                // Function parameters may have variably modified types in a
                // prototype. Only a variably modified result makes the
                // function type itself variably modified for these
                // declaration constraints.
                Some(TypeKind::Function(signature)) => visit(types, signature.result.ty, active),
                Some(TypeKind::AlignmentAdjusted(adjusted)) => {
                    visit(types, adjusted.underlying, active)
                }
                _ => false,
            };
            active.remove(&ty);
            result
        }

        visit(&self.types, ty, &mut HashSet::new())
    }

    fn has_runtime_and_zero_array_dimensions(&self, ty: TypeId) -> bool {
        let mut current = ty;
        let mut has_runtime_bound = false;
        let mut has_zero_length = false;
        while let Some(TypeKind::Array(array)) = self
            .types
            .try_kind(self.types.without_alignment_adjustment(current))
        {
            match array.length {
                ArrayLength::Constant(0) => has_zero_length = true,
                ArrayLength::Variable(_) | ArrayLength::UnspecifiedVariable(_) => {
                    has_runtime_bound = true;
                }
                ArrayLength::Constant(_) | ArrayLength::Incomplete => {}
            }
            if has_runtime_bound && has_zero_length {
                return true;
            }
            current = array.element.ty;
        }
        false
    }

    fn requires_runtime_sized_storage(&self, ty: TypeId) -> bool {
        match self.types.try_kind(ty) {
            Some(TypeKind::Array(array)) => {
                matches!(
                    array.length,
                    ArrayLength::Variable(_) | ArrayLength::UnspecifiedVariable(_)
                ) || self.requires_runtime_sized_storage(array.element.ty)
            }
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
        if width == 128 && self.config.target.abi.supports_int128_values() {
            TypeId::INT128
        } else if width == layout.int_width {
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
        if width == 128 && self.config.target.abi.supports_int128_values() {
            TypeId::UNSIGNED_INT128
        } else if width == layout.int_width {
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
        if self.config.target.data_layout.long_double_format == LongDoubleFormat::IeeeBinary128
            && types.iter().any(|ty| ty.ty == TypeId::LONG_DOUBLE)
        {
            self.fail(
                "CCC2343",
                span,
                "binary128 long double layout is supported, but this operation requires unavailable arithmetic or ABI support",
            )
        } else {
            Ok(())
        }
    }

    fn reject_int128_value_operation(
        &mut self,
        types: &[QualifiedType],
        span: Span,
    ) -> AnalysisResult<()> {
        if !self.config.target.abi.supports_int128_values()
            && types
                .iter()
                .any(|ty| self.type_contains_int128(ty.ty, &mut HashSet::new()))
        {
            self.fail(
                "CCC2443",
                span,
                "128-bit integer layout is supported, but value operations are not enabled",
            )
        } else {
            Ok(())
        }
    }

    fn reject_int128_boundary_type(
        &mut self,
        ty: TypeId,
        span: Span,
        boundary: &str,
    ) -> AnalysisResult<()> {
        if !self.config.target.abi.supports_int128_values()
            && self.type_contains_int128(ty, &mut HashSet::new())
        {
            self.fail(
                "CCC2443",
                span,
                format!(
                    "{boundary} type `{}` contains a 128-bit integer with no enabled ABI transport",
                    self.types.display(ty)
                ),
            )
        } else {
            Ok(())
        }
    }

    fn reject_unavailable_thread_storage(
        &mut self,
        thread_local: bool,
        span: Span,
    ) -> AnalysisResult<()> {
        if thread_local && !self.config.target.abi.supports_tls_codegen() {
            return self.fail(
                "CCC2441",
                span,
                format!(
                    "thread-local storage is not enabled for target ABI `{}`",
                    self.config.target.abi.name()
                ),
            );
        }
        Ok(())
    }

    fn apply_declarator_type_attributes(
        &mut self,
        mut ty: QualifiedType,
        attributes: &[FullTypedAttribute],
        span: Span,
    ) -> AnalysisResult<QualifiedType> {
        for attribute in attributes {
            if !attribute_has_name(attribute, "mode") {
                continue;
            }
            let Some(mode) = attribute_argument_identifier(&attribute.arguments) else {
                return self.fail(
                    "CCC2421",
                    span,
                    "implemented `mode` requires one identifier argument",
                );
            };
            if mode != "word" {
                return self.fail(
                    "CCC2421",
                    span,
                    format!("integer machine mode `{mode}` is not supported"),
                );
            }
            let Some(kind) = self.types.builtin_type(ty.ty) else {
                return self.fail(
                    "CCC2421",
                    span,
                    "implemented `mode(word)` requires an integer type",
                );
            };
            if !kind.is_integer() {
                return self.fail(
                    "CCC2421",
                    span,
                    "implemented `mode(word)` requires an integer type",
                );
            }
            ty.ty = if self.integer_kind_is_signed(kind) {
                self.signed_integer_for_width(self.config.target.data_layout.pointer_width)
            } else {
                self.unsigned_integer_for_width(self.config.target.data_layout.pointer_width)
            };
        }
        Ok(ty)
    }

    fn apply_typedef_alignment(
        &mut self,
        ty: QualifiedType,
        attributes: &[FullTypedAttribute],
        defines_private_record: bool,
        span: Span,
    ) -> AnalysisResult<QualifiedType> {
        let mut requested = None;
        for attribute in attributes {
            if !attribute_has_name(attribute, "aligned") {
                continue;
            }
            let alignment = if attribute.arguments.is_empty() {
                self.maximum_supported_alignment()
            } else if let [argument] = attribute.arguments.as_slice() {
                let Some(alignment) = argument.parse::<u64>().ok() else {
                    return self.fail(
                        "CCC2422",
                        span,
                        "implemented typedef `aligned` requires an integer argument",
                    );
                };
                alignment
            } else {
                return self.fail(
                    "CCC2422",
                    span,
                    "implemented typedef `aligned` accepts at most one integer argument",
                );
            };
            if !supported_object_alignment(alignment) {
                return self.fail(
                    "CCC2422",
                    span,
                    "requested typedef alignment must be a backend-supported power of two",
                );
            }
            requested = Some(requested.map_or(alignment, |current: u64| current.max(alignment)));
        }
        let Some(alignment) = requested else {
            return Ok(ty);
        };
        if self
            .types
            .builtin_type(ty.ty)
            .is_some_and(BuiltinType::is_integer)
        {
            let adjusted = self.types.alignment_adjusted(ty.ty, alignment);
            let adjusted = QualifiedType::new(adjusted, ty.qualifiers);
            self.reject_weakened_atomic_alignment(adjusted, span)?;
            return Ok(adjusted);
        }
        if !defines_private_record {
            return self.fail(
                "CCC2422",
                span,
                "typedef `aligned` is implemented only for a single inline anonymous record",
            );
        }
        let Some(TypeKind::Record(record_id)) = self.types.try_kind(ty.ty).cloned() else {
            return self.fail(
                "CCC2422",
                span,
                "typedef `aligned` is implemented only for record types",
            );
        };
        let Some(record) = self.types.record(record_id).cloned() else {
            return self.fail(
                "CCC2422",
                span,
                "aligned typedef refers to an unknown record",
            );
        };
        let Some(fields) = record.fields else {
            return self.fail(
                "CCC2422",
                span,
                "aligned typedef requires a complete record type",
            );
        };
        let packing = record
            .packing
            .combine(PackingPolicy::NATIVE.with_minimum_record_alignment(alignment));
        let (aligned_record, aligned_ty) = self.types.declare_record(record.kind, None);
        self.types
            .complete_record_with_packing(aligned_record, fields, packing)
            .expect("a newly declared aligned record is incomplete");
        Ok(QualifiedType::new(aligned_ty, ty.qualifiers))
    }

    fn reject_weakened_atomic_alignment(
        &mut self,
        ty: QualifiedType,
        span: Span,
    ) -> AnalysisResult<()> {
        if !ty.qualifiers.contains(TypeQualifiers::ATOMIC) {
            return Ok(());
        }
        let Some(TypeKind::AlignmentAdjusted(adjusted)) = self.types.try_kind(ty.ty).cloned()
        else {
            return Ok(());
        };
        let natural = self
            .types
            .layout_of(adjusted.underlying, self.config)
            .map_err(|error| self.emit("CCC2453", span, error.to_string()))?
            .align;
        if adjusted.alignment < natural {
            return self.fail(
                "CCC2453",
                span,
                format!(
                    "atomic type `{}` has {}-byte alignment, weaker than its native {natural}-byte alignment",
                    self.types.display_qualified(ty),
                    adjusted.alignment
                ),
            );
        }
        Ok(())
    }

    fn reject_packed_atomic_field(
        &mut self,
        ty: QualifiedType,
        requested_alignment: Option<u64>,
        packing: PackingPolicy,
        span: Span,
    ) -> AnalysisResult<()> {
        if !ty.qualifiers.contains(TypeQualifiers::ATOMIC) {
            return Ok(());
        }
        let natural = self
            .types
            .layout_of(ty.ty, self.config)
            .map_err(|error| self.emit("CCC2453", span, error.to_string()))?
            .align;
        let effective = packing
            .field_alignment(natural)
            .max(requested_alignment.unwrap_or(1));
        if effective < natural {
            return self.fail(
                "CCC2453",
                span,
                format!(
                    "packed atomic field `{}` has {effective}-byte alignment, weaker than its native {natural}-byte alignment",
                    self.types.display_qualified(ty)
                ),
            );
        }
        Ok(())
    }

    fn apply_file_typedef_attributes(
        &mut self,
        ty: QualifiedType,
        attributes: &[FullTypedAttribute],
        defines_private_record: bool,
        span: Span,
    ) -> AnalysisResult<QualifiedType> {
        let ty = self.apply_typedef_alignment(ty, attributes, defines_private_record, span)?;
        if !attributes
            .iter()
            .any(|attribute| attribute_has_name(attribute, "transparent_union"))
        {
            return Ok(ty);
        }
        if !defines_private_record {
            return self.fail(
                "CCC2439",
                span,
                "`transparent_union` is supported only on a single inline anonymous union typedef",
            );
        }
        let Some(TypeKind::Record(record_id)) = self.types.try_kind(ty.ty).cloned() else {
            return self.fail("CCC2439", span, "`transparent_union` requires a union type");
        };
        let Some(record) = self.types.record(record_id).cloned() else {
            return self.fail(
                "CCC2439",
                span,
                "`transparent_union` refers to an unknown union",
            );
        };
        if record.kind != RecordKind::Union {
            return self.fail(
                "CCC2439",
                span,
                "`transparent_union` requires a union rather than a structure",
            );
        }
        let Some(fields) = record.fields else {
            return self.fail(
                "CCC2439",
                span,
                "`transparent_union` requires a complete union",
            );
        };
        if fields.is_empty()
            || fields.iter().any(|field| {
                field.bitfield.is_some() || self.pointer_pointee(field.ty.ty).is_none()
            })
        {
            return self.fail(
                "CCC2439",
                span,
                "the supported `transparent_union` form requires one or more pointer members",
            );
        }
        let first = self
            .types
            .layout_of(fields[0].ty.ty, self.config)
            .map_err(|error| self.emit("CCC2439", span, error.to_string()))?;
        for field in fields.iter().skip(1) {
            let layout = self
                .types
                .layout_of(field.ty.ty, self.config)
                .map_err(|error| self.emit("CCC2439", span, error.to_string()))?;
            if layout.size != first.size || layout.align != first.align {
                return self.fail(
                    "CCC2439",
                    span,
                    "all supported `transparent_union` members must have the first member's representation",
                );
            }
        }
        self.types
            .mark_transparent_union(record_id)
            .map_err(|error| self.emit("CCC2439", span, error.to_string()))?;
        Ok(ty)
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
        let canonical_name = canonical_gnu_attribute_name(&attribute.name.name);
        if canonical_name == "noreturn" && !attribute.arguments.is_empty() {
            return self.fail(
                "CCC2420",
                attribute.span,
                "implemented `noreturn` does not accept arguments",
            );
        }
        if matches!(canonical_name, "always_inline" | "noinline") && !attribute.arguments.is_empty()
        {
            return self.fail(
                "CCC2457",
                attribute.span,
                format!("implemented `{canonical_name}` does not accept arguments"),
            );
        }
        if canonical_name == "weak" && !attribute.arguments.is_empty() {
            return self.fail(
                "CCC2423",
                attribute.span,
                "implemented `weak` does not accept arguments",
            );
        }
        if matches!(
            canonical_name,
            "packed" | "unused" | "may_alias" | "transparent_union"
        ) && !attribute.arguments.is_empty()
        {
            return self.fail(
                "CCC2435",
                attribute.span,
                format!("implemented `{canonical_name}` does not accept arguments"),
            );
        }
        if canonical_name == "alloc_size"
            && !valid_alloc_size_arguments(
                &attribute
                    .arguments
                    .iter()
                    .map(|token| token.spelling.as_str())
                    .collect::<Vec<_>>(),
            )
        {
            return self.fail(
                "CCC2438",
                attribute.span,
                "`alloc_size` requires one or two positive parameter indexes",
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

    fn reject_weak_attribute(
        &mut self,
        attributes: &[FullTypedAttribute],
        span: Span,
        placement: &str,
    ) -> AnalysisResult<()> {
        if attributes
            .iter()
            .any(|attribute| attribute_has_name(attribute, "weak"))
        {
            return self.fail(
                "CCC2423",
                span,
                format!("implemented `weak` cannot be applied to {placement}"),
            );
        }
        Ok(())
    }

    fn reject_function_inlining_attribute(
        &mut self,
        attributes: &[FullTypedAttribute],
        span: Span,
        placement: &str,
    ) -> AnalysisResult<()> {
        if let Some(name) = attributes.iter().find_map(|attribute| {
            let name = canonical_gnu_attribute_name(&attribute.name);
            matches!(name, "always_inline" | "noinline").then_some(name)
        }) {
            return self.fail(
                "CCC2457",
                span,
                format!("implemented `{name}` cannot be applied to {placement}"),
            );
        }
        Ok(())
    }

    fn reject_packed_attribute(
        &mut self,
        attributes: &[FullTypedAttribute],
        span: Span,
    ) -> AnalysisResult<()> {
        if attributes
            .iter()
            .any(|attribute| attribute_has_name(attribute, "packed"))
        {
            return self.fail(
                "CCC2432",
                span,
                "implemented `packed` is supported only on record specifiers",
            );
        }
        Ok(())
    }

    fn reject_transparent_union_attribute(
        &mut self,
        attributes: &[FullTypedAttribute],
        span: Span,
        placement: &str,
    ) -> AnalysisResult<()> {
        if attributes
            .iter()
            .any(|attribute| attribute_has_name(attribute, "transparent_union"))
        {
            return self.fail(
                "CCC2439",
                span,
                format!("`transparent_union` is not supported on {placement}"),
            );
        }
        Ok(())
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
        if bytes.is_empty() || bytes.contains(&0) {
            return self.fail(
                "CCC2349",
                label.span,
                "assembly label cannot be empty or contain a null byte",
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

    fn function_visibility(
        &mut self,
        attributes: &[FullTypedAttribute],
        span: Span,
    ) -> AnalysisResult<Option<SymbolVisibility>> {
        let mut visibility = None;
        for attribute in attributes {
            if attribute.name != "visibility" {
                continue;
            }
            visibility = Some(
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
                },
            );
        }
        Ok(visibility)
    }

    fn apply_emission_attributes(
        &mut self,
        emission: &mut GlobalEmission,
        attributes: &[FullTypedAttribute],
        span: Span,
    ) -> AnalysisResult<()> {
        for attribute in attributes {
            match canonical_gnu_attribute_name(&attribute.name) {
                "aligned" => {
                    let value = if attribute.arguments.is_empty() {
                        self.maximum_supported_alignment()
                    } else {
                        let Some(value) = attribute_argument_number(&attribute.arguments) else {
                            return self.fail(
                                "CCC2351",
                                span,
                                "implemented `aligned` requires an integer argument",
                            );
                        };
                        value
                    };
                    if !supported_object_alignment(value) {
                        return self.fail(
                            "CCC2352",
                            span,
                            "requested alignment must be a backend-supported power of two",
                        );
                    }
                    emission.requested_alignment =
                        strongest_alignment(emission.requested_alignment, Some(value));
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

    fn reject_tls_model_attribute(
        &mut self,
        attributes: &[FullTypedAttribute],
        span: Span,
    ) -> AnalysisResult<()> {
        if attributes
            .iter()
            .any(|attribute| attribute_has_name(attribute, "tls_model"))
        {
            return self.fail(
                "CCC2441",
                span,
                "`tls_model` is only valid on an object with thread storage duration",
            );
        }
        Ok(())
    }

    fn object_requested_alignment(
        &mut self,
        ty: QualifiedType,
        standard_alignment: Option<u64>,
        attributes: &[FullTypedAttribute],
        span: Span,
    ) -> AnalysisResult<Option<u64>> {
        let mut requested = standard_alignment;
        for attribute in attributes {
            if !attribute_has_name(attribute, "aligned") {
                continue;
            }
            let alignment = if attribute.arguments.is_empty() {
                self.maximum_supported_alignment()
            } else {
                let Some(alignment) = attribute_argument_number(&attribute.arguments) else {
                    return self.fail(
                        "CCC2351",
                        span,
                        "implemented `aligned` requires an integer argument",
                    );
                };
                alignment
            };
            if !supported_object_alignment(alignment) {
                return self.fail(
                    "CCC2352",
                    span,
                    "requested alignment must be a backend-supported power of two",
                );
            }
            requested = strongest_alignment(requested, Some(alignment));
        }
        let mut alignment_ty = ty.ty;
        while let Some(TypeKind::Array(array)) = self.types.try_kind(alignment_ty) {
            alignment_ty = array.element.ty;
        }
        let natural = self
            .types
            .layout_of(alignment_ty, self.config)
            .map_err(|error| self.emit("CCC2437", span, error.to_string()))?
            .align;
        if standard_alignment.is_some() && requested.is_some_and(|value| value < natural) {
            return self.fail(
                "CCC2437",
                span,
                format!(
                    "requested alignment {} is weaker than the type's natural alignment {natural}",
                    requested.unwrap_or_default()
                ),
            );
        }
        Ok(requested)
    }

    fn maximum_supported_alignment(&self) -> u64 {
        let layout = &self.config.target.data_layout;
        [
            layout.bool_align,
            layout.char_align,
            layout.short_align,
            layout.int_align,
            layout.long_align,
            layout.long_long_align,
            layout.pointer_align,
            layout.float_align,
            layout.double_align,
            layout.long_double_align,
        ]
        .into_iter()
        .max()
        .map_or(1, u64::from)
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
            | PragmaEvent::Diagnostic { .. }
            | PragmaEvent::GccOptimize { .. } => Ok(()),
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
            S::Expression(_)
            | S::Goto(_)
            | S::ComputedGoto(_)
            | S::Asm(_)
            | S::Continue
            | S::Break
            | S::Return(_) => {}
        }
    }

    fn validate_labels(&mut self) {
        let Some(function) = self.function.as_ref() else {
            return;
        };
        let missing = function.labels.undefined_uses();
        let variably_modified_gotos = function
            .variably_modified_gotos
            .iter()
            .filter_map(|jump| {
                let (definition_span, target_path) =
                    function.variably_modified_label_paths.get(&jump.label)?;
                variably_modified_path_enters(&jump.source_path, target_path)
                    .then_some((jump.span, *definition_span))
            })
            .collect::<Vec<_>>();
        let computed_gotos = if function.has_variably_modified_declaration {
            function.computed_gotos.clone()
        } else {
            Vec::new()
        };
        for (name, span) in missing {
            self.emit("CCC2363", span, format!("use of undefined label `{name}`"));
        }
        for (jump_span, definition_span) in variably_modified_gotos {
            self.diagnostics.push(
                Diagnostic::error(
                    "CCC2442",
                    "a goto cannot enter the scope of an identifier with variably modified type",
                )
                .with_primary(jump_span, "this jump bypasses the declaration")
                .with_secondary(definition_span, "the target is inside that scope"),
            );
        }
        for span in computed_gotos {
            self.emit(
                "CCC2442",
                span,
                "a computed goto is not supported in a function with variably modified declarations",
            );
        }
    }

    fn reject_switch_variably_modified_ingress(&mut self, span: Span) -> AnalysisResult<()> {
        let Some(function) = self.function.as_ref() else {
            return Ok(());
        };
        let Some(switch) = function.switches.last() else {
            return Ok(());
        };
        if variably_modified_path_enters(
            &switch.entry_variably_modified_path,
            &function.active_variably_modified_path,
        ) {
            return self.fail(
                "CCC2442",
                span,
                "a switch label cannot bypass a declaration with variably modified type",
            );
        }
        Ok(())
    }

    fn initializer_references_thread_storage(&self, initializer: &FullTypedInitializer) -> bool {
        match &initializer.kind {
            FullTypedInitializerKind::Scalar(expression) => {
                let Some(ConstantValue::Address(address)) = expression.constant else {
                    return false;
                };
                match address.base {
                    RelocatableBase::Global(id) => self
                        .globals
                        .get(id.0 as usize)
                        .is_some_and(|global| global.duration == StorageDuration::Thread),
                    RelocatableBase::BlockStatic { function, local } => self
                        .function
                        .as_ref()
                        .filter(|state| state.id == function)
                        .and_then(|state| state.static_duration_locals.get(&local))
                        .is_some_and(|duration| *duration == StorageDuration::Thread),
                    RelocatableBase::Function(_)
                    | RelocatableBase::String(_)
                    | RelocatableBase::Label { .. } => false,
                }
            }
            FullTypedInitializerKind::Aggregate(entries) => entries
                .iter()
                .any(|entry| self.initializer_references_thread_storage(&entry.initializer)),
            FullTypedInitializerKind::String(_) | FullTypedInitializerKind::Zero => false,
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
        if let Some(function) = self.function.as_mut() {
            function
                .variably_modified_scope_starts
                .push(function.active_variably_modified_path.len());
        }
        self.scopes.push();
    }

    fn pop_scope(&mut self) {
        if let Some(function) = self.function.as_mut()
            && let Some(start) = function.variably_modified_scope_starts.pop()
        {
            function.active_variably_modified_path.truncate(start);
        }
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

    fn current_tag(&self, name: &str) -> Option<TagSymbol> {
        self.scopes.current_tag(name)
    }

    fn emit(&mut self, code: &'static str, span: Span, message: impl Into<String>) {
        if self.error_limit_reached() {
            return;
        }
        self.diagnostics
            .push(Diagnostic::error(code, message.into()).with_primary_span(span));
    }

    fn error_limit_reached(&self) -> bool {
        self.error_limit
            .is_some_and(|limit| self.diagnostics.len() >= limit)
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

fn constant_to_long_double(
    value: ConstantValue,
    format: LongDoubleFormat,
) -> Option<ConstantValue> {
    let bits = match format {
        LongDoubleFormat::Binary64 => return None,
        LongDoubleFormat::X87Extended => {
            let converted = match value {
                ConstantValue::Signed(value) => X87DoubleExtended::from_i128(value).value,
                ConstantValue::Unsigned(value) => X87DoubleExtended::from_u128(value).value,
                ConstantValue::Floating(value) => {
                    let source = Double::from_bits(u128::from(value.to_bits()));
                    let mut loses_info = false;
                    let converted: X87DoubleExtended = source.convert(&mut loses_info).value;
                    converted
                }
                ConstantValue::LongDouble(value)
                    if value.format == LongDoubleFormat::X87Extended =>
                {
                    return Some(ConstantValue::LongDouble(value));
                }
                ConstantValue::LongDouble(value)
                    if value.format == LongDoubleFormat::IeeeBinary128 =>
                {
                    let source = Quad::from_bits(value.bits());
                    let mut loses_info = false;
                    let converted: X87DoubleExtended = source.convert(&mut loses_info).value;
                    converted
                }
                ConstantValue::LongDouble(_)
                | ConstantValue::NullPointer
                | ConstantValue::Address(_) => return None,
            };
            converted.to_bits()
        }
        LongDoubleFormat::IeeeBinary128 => {
            let converted = match value {
                ConstantValue::Signed(value) => Quad::from_i128(value).value,
                ConstantValue::Unsigned(value) => Quad::from_u128(value).value,
                ConstantValue::Floating(value) => {
                    let source = Double::from_bits(u128::from(value.to_bits()));
                    let mut loses_info = false;
                    let converted: Quad = source.convert(&mut loses_info).value;
                    converted
                }
                ConstantValue::LongDouble(value)
                    if value.format == LongDoubleFormat::IeeeBinary128 =>
                {
                    return Some(ConstantValue::LongDouble(value));
                }
                ConstantValue::LongDouble(value)
                    if value.format == LongDoubleFormat::X87Extended =>
                {
                    let source = X87DoubleExtended::from_bits(value.bits());
                    let mut loses_info = false;
                    let converted: Quad = source.convert(&mut loses_info).value;
                    converted
                }
                ConstantValue::LongDouble(_)
                | ConstantValue::NullPointer
                | ConstantValue::Address(_) => return None,
            };
            converted.to_bits()
        }
    };
    Some(ConstantValue::LongDouble(LongDoubleConstant::from_bits(
        format, bits,
    )))
}

fn long_double_to_integer(value: LongDoubleConstant, width: u8, signed: bool) -> Option<u128> {
    let width = usize::from(width);
    let mut exact = false;
    let (status, raw) = match value.format {
        LongDoubleFormat::Binary64 => return None,
        LongDoubleFormat::X87Extended => {
            let value = X87DoubleExtended::from_bits(value.bits());
            if signed {
                let result = value.to_i128_r(width, Round::TowardZero, &mut exact);
                (result.status, result.value as u128)
            } else {
                let result = value.to_u128_r(width, Round::TowardZero, &mut exact);
                (result.status, result.value)
            }
        }
        LongDoubleFormat::IeeeBinary128 => {
            let value = Quad::from_bits(value.bits());
            if signed {
                let result = value.to_i128_r(width, Round::TowardZero, &mut exact);
                (result.status, result.value as u128)
            } else {
                let result = value.to_u128_r(width, Round::TowardZero, &mut exact);
                (result.status, result.value)
            }
        }
    };
    (!status.contains(Status::INVALID_OP)).then_some(raw)
}

fn long_double_to_f32(value: LongDoubleConstant) -> Option<f64> {
    let mut loses_info = false;
    let converted: Single = match value.format {
        LongDoubleFormat::Binary64 => return None,
        LongDoubleFormat::X87Extended => {
            X87DoubleExtended::from_bits(value.bits())
                .convert(&mut loses_info)
                .value
        }
        LongDoubleFormat::IeeeBinary128 => {
            Quad::from_bits(value.bits()).convert(&mut loses_info).value
        }
    };
    let bits = u32::try_from(converted.to_bits()).ok()?;
    Some(f64::from(f32::from_bits(bits)))
}

fn f64_to_float16(value: f64) -> Half {
    let source = Double::from_bits(u128::from(value.to_bits()));
    let mut loses_info = false;
    source.convert(&mut loses_info).value
}

fn long_double_to_float16(value: LongDoubleConstant) -> Option<Half> {
    let mut loses_info = false;
    Some(match value.format {
        LongDoubleFormat::Binary64 => return None,
        LongDoubleFormat::X87Extended => {
            X87DoubleExtended::from_bits(value.bits())
                .convert(&mut loses_info)
                .value
        }
        LongDoubleFormat::IeeeBinary128 => {
            Quad::from_bits(value.bits()).convert(&mut loses_info).value
        }
    })
}

fn float16_to_f64(value: Half) -> f64 {
    let mut loses_info = false;
    let converted: Double = value.convert(&mut loses_info).value;
    f64::from_bits(converted.to_bits() as u64)
}

fn long_double_to_f64(value: LongDoubleConstant) -> Option<f64> {
    let mut loses_info = false;
    let converted: Double = match value.format {
        LongDoubleFormat::Binary64 => return None,
        LongDoubleFormat::X87Extended => {
            X87DoubleExtended::from_bits(value.bits())
                .convert(&mut loses_info)
                .value
        }
        LongDoubleFormat::IeeeBinary128 => {
            Quad::from_bits(value.bits()).convert(&mut loses_info).value
        }
    };
    let bits = u64::try_from(converted.to_bits()).ok()?;
    Some(f64::from_bits(bits))
}

fn initializer_string_literal(expression: &syntax::Expression) -> Option<&ccc_pp::StringLiteral> {
    match &expression.kind {
        syntax::ExpressionKind::String(literal) => Some(literal),
        syntax::ExpressionKind::Parenthesized(inner) | syntax::ExpressionKind::Extension(inner) => {
            initializer_string_literal(inner)
        }
        _ => None,
    }
}

fn ordinary_asm_text(literal: &ccc_pp::StringLiteral) -> Option<String> {
    if literal.prefix != StringLiteralPrefix::None
        || literal
            .code_units
            .iter()
            .any(|unit| *unit == 0 || *unit > 0x7f)
    {
        return None;
    }
    Some(
        literal
            .code_units
            .iter()
            .map(|unit| char::from_u32(*unit).expect("ASCII code unit is a valid scalar"))
            .collect(),
    )
}

fn defining_function_parameter_list_span(declarator: &syntax::Declarator) -> Option<Span> {
    fn find(direct: &syntax::DirectDeclarator) -> Option<Span> {
        match direct {
            syntax::DirectDeclarator::Identifier(_) | syntax::DirectDeclarator::Abstract(_) => None,
            syntax::DirectDeclarator::Parenthesized(declarator, _) => {
                defining_function_parameter_list_span(declarator)
            }
            syntax::DirectDeclarator::Array { inner, .. } => find(inner),
            syntax::DirectDeclarator::Function { inner, span, .. } => find(inner).or(Some(*span)),
        }
    }

    find(&declarator.direct)
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

fn integer_constant_expression_kind(operands: &[&FullTypedExpression]) -> ConstantExpressionKind {
    if operands.iter().any(|operand| {
        matches!(
            operand.constant_expression_kind,
            ConstantExpressionKind::Invalid | ConstantExpressionKind::FloatingLiteral
        )
    }) {
        ConstantExpressionKind::Invalid
    } else if operands
        .iter()
        .any(|operand| operand.constant_expression_kind == ConstantExpressionKind::UnevaluatedOnly)
    {
        ConstantExpressionKind::UnevaluatedOnly
    } else {
        ConstantExpressionKind::Integer
    }
}

fn unevaluated_operator_constant_expression_kind(
    operands: &[&FullTypedExpression],
) -> ConstantExpressionKind {
    if operands.iter().any(|operand| {
        matches!(
            operand.constant_expression_kind,
            ConstantExpressionKind::Invalid | ConstantExpressionKind::FloatingLiteral
        )
    }) {
        ConstantExpressionKind::Invalid
    } else {
        ConstantExpressionKind::UnevaluatedOnly
    }
}

fn logical_constant_expression_kind(
    operator: syntax::BinaryOperator,
    left: &FullTypedExpression,
    right: &FullTypedExpression,
) -> ConstantExpressionKind {
    use syntax::BinaryOperator as B;

    let combined = integer_constant_expression_kind(&[left, right]);
    if combined == ConstantExpressionKind::Invalid {
        return combined;
    }
    if left.constant_expression_kind != ConstantExpressionKind::Integer {
        return ConstantExpressionKind::UnevaluatedOnly;
    }

    let right_is_unselected = match (operator, left.constant) {
        (B::LogicalAnd, Some(value)) => value.is_zero(),
        (B::LogicalOr, Some(value)) => !value.is_zero(),
        _ => false,
    };
    if right_is_unselected || right.constant_expression_kind == ConstantExpressionKind::Integer {
        ConstantExpressionKind::Integer
    } else {
        ConstantExpressionKind::UnevaluatedOnly
    }
}

fn runtime_sized_array(types: &ccc_types::TypeStore, mut ty: TypeId) -> bool {
    while let Some(TypeKind::Array(array)) = types.try_kind(ty) {
        if matches!(array.length, ArrayLength::Variable(_)) {
            return true;
        }
        ty = array.element.ty;
    }
    false
}

fn innermost_array_element(types: &ccc_types::TypeStore, mut ty: QualifiedType) -> QualifiedType {
    while let Some(TypeKind::Array(array)) = types.try_kind(ty.ty) {
        ty = array.element;
    }
    ty
}

fn runtime_size_bound_ids(
    types: &ccc_types::TypeStore,
    mut ty: TypeId,
) -> HashSet<VariableLengthId> {
    let mut ids = HashSet::new();
    while let Some(TypeKind::Array(array)) = types.try_kind(ty) {
        if let ArrayLength::Variable(id) = array.length {
            ids.insert(id);
        }
        ty = array.element.ty;
    }
    ids
}

fn bounds_for_runtime_type(
    bounds: Vec<FullTypedVariableLengthBound>,
    ty: QualifiedType,
    types: &ccc_types::TypeStore,
) -> Vec<FullTypedVariableLengthBound> {
    let required = runtime_size_bound_ids(types, ty.ty);
    bounds
        .into_iter()
        .filter(|bound| required.contains(&bound.id))
        .collect()
}

fn conditional_constant_expression_kind(
    condition: &FullTypedExpression,
    then_expression: &FullTypedExpression,
    else_expression: &FullTypedExpression,
) -> ConstantExpressionKind {
    let combined = integer_constant_expression_kind(&[condition, then_expression, else_expression]);
    if combined == ConstantExpressionKind::Invalid {
        return combined;
    }
    if condition.constant_expression_kind != ConstantExpressionKind::Integer {
        return ConstantExpressionKind::UnevaluatedOnly;
    }

    let selected_kind = match condition.constant {
        Some(value) if value.is_zero() => else_expression.constant_expression_kind,
        Some(_) => then_expression.constant_expression_kind,
        None if then_expression.constant_expression_kind == ConstantExpressionKind::Integer
            && else_expression.constant_expression_kind == ConstantExpressionKind::Integer =>
        {
            ConstantExpressionKind::Integer
        }
        None => ConstantExpressionKind::UnevaluatedOnly,
    };
    if selected_kind == ConstantExpressionKind::Integer {
        ConstantExpressionKind::Integer
    } else {
        ConstantExpressionKind::UnevaluatedOnly
    }
}

fn builtin_expect_folded_constant(expression: &FullTypedExpression) -> bool {
    use FullTypedExpressionKind as E;
    use syntax::BinaryOperator as B;

    match &expression.kind {
        E::Constant(_) | E::StringLiteral(_) | E::Alignof { .. } | E::Offsetof { .. } => true,
        E::Sizeof { size, .. } => size.is_some(),
        E::DeclRef(SymbolReference::Enumerator { .. }) => true,
        E::DeclRef(_) => false,
        E::GenericSelection { selected, .. } => builtin_expect_folded_constant(selected),
        E::VariableLengthBoundEvaluation { .. } => false,
        E::Conversion {
            kind: ConversionKind::LvalueToValue { .. },
            ..
        } => false,
        E::Conversion {
            kind: ConversionKind::ArrayToPointer | ConversionKind::FunctionToPointer,
            ..
        } => expression.constant.is_some(),
        E::Conversion {
            kind: ConversionKind::ToVoid,
            expression,
        } => builtin_expect_folded_constant(expression),
        E::Conversion {
            expression: operand,
            ..
        } => expression.constant.is_some() && builtin_expect_folded_constant(operand),
        E::Unary { operand, .. } => {
            expression.constant.is_some() && builtin_expect_folded_constant(operand)
        }
        E::Binary {
            operator: operator @ (B::LogicalAnd | B::LogicalOr),
            left,
            right,
        } => {
            if expression.constant.is_none() || !builtin_expect_folded_constant(left) {
                return false;
            }
            match (*operator, left.constant) {
                (B::LogicalAnd, Some(value)) if value.is_zero() => true,
                (B::LogicalOr, Some(value)) if !value.is_zero() => true,
                (B::LogicalAnd | B::LogicalOr, Some(_)) => builtin_expect_folded_constant(right),
                _ => false,
            }
        }
        E::Binary { left, right, .. } => {
            expression.constant.is_some()
                && builtin_expect_folded_constant(left)
                && builtin_expect_folded_constant(right)
        }
        E::AddressOf(_) => expression.constant.is_some(),
        E::Conditional {
            condition,
            then_expression,
            else_expression,
        } => {
            if expression.constant.is_none() || !builtin_expect_folded_constant(condition) {
                return false;
            }
            match condition.constant {
                Some(value) if value.is_zero() => builtin_expect_folded_constant(else_expression),
                Some(_) => builtin_expect_folded_constant(then_expression),
                None => false,
            }
        }
        E::Comma(expressions) => expressions.iter().all(builtin_expect_folded_constant),
        E::StatementExpression { .. } => false,
        E::BuiltinExpect { value, expected: _ } => {
            expression.constant.is_some() && builtin_expect_folded_constant(value)
        }
        E::Dereference(_)
        | E::Subscript { .. }
        | E::Member { .. }
        | E::CompoundLiteral { .. }
        | E::Assignment { .. }
        | E::Increment { .. }
        | E::Call { .. }
        | E::VaStart { .. }
        | E::VaArg { .. }
        | E::VaCopy { .. }
        | E::VaEnd { .. }
        | E::IntegerIntrinsic { .. }
        | E::MemoryCopy { .. }
        | E::MemorySet { .. }
        | E::Prefetch { .. }
        | E::AtomicLoad { .. }
        | E::AtomicStore { .. }
        | E::AtomicReadModifyWrite { .. }
        | E::AtomicCompareExchange { .. }
        | E::MemoryFence { .. } => false,
    }
}

fn direct_function_reference(expression: &FullTypedExpression) -> Option<FullFunctionId> {
    match &expression.kind {
        FullTypedExpressionKind::DeclRef(SymbolReference::Function(id)) => Some(*id),
        FullTypedExpressionKind::GenericSelection { selected, .. } => {
            direct_function_reference(selected)
        }
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

fn static_compound_literal_initializer(
    expression: &FullTypedExpression,
) -> Option<&FullTypedInitializer> {
    match &expression.kind {
        FullTypedExpressionKind::CompoundLiteral {
            storage: CompoundLiteralStorage::Static(_),
            initializer,
        } => Some(initializer),
        FullTypedExpressionKind::GenericSelection { selected, .. } => {
            static_compound_literal_initializer(selected)
        }
        _ => None,
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
            | BuiltinType::Int128
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
        BuiltinType::Int128 | BuiltinType::UnsignedInt128 => 6,
        BuiltinType::Void
        | BuiltinType::Float16
        | BuiltinType::Float
        | BuiltinType::Double
        | BuiltinType::LongDouble => 0,
    }
}

fn unsigned_counterpart(kind: BuiltinType) -> BuiltinType {
    match kind {
        BuiltinType::Char | BuiltinType::SignedChar => BuiltinType::UnsignedChar,
        BuiltinType::Short => BuiltinType::UnsignedShort,
        BuiltinType::Int => BuiltinType::UnsignedInt,
        BuiltinType::Long => BuiltinType::UnsignedLong,
        BuiltinType::LongLong => BuiltinType::UnsignedLongLong,
        BuiltinType::Int128 => BuiltinType::UnsignedInt128,
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

#[derive(Clone, Copy)]
struct IntegerConstantType {
    width: u8,
    signed: bool,
}

fn signed_integer_constant(
    value: i128,
    integer_type: Option<IntegerConstantType>,
) -> Option<ConstantValue> {
    let integer_type = integer_type?;
    if !integer_type.signed
        || value < signed_min(integer_type.width)
        || value > signed_max(integer_type.width) as i128
    {
        return None;
    }
    Some(ConstantValue::Signed(value))
}

fn unsigned_integer_constant(
    value: u128,
    integer_type: Option<IntegerConstantType>,
) -> Option<ConstantValue> {
    let integer_type = integer_type?;
    if integer_type.signed {
        return None;
    }
    Some(ConstantValue::Unsigned(truncate_to_width(
        value,
        integer_type.width,
    )))
}

fn evaluate_unary_constant(
    operator: syntax::UnaryOperator,
    operand: Option<ConstantValue>,
    integer_type: Option<IntegerConstantType>,
) -> Option<ConstantValue> {
    use syntax::UnaryOperator as U;
    match (operator, operand?) {
        (U::Plus, value @ ConstantValue::Floating(_)) => Some(value),
        (U::Plus, ConstantValue::Signed(value)) => signed_integer_constant(value, integer_type),
        (U::Plus, ConstantValue::Unsigned(value)) => unsigned_integer_constant(value, integer_type),
        (U::Minus, ConstantValue::Signed(value)) => value
            .checked_neg()
            .and_then(|value| signed_integer_constant(value, integer_type)),
        (U::Minus, ConstantValue::Unsigned(value)) => {
            unsigned_integer_constant(value.wrapping_neg(), integer_type)
        }
        (U::Minus, ConstantValue::Floating(value)) => Some(ConstantValue::Floating(-value)),
        (U::Plus, value @ ConstantValue::LongDouble(_)) => Some(value),
        (U::Minus, ConstantValue::LongDouble(value)) => {
            negate_long_double(value).map(ConstantValue::LongDouble)
        }
        (U::BitwiseNot, ConstantValue::Signed(value)) => {
            signed_integer_constant(!value, integer_type)
        }
        (U::BitwiseNot, ConstantValue::Unsigned(value)) => {
            unsigned_integer_constant(!value, integer_type)
        }
        _ => None,
    }
}

fn evaluate_binary_constant(
    operator: syntax::BinaryOperator,
    left: Option<ConstantValue>,
    right: Option<ConstantValue>,
    integer_type: Option<IntegerConstantType>,
    float_precision: bool,
) -> Option<ConstantValue> {
    use syntax::BinaryOperator as B;
    let left = left?;
    match (operator, left) {
        (B::LogicalAnd, value) if value.is_zero() => {
            return Some(ConstantValue::Signed(0));
        }
        (B::LogicalOr, value) if !value.is_zero() => {
            return Some(ConstantValue::Signed(1));
        }
        _ => {}
    }
    let right = right?;
    let boolean = |value: bool| Some(ConstantValue::Signed(i128::from(value)));
    if matches!(operator, B::LeftShift | B::RightShift) {
        let integer_type = integer_type?;
        let count = match right {
            ConstantValue::Signed(value) => u32::try_from(value).ok()?,
            ConstantValue::Unsigned(value) => u32::try_from(value).ok()?,
            ConstantValue::Floating(_)
            | ConstantValue::LongDouble(_)
            | ConstantValue::NullPointer
            | ConstantValue::Address(_) => return None,
        };
        if count >= u32::from(integer_type.width) {
            return None;
        }
        return match (operator, left) {
            (B::LeftShift, ConstantValue::Signed(value)) if value >= 0 => {
                let maximum = (signed_max(integer_type.width) as i128).checked_shr(count)?;
                if value > maximum {
                    return None;
                }
                value
                    .checked_shl(count)
                    .and_then(|value| signed_integer_constant(value, Some(integer_type)))
            }
            (B::RightShift, ConstantValue::Signed(value)) => value
                .checked_shr(count)
                .and_then(|value| signed_integer_constant(value, Some(integer_type))),
            (B::LeftShift, ConstantValue::Unsigned(value)) => {
                unsigned_integer_constant(value.wrapping_shl(count), Some(integer_type))
            }
            (B::RightShift, ConstantValue::Unsigned(value)) => {
                unsigned_integer_constant(value.checked_shr(count)?, Some(integer_type))
            }
            _ => None,
        };
    }
    match (left, right) {
        (ConstantValue::Signed(left), ConstantValue::Signed(right)) => match operator {
            B::Multiply => left
                .checked_mul(right)
                .and_then(|value| signed_integer_constant(value, integer_type)),
            B::Divide | B::Remainder => {
                let integer_type = integer_type?;
                if !integer_type.signed
                    || right == 0
                    || (left == signed_min(integer_type.width) && right == -1)
                {
                    return None;
                }
                let value = if operator == B::Divide {
                    left.checked_div(right)?
                } else {
                    left.checked_rem(right)?
                };
                signed_integer_constant(value, Some(integer_type))
            }
            B::Add => left
                .checked_add(right)
                .and_then(|value| signed_integer_constant(value, integer_type)),
            B::Subtract => left
                .checked_sub(right)
                .and_then(|value| signed_integer_constant(value, integer_type)),
            B::LeftShift | B::RightShift => unreachable!("shift constants are handled above"),
            B::Less => boolean(left < right),
            B::LessEqual => boolean(left <= right),
            B::Greater => boolean(left > right),
            B::GreaterEqual => boolean(left >= right),
            B::Equal => boolean(left == right),
            B::NotEqual => boolean(left != right),
            B::BitwiseAnd => signed_integer_constant(left & right, integer_type),
            B::BitwiseXor => signed_integer_constant(left ^ right, integer_type),
            B::BitwiseOr => signed_integer_constant(left | right, integer_type),
            B::LogicalAnd => boolean(left != 0 && right != 0),
            B::LogicalOr => boolean(left != 0 || right != 0),
        },
        (ConstantValue::Unsigned(left), ConstantValue::Unsigned(right)) => match operator {
            B::Multiply => unsigned_integer_constant(left.wrapping_mul(right), integer_type),
            B::Divide => {
                (right != 0).then(|| unsigned_integer_constant(left / right, integer_type))?
            }
            B::Remainder => {
                (right != 0).then(|| unsigned_integer_constant(left % right, integer_type))?
            }
            B::Add => unsigned_integer_constant(left.wrapping_add(right), integer_type),
            B::Subtract => unsigned_integer_constant(left.wrapping_sub(right), integer_type),
            B::LeftShift | B::RightShift => unreachable!("shift constants are handled above"),
            B::Less => boolean(left < right),
            B::LessEqual => boolean(left <= right),
            B::Greater => boolean(left > right),
            B::GreaterEqual => boolean(left >= right),
            B::Equal => boolean(left == right),
            B::NotEqual => boolean(left != right),
            B::BitwiseAnd => unsigned_integer_constant(left & right, integer_type),
            B::BitwiseXor => unsigned_integer_constant(left ^ right, integer_type),
            B::BitwiseOr => unsigned_integer_constant(left | right, integer_type),
            B::LogicalAnd => boolean(left != 0 && right != 0),
            B::LogicalOr => boolean(left != 0 || right != 0),
        },
        (ConstantValue::Floating(left), ConstantValue::Floating(right)) => match operator {
            B::Multiply | B::Divide | B::Add | B::Subtract => {
                Some(fold_floating_binary(operator, left, right, float_precision))
            }
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
        (ConstantValue::LongDouble(left), ConstantValue::LongDouble(right)) => {
            evaluate_long_double_binary(operator, left, right)
        }
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

fn fold_floating_binary(
    operator: syntax::BinaryOperator,
    left: f64,
    right: f64,
    float_precision: bool,
) -> ConstantValue {
    use syntax::BinaryOperator as B;
    if float_precision {
        let (left, right) = (left as f32, right as f32);
        ConstantValue::Floating(f64::from(match operator {
            B::Multiply => left * right,
            B::Divide => left / right,
            B::Add => left + right,
            B::Subtract => left - right,
            _ => unreachable!("only arithmetic operations are folded here"),
        }))
    } else {
        ConstantValue::Floating(match operator {
            B::Multiply => left * right,
            B::Divide => left / right,
            B::Add => left + right,
            B::Subtract => left - right,
            _ => unreachable!("only arithmetic operations are folded here"),
        })
    }
}

fn negate_long_double(value: LongDoubleConstant) -> Option<LongDoubleConstant> {
    let bits = match value.format {
        LongDoubleFormat::Binary64 => return None,
        LongDoubleFormat::X87Extended => {
            let value = X87DoubleExtended::from_bits(value.bits());
            if value.is_nan() {
                return None;
            }
            (-value).to_bits()
        }
        LongDoubleFormat::IeeeBinary128 => {
            let value = Quad::from_bits(value.bits());
            if value.is_nan() {
                return None;
            }
            (-value).to_bits()
        }
    };
    Some(LongDoubleConstant::from_bits(value.format, bits))
}

fn evaluate_long_double_binary(
    operator: syntax::BinaryOperator,
    left: LongDoubleConstant,
    right: LongDoubleConstant,
) -> Option<ConstantValue> {
    if left.format != right.format {
        return None;
    }
    match left.format {
        LongDoubleFormat::Binary64 => None,
        LongDoubleFormat::X87Extended => evaluate_apfloat_binary(
            operator,
            X87DoubleExtended::from_bits(left.bits()),
            X87DoubleExtended::from_bits(right.bits()),
            left.format,
        ),
        LongDoubleFormat::IeeeBinary128 => evaluate_apfloat_binary(
            operator,
            Quad::from_bits(left.bits()),
            Quad::from_bits(right.bits()),
            left.format,
        ),
    }
}

fn evaluate_apfloat_binary<F: Float>(
    operator: syntax::BinaryOperator,
    left: F,
    right: F,
    format: LongDoubleFormat,
) -> Option<ConstantValue> {
    use syntax::BinaryOperator as B;
    if left.is_nan() || right.is_nan() {
        // Runtime comparison and arithmetic preserve the target's exception
        // behavior for signaling NaNs; the default frontend contract does not
        // need to speculate about it while folding.
        return None;
    }
    let boolean = |value: bool| Some(ConstantValue::Signed(i128::from(value)));
    match operator {
        B::Multiply | B::Divide | B::Add | B::Subtract => {
            let result = match operator {
                B::Multiply => left.mul_r(right, Round::NearestTiesToEven),
                B::Divide => left.div_r(right, Round::NearestTiesToEven),
                B::Add => left.add_r(right, Round::NearestTiesToEven),
                B::Subtract => left.sub_r(right, Round::NearestTiesToEven),
                _ => unreachable!(),
            };
            if result.value.is_nan() {
                return None;
            }
            Some(ConstantValue::LongDouble(LongDoubleConstant::from_bits(
                format,
                result.value.to_bits(),
            )))
        }
        B::Less => boolean(left < right),
        B::LessEqual => boolean(left <= right),
        B::Greater => boolean(left > right),
        B::GreaterEqual => boolean(left >= right),
        B::Equal => boolean(left == right),
        B::NotEqual => boolean(left != right),
        B::LogicalAnd => boolean(!left.is_zero() && !right.is_zero()),
        B::LogicalOr => boolean(!left.is_zero() || !right.is_zero()),
        B::Remainder
        | B::LeftShift
        | B::RightShift
        | B::BitwiseAnd
        | B::BitwiseXor
        | B::BitwiseOr => None,
    }
}

fn variably_modified_path_enters(
    source: &[VariablyModifiedScopeEntry],
    target: &[VariablyModifiedScopeEntry],
) -> bool {
    target.iter().any(|entry| !source.contains(entry))
}

fn has_direct_label_address_provenance(expression: &FullTypedExpression) -> bool {
    if matches!(
        expression.constant,
        Some(ConstantValue::Address(RelocatableAddress {
            base: RelocatableBase::Label { .. },
            ..
        }))
    ) {
        return true;
    }
    match &expression.kind {
        FullTypedExpressionKind::GenericSelection { selected, .. } => {
            has_direct_label_address_provenance(selected)
        }
        FullTypedExpressionKind::Conversion { expression, .. } => {
            has_direct_label_address_provenance(expression)
        }
        FullTypedExpressionKind::Conditional {
            condition,
            then_expression,
            else_expression,
        } => match condition.constant {
            Some(value) if value.is_zero() => has_direct_label_address_provenance(else_expression),
            Some(_) => has_direct_label_address_provenance(then_expression),
            None => {
                has_direct_label_address_provenance(then_expression)
                    || has_direct_label_address_provenance(else_expression)
            }
        },
        FullTypedExpressionKind::Comma(expressions) => expressions
            .last()
            .is_some_and(has_direct_label_address_provenance),
        FullTypedExpressionKind::BuiltinExpect { value, .. } => {
            has_direct_label_address_provenance(value)
        }
        _ => false,
    }
}

fn loaded_bitfield(expression: &FullTypedExpression) -> Option<&BitfieldPlace> {
    expression
        .place
        .as_ref()
        .and_then(|place| place.bitfield.as_ref())
        .or_else(|| {
            let FullTypedExpressionKind::Conversion {
                kind: ConversionKind::LvalueToValue { .. },
                expression,
            } = &expression.kind
            else {
                return None;
            };
            expression
                .place
                .as_ref()
                .and_then(|place| place.bitfield.as_ref())
        })
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

fn strongest_alignment(left: Option<u64>, right: Option<u64>) -> Option<u64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.max(right)),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

fn common_power_of_two_alignment(base_alignment: u64, byte_offset: u64) -> u64 {
    if byte_offset == 0 {
        return base_alignment;
    }
    base_alignment.min(1_u64 << byte_offset.trailing_zeros())
}

fn supported_object_alignment(alignment: u64) -> bool {
    alignment.is_power_of_two() && alignment.trailing_zeros() < 32
}

fn valid_alloc_size_arguments(arguments: &[&str]) -> bool {
    let positive = |value: &str| value.parse::<u64>().is_ok_and(|value| value != 0);
    match arguments {
        [first] => positive(first),
        [first, comma, second] => *comma == "," && positive(first) && positive(second),
        _ => false,
    }
}

fn attribute_argument_string(arguments: &[String]) -> Option<String> {
    arguments.iter().find_map(|argument| {
        argument
            .strip_prefix('"')
            .and_then(|argument| argument.strip_suffix('"'))
            .map(str::to_owned)
    })
}

fn canonical_gnu_attribute_name(name: &str) -> &str {
    name.strip_prefix("__")
        .and_then(|name| name.strip_suffix("__"))
        .unwrap_or(name)
}

fn defines_inline_anonymous_record(specifiers: &syntax::DeclarationSpecifiers) -> bool {
    specifiers.items.iter().any(|specifier| {
        matches!(
            specifier,
            syntax::DeclarationSpecifier::Type(
                syntax::TypeSpecifier::Struct(record) | syntax::TypeSpecifier::Union(record)
            ) if record.tag.is_none() && record.items.is_some()
        )
    })
}

fn attribute_has_name(attribute: &FullTypedAttribute, name: &str) -> bool {
    canonical_gnu_attribute_name(&attribute.name) == name
}

fn is_known_returns_twice_function(name: &str) -> bool {
    matches!(name, "setjmp" | "_setjmp" | "sigsetjmp" | "__sigsetjmp")
}

fn attribute_argument_identifier(arguments: &[String]) -> Option<&str> {
    let [argument] = arguments else {
        return None;
    };
    Some(canonical_gnu_attribute_name(argument))
}
