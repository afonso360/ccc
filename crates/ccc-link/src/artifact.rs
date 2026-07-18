//! Verified artifact bundles exchanged between code generation and packaging.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use object::read::{Object as _, ObjectSection as _, ObjectSymbol as _};
use object::{Architecture, BinaryFormat, ObjectKind, SymbolScope};

use crate::bridge::{
    GeneratedAssembly, GeneratedSymbolKind, is_bridge_generated_symbol, validate_symbol,
};
use crate::{LinkError, artifact_error};

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum GeneratedSymbolOwner {
    PrimaryObject,
    AssemblyUnit(String),
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum GeneratedSymbolVisibility {
    /// A source-level externally visible definition, such as a variadic entry.
    Public,
    /// A source-level externally linked definition with hidden ELF visibility.
    SourceHidden,
    /// A source-level externally linked definition with protected visibility.
    SourceProtected,
    /// A source-level externally linked definition with ELF internal visibility.
    SourceElfInternal,
    /// A source-level definition with internal linkage.
    SourceInternal,
    /// A compiler implementation detail localized after the partial link.
    Internal,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum GeneratedSymbolBinding {
    Strong,
    Weak,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GeneratedSymbol {
    pub name: String,
    /// Whether `name` is already the physical object-file spelling and must
    /// bypass the target's ordinary C symbol mangling.
    pub object_name_is_exact: bool,
    pub kind: GeneratedSymbolKind,
    pub owner: GeneratedSymbolOwner,
    pub visibility: GeneratedSymbolVisibility,
    pub binding: GeneratedSymbolBinding,
}

impl GeneratedSymbol {
    pub fn internal(
        name: impl Into<String>,
        kind: GeneratedSymbolKind,
        owner: GeneratedSymbolOwner,
    ) -> Self {
        Self {
            name: name.into(),
            object_name_is_exact: false,
            kind,
            owner,
            visibility: GeneratedSymbolVisibility::Internal,
            binding: GeneratedSymbolBinding::Strong,
        }
    }

    pub fn public(
        name: impl Into<String>,
        kind: GeneratedSymbolKind,
        owner: GeneratedSymbolOwner,
    ) -> Self {
        Self {
            name: name.into(),
            object_name_is_exact: false,
            kind,
            owner,
            visibility: GeneratedSymbolVisibility::Public,
            binding: GeneratedSymbolBinding::Strong,
        }
    }

    pub fn source_hidden(
        name: impl Into<String>,
        kind: GeneratedSymbolKind,
        owner: GeneratedSymbolOwner,
    ) -> Self {
        Self {
            name: name.into(),
            object_name_is_exact: false,
            kind,
            owner,
            visibility: GeneratedSymbolVisibility::SourceHidden,
            binding: GeneratedSymbolBinding::Strong,
        }
    }

    pub fn source_internal(
        name: impl Into<String>,
        kind: GeneratedSymbolKind,
        owner: GeneratedSymbolOwner,
    ) -> Self {
        Self {
            name: name.into(),
            object_name_is_exact: false,
            kind,
            owner,
            visibility: GeneratedSymbolVisibility::SourceInternal,
            binding: GeneratedSymbolBinding::Strong,
        }
    }

    pub fn source_elf_internal(
        name: impl Into<String>,
        kind: GeneratedSymbolKind,
        owner: GeneratedSymbolOwner,
    ) -> Self {
        Self {
            name: name.into(),
            object_name_is_exact: false,
            kind,
            owner,
            visibility: GeneratedSymbolVisibility::SourceElfInternal,
            binding: GeneratedSymbolBinding::Strong,
        }
    }

    pub fn source_protected(
        name: impl Into<String>,
        kind: GeneratedSymbolKind,
        owner: GeneratedSymbolOwner,
    ) -> Self {
        Self {
            name: name.into(),
            object_name_is_exact: false,
            kind,
            owner,
            visibility: GeneratedSymbolVisibility::SourceProtected,
            binding: GeneratedSymbolBinding::Strong,
        }
    }

    pub fn with_weak_binding(mut self) -> Self {
        self.binding = GeneratedSymbolBinding::Weak;
        self
    }

    pub fn with_exact_object_name(mut self) -> Self {
        self.object_name_is_exact = true;
        self
    }

    pub(crate) fn object_name(&self, format: BinaryFormat) -> Cow<'_, str> {
        if format == BinaryFormat::MachO && !self.object_name_is_exact {
            Cow::Owned(format!("_{}", self.name))
        } else {
            Cow::Borrowed(&self.name)
        }
    }
}

/// Versioned ownership and visibility contract for generated symbols.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BridgeManifestV2 {
    translation_unit_digest: [u8; 32],
    symbols: Vec<GeneratedSymbol>,
}

impl BridgeManifestV2 {
    pub const VERSION: u16 = 2;

    pub fn empty(translation_unit_digest: [u8; 32]) -> Self {
        Self {
            translation_unit_digest,
            symbols: Vec::new(),
        }
    }

    pub fn new(translation_unit_digest: [u8; 32], symbols: Vec<GeneratedSymbol>) -> Self {
        Self {
            translation_unit_digest,
            symbols,
        }
    }

    pub fn translation_unit_digest(&self) -> &[u8; 32] {
        &self.translation_unit_digest
    }

    pub fn symbols(&self) -> &[GeneratedSymbol] {
        &self.symbols
    }

    pub fn localization_symbols(&self) -> Vec<&str> {
        let mut names = self
            .symbols
            .iter()
            .filter(|symbol| {
                matches!(
                    symbol.visibility,
                    GeneratedSymbolVisibility::Internal | GeneratedSymbolVisibility::SourceInternal
                )
            })
            .map(|symbol| symbol.name.as_str())
            .collect::<Vec<_>>();
        names.sort_unstable();
        names
    }

    pub(crate) fn localization_object_symbols(&self, format: BinaryFormat) -> Vec<Cow<'_, str>> {
        let mut names = self
            .symbols
            .iter()
            .filter(|symbol| {
                matches!(
                    symbol.visibility,
                    GeneratedSymbolVisibility::Internal | GeneratedSymbolVisibility::SourceInternal
                )
            })
            .map(|symbol| symbol.object_name(format))
            .collect::<Vec<_>>();
        names.sort_unstable();
        names
    }

    fn verify_shape(&self, assemblies: &[GeneratedAssembly]) -> Result<(), LinkError> {
        let mut names = BTreeSet::new();
        let mut owned_by_assembly = BTreeMap::<&str, BTreeSet<&str>>::new();
        for symbol in &self.symbols {
            validate_symbol(&symbol.name)?;
            if !names.insert(symbol.name.as_str()) {
                return Err(artifact_error(format!(
                    "generated symbol `{}` appears more than once in the bridge manifest",
                    symbol.name
                )));
            }
            if symbol.visibility == GeneratedSymbolVisibility::Internal
                && !is_bridge_generated_symbol(&symbol.name)
            {
                return Err(artifact_error(format!(
                    "internal generated symbol `{}` is outside the reserved namespace",
                    symbol.name
                )));
            }
            if let GeneratedSymbolOwner::AssemblyUnit(stem) = &symbol.owner {
                owned_by_assembly
                    .entry(stem)
                    .or_default()
                    .insert(symbol.name.as_str());
            }
        }

        let mut stems = BTreeSet::new();
        for assembly in assemblies {
            if !stems.insert(assembly.stem()) {
                return Err(artifact_error(format!(
                    "generated assembly stem `{}` appears more than once",
                    assembly.stem()
                )));
            }
            let actual = assembly
                .defined_symbols()
                .iter()
                .map(String::as_str)
                .collect::<BTreeSet<_>>();
            let expected = owned_by_assembly
                .remove(assembly.stem())
                .unwrap_or_default();
            if actual != expected {
                return Err(artifact_error(format!(
                    "generated assembly `{}` definitions do not match its bridge manifest ownership",
                    assembly.stem()
                )));
            }
        }
        if let Some((stem, _)) = owned_by_assembly.into_iter().next() {
            return Err(artifact_error(format!(
                "bridge manifest assigns symbols to missing assembly `{stem}`"
            )));
        }
        if assemblies.is_empty() && !self.symbols.is_empty() {
            return Err(artifact_error(
                "a bridge manifest with generated symbols has no generated assembly",
            ));
        }
        Ok(())
    }

    fn verify_primary_object(&self, primary_object: &[u8]) -> Result<(), LinkError> {
        let object = parse_relocatable(primary_object, "primary object")?;
        let format = object.format();
        let mut physical_manifest_names = BTreeMap::<String, &str>::new();
        for symbol in &self.symbols {
            let object_name = symbol.object_name(format).into_owned();
            if let Some(previous) =
                physical_manifest_names.insert(object_name.clone(), &symbol.name)
            {
                return Err(artifact_error(format!(
                    "generated symbols `{previous}` and `{}` map to the same physical object symbol `{object_name}`",
                    symbol.name
                )));
            }
        }
        let mut defined = BTreeMap::<String, SymbolScope>::new();
        let mut undefined = BTreeSet::<String>::new();
        for symbol in object.symbols() {
            let Ok(name) = symbol.name() else {
                continue;
            };
            if name.is_empty() {
                continue;
            }
            if symbol.is_undefined() {
                undefined.insert(name.to_owned());
            } else {
                defined.insert(name.to_owned(), symbol.scope());
            }
        }

        for symbol in &self.symbols {
            let object_name = symbol.object_name(format);
            match &symbol.owner {
                GeneratedSymbolOwner::PrimaryObject
                    if !defined.contains_key(object_name.as_ref()) =>
                {
                    return Err(artifact_error(format!(
                        "primary object does not define manifest symbol `{}`",
                        symbol.name
                    )));
                }
                GeneratedSymbolOwner::AssemblyUnit(_)
                    if defined.contains_key(object_name.as_ref()) =>
                {
                    return Err(artifact_error(format!(
                        "primary object collides with assembly-owned symbol `{}`",
                        symbol.name
                    )));
                }
                GeneratedSymbolOwner::AssemblyUnit(_)
                    if matches!(
                        symbol.kind,
                        GeneratedSymbolKind::CallHelper
                            | GeneratedSymbolKind::CallStub
                            | GeneratedSymbolKind::TlsAccessor
                    ) && !undefined.contains(object_name.as_ref()) =>
                {
                    return Err(artifact_error(format!(
                        "primary object does not reference required bridge symbol `{}`",
                        symbol.name
                    )));
                }
                _ => {}
            }
        }
        Ok(())
    }
}

pub(crate) fn canonical_symbol_name(format: BinaryFormat, name: &str) -> &str {
    if format == BinaryFormat::MachO {
        name.strip_prefix('_').unwrap_or(name)
    } else {
        name
    }
}

/// Code generation output plus any deterministic assembly needed to complete
/// the relocatable object.
#[derive(Clone, Debug)]
pub struct ArtifactBundle {
    primary_object: Vec<u8>,
    assemblies: Vec<GeneratedAssembly>,
    manifest: BridgeManifestV2,
}

impl ArtifactBundle {
    pub fn bridge_free(primary_object: Vec<u8>, translation_unit_digest: [u8; 32]) -> Self {
        Self {
            primary_object,
            assemblies: Vec::new(),
            manifest: BridgeManifestV2::empty(translation_unit_digest),
        }
    }

    pub fn new(
        primary_object: Vec<u8>,
        assemblies: Vec<GeneratedAssembly>,
        manifest: BridgeManifestV2,
    ) -> Self {
        Self {
            primary_object,
            assemblies,
            manifest,
        }
    }

    pub fn primary_object(&self) -> &[u8] {
        &self.primary_object
    }

    pub fn assemblies(&self) -> &[GeneratedAssembly] {
        &self.assemblies
    }

    pub fn manifest(&self) -> &BridgeManifestV2 {
        &self.manifest
    }

    pub fn needs_packaging_tools(&self) -> bool {
        !self.assemblies.is_empty()
    }

    pub fn verify(self) -> Result<VerifiedArtifactBundle, LinkError> {
        self.manifest.verify_shape(&self.assemblies)?;
        self.manifest.verify_primary_object(&self.primary_object)?;
        Ok(VerifiedArtifactBundle(self))
    }
}

/// An artifact whose manifest and primary relocatable object agree.
#[derive(Clone, Debug)]
pub struct VerifiedArtifactBundle(ArtifactBundle);

impl VerifiedArtifactBundle {
    pub fn primary_object(&self) -> &[u8] {
        self.0.primary_object()
    }

    pub fn assemblies(&self) -> &[GeneratedAssembly] {
        self.0.assemblies()
    }

    pub fn manifest(&self) -> &BridgeManifestV2 {
        self.0.manifest()
    }

    pub fn needs_packaging_tools(&self) -> bool {
        self.0.needs_packaging_tools()
    }
}

pub(crate) fn parse_relocatable<'data>(
    bytes: &'data [u8],
    description: &str,
) -> Result<object::File<'data>, LinkError> {
    let object = object::File::parse(bytes).map_err(|error| {
        artifact_error(format!("cannot parse {description} as an object: {error}"))
    })?;
    let enabled_format = matches!(
        (object.format(), object.architecture()),
        (
            BinaryFormat::Elf,
            Architecture::X86_64 | Architecture::Aarch64 | Architecture::Riscv64
        ) | (BinaryFormat::MachO, Architecture::Aarch64)
    );
    if !enabled_format || object.kind() != ObjectKind::Relocatable || object.entry() != 0 {
        return Err(artifact_error(format!(
            "{description} is not an enabled relocatable object"
        )));
    }
    for section in object.sections() {
        if matches!(section.name(), Ok(".dynamic" | ".dynsym" | ".interp")) {
            return Err(artifact_error(format!(
                "{description} unexpectedly contains dynamic-link section `{}`",
                section.name().unwrap_or("<invalid>")
            )));
        }
    }
    Ok(object)
}

#[cfg(test)]
mod tests {
    use object::write::{Object, StandardSection, Symbol, SymbolSection};
    use object::{Endianness, SymbolFlags, SymbolKind};

    use super::*;

    fn object_with_symbols(symbols: &[(&str, bool)]) -> Vec<u8> {
        let mut object = Object::new(BinaryFormat::Elf, Architecture::X86_64, Endianness::Little);
        let text = object.section_id(StandardSection::Text);
        object.append_section_data(text, &[0xc3], 1);
        for (name, defined) in symbols {
            object.add_symbol(Symbol {
                name: name.as_bytes().to_vec(),
                value: 0,
                size: 0,
                kind: SymbolKind::Text,
                scope: SymbolScope::Linkage,
                weak: false,
                section: if *defined {
                    SymbolSection::Section(text)
                } else {
                    SymbolSection::Undefined
                },
                flags: SymbolFlags::None,
            });
        }
        object.write().unwrap()
    }

    fn empty_macho_object() -> Vec<u8> {
        let mut object = Object::new(
            BinaryFormat::MachO,
            Architecture::Aarch64,
            Endianness::Little,
        );
        let text = object.section_id(StandardSection::Text);
        object.append_section_data(text, &[0xc0, 0x03, 0x5f, 0xd6], 4);
        object.write().unwrap()
    }

    #[test]
    fn bridge_free_bundle_verifies_without_a_manifest_surface() {
        let bundle = ArtifactBundle::bridge_free(object_with_symbols(&[("main", true)]), [7; 32]);
        let verified = bundle.verify().unwrap();
        assert!(!verified.needs_packaging_tools());
        assert!(verified.manifest().localization_symbols().is_empty());
    }

    #[test]
    fn manifest_checks_ownership_references_and_exact_localization() {
        let helper = "__ccc_call_helper_0123456789abcdef";
        let assembly =
            GeneratedAssembly::new("helper", ".text\n", vec![helper.to_owned()], Vec::new())
                .unwrap();
        let manifest = BridgeManifestV2::new(
            [9; 32],
            vec![GeneratedSymbol::internal(
                helper,
                GeneratedSymbolKind::CallHelper,
                GeneratedSymbolOwner::AssemblyUnit("helper".to_owned()),
            )],
        );
        let bundle = ArtifactBundle::new(
            object_with_symbols(&[(helper, false)]),
            vec![assembly],
            manifest,
        )
        .verify()
        .unwrap();
        assert_eq!(bundle.manifest().localization_symbols(), [helper]);
    }

    #[test]
    fn elf_internal_visibility_does_not_request_local_binding() {
        let entry = "variadic_internal";
        let assembly =
            GeneratedAssembly::new("entry", ".text\n", vec![entry.to_owned()], Vec::new()).unwrap();
        let manifest = BridgeManifestV2::new(
            [8; 32],
            vec![GeneratedSymbol::source_elf_internal(
                entry,
                GeneratedSymbolKind::VariadicEntry,
                GeneratedSymbolOwner::AssemblyUnit("entry".to_owned()),
            )],
        );
        let bundle = ArtifactBundle::new(
            object_with_symbols(&[(entry, false)]),
            vec![assembly],
            manifest,
        )
        .verify()
        .unwrap();
        assert!(bundle.manifest().localization_symbols().is_empty());
    }

    #[test]
    fn exact_source_internal_macho_name_is_localized_without_c_mangling() {
        let manifest = BridgeManifestV2::new(
            [4; 32],
            vec![
                GeneratedSymbol::source_internal(
                    "physical_local",
                    GeneratedSymbolKind::VariadicEntry,
                    GeneratedSymbolOwner::AssemblyUnit("entry".to_owned()),
                )
                .with_exact_object_name(),
            ],
        );
        assert_eq!(
            manifest.localization_object_symbols(BinaryFormat::MachO),
            [Cow::Borrowed("physical_local")]
        );
    }

    #[test]
    fn manifest_rejects_an_assembly_collision_in_the_primary_object() {
        let helper = "__ccc_call_helper_collision";
        let assembly =
            GeneratedAssembly::new("helper", ".text\n", vec![helper.to_owned()], Vec::new())
                .unwrap();
        let error = ArtifactBundle::new(
            object_with_symbols(&[(helper, true)]),
            vec![assembly],
            BridgeManifestV2::new(
                [0; 32],
                vec![GeneratedSymbol::internal(
                    helper,
                    GeneratedSymbolKind::CallHelper,
                    GeneratedSymbolOwner::AssemblyUnit("helper".to_owned()),
                )],
            ),
        )
        .verify()
        .unwrap_err();
        assert!(error.message.contains("collides"));
    }

    #[test]
    fn manifest_rejects_distinct_logical_names_that_collide_on_macho() {
        let ordinary =
            GeneratedAssembly::new("ordinary", ".text\n", vec!["foo".to_owned()], Vec::new())
                .unwrap();
        let exact = GeneratedAssembly::new("exact", ".text\n", vec!["_foo".to_owned()], Vec::new())
            .unwrap();
        let error = ArtifactBundle::new(
            empty_macho_object(),
            vec![ordinary, exact],
            BridgeManifestV2::new(
                [0; 32],
                vec![
                    GeneratedSymbol::public(
                        "foo",
                        GeneratedSymbolKind::VariadicEntry,
                        GeneratedSymbolOwner::AssemblyUnit("ordinary".to_owned()),
                    ),
                    GeneratedSymbol::public(
                        "_foo",
                        GeneratedSymbolKind::VariadicEntry,
                        GeneratedSymbolOwner::AssemblyUnit("exact".to_owned()),
                    )
                    .with_exact_object_name(),
                ],
            ),
        )
        .verify()
        .unwrap_err();
        assert!(error.message.contains("same physical object symbol `_foo`"));
    }
}
