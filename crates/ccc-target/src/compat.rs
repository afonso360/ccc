//! Hosted-header compatibility profiles and capability policy.

use std::collections::BTreeMap;

/// A compiler compatibility version advertised to hosted headers.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CompatibilityVersion {
    pub major: u16,
    pub minor: u16,
    pub patch: u16,
}

impl CompatibilityVersion {
    pub const fn new(major: u16, minor: u16, patch: u16) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }
}

/// The compiler phases certified by a hosted-header compatibility profile.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(u8)]
pub enum CompatibilityScope {
    /// The profile selects and expands hosted headers but does not certify the
    /// resulting declarations for parsing or semantic analysis.
    Preprocessing,
    /// The profile certifies token conversion and parsing of selected headers.
    Parsing,
    /// The profile certifies semantic analysis of selected declarations.
    SemanticAnalysis,
    /// The profile certifies code generation for selected declarations.
    CodeGeneration,
}

impl CompatibilityScope {
    /// Whether this ceiling includes the requested compiler activity.
    pub const fn includes(self, required: Self) -> bool {
        self as u8 >= required as u8
    }
}

/// The GNU compatibility contract used to select hosted-header paths.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GnuCompatibilityProfile {
    pub name: String,
    pub version: CompatibilityVersion,
    pub scope: CompatibilityScope,
}

impl GnuCompatibilityProfile {
    pub fn gcc_4_2_1() -> Self {
        Self {
            name: "gcc-4.2.1".to_owned(),
            version: CompatibilityVersion::new(4, 2, 1),
            scope: CompatibilityScope::CodeGeneration,
        }
    }
}

/// The family of a compatibility capability.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum CapabilityKind {
    Attribute,
    Builtin,
    Extension,
    Feature,
    Pragma,
}

/// The semantic state of a compatibility capability.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum CapabilityState {
    Implemented,
    BehaviorCompatibleNoOp,
    ParseOnly,
    Unsupported,
}

impl CapabilityState {
    /// Whether a feature predicate may truthfully report this capability.
    pub const fn is_available(self) -> bool {
        matches!(self, Self::Implemented | Self::BehaviorCompatibleNoOp)
    }
}

/// A stable lookup key in the shared compatibility registry.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CapabilityKey {
    pub kind: CapabilityKind,
    pub name: String,
}

impl CapabilityKey {
    pub fn new(kind: CapabilityKind, name: impl Into<String>) -> Self {
        Self {
            kind,
            name: name.into(),
        }
    }
}

/// One entry in the shared compatibility registry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapabilityEntry {
    pub state: CapabilityState,
    pub rationale: Option<String>,
}

/// Compatibility facts shared by preprocessing, parsing, and diagnostics.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CapabilityRegistry {
    entries: BTreeMap<CapabilityKey, CapabilityEntry>,
}

impl CapabilityRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Capability facts for the reserved GNU spellings used by hosted headers.
    pub fn gnu_frontend() -> Self {
        let mut registry = Self::new();

        for name in [
            "__const",
            "__const__",
            "__inline",
            "__inline__",
            "__restrict",
            "__restrict__",
            "__signed",
            "__signed__",
            "__volatile",
            "__volatile__",
            "__alignof",
            "__alignof__",
            "__thread",
            "gnu-declaration-asm-labels",
            "gnu-function-name-aliases",
            "gnu-statement-expressions",
        ] {
            registry.insert(
                CapabilityKind::Extension,
                name,
                CapabilityState::Implemented,
            );
        }
        for name in ["__builtin_memcpy", "__builtin_memmove", "__builtin_memset"] {
            registry.insert_with_rationale(
                CapabilityKind::Builtin,
                name,
                CapabilityState::Implemented,
                "the frontend enforces the libc memory-operation signature and the backend emits the matching target libcall",
            );
        }

        for name in [
            "__asm",
            "__asm__",
            "__attribute",
            "__attribute__",
            "__typeof",
            "__typeof__",
            "gnu-alternative-keywords",
            "gnu-attribute-specifiers",
            "gnu-typeof",
        ] {
            registry.insert_with_rationale(
                CapabilityKind::Extension,
                name,
                CapabilityState::ParseOnly,
                "the frontend preserves this construct but does not promise its complete semantics",
            );
        }

        for name in ["__extension__", "gnu-extension-marker"] {
            registry.insert_with_rationale(
                CapabilityKind::Extension,
                name,
                CapabilityState::BehaviorCompatibleNoOp,
                "the marker only controls diagnostics that CCC does not emit",
            );
        }
        registry.insert(
            CapabilityKind::Extension,
            "gnu-restrict-qualifiers",
            CapabilityState::Implemented,
        );

        for name in [
            "nothrow",
            "__nothrow__",
            "pure",
            "__pure__",
            "const",
            "__const__",
            "malloc",
            "__malloc__",
            "format",
            "__format__",
            "format_arg",
            "__format_arg__",
            "nonnull",
            "__nonnull__",
            "warn_unused_result",
            "__warn_unused_result__",
            "unused",
            "__unused__",
            "deprecated",
            "__deprecated__",
            "may_alias",
            "__may_alias__",
            "alloc_size",
            "__alloc_size__",
        ] {
            registry.insert_with_rationale(
                CapabilityKind::Attribute,
                name,
                CapabilityState::BehaviorCompatibleNoOp,
                "the optimization or diagnostic contract does not alter generated C behavior",
            );
        }
        for name in [
            "noinline",
            "__noinline__",
            "always_inline",
            "__always_inline__",
        ] {
            registry.insert_with_rationale(
                CapabilityKind::Attribute,
                name,
                CapabilityState::Implemented,
                "semantic analysis validates the function-only optimization contract and preserves it as an exact CCC-IR function property",
            );
        }
        for name in [
            "noreturn",
            "__noreturn__",
            "packed",
            "__packed__",
            "mode",
            "__mode__",
            "aligned",
            "__aligned__",
            "weak",
            "__weak__",
            "transparent_union",
            "__transparent_union__",
            "tls_model",
            "__tls_model__",
            "returns_twice",
            "__returns_twice__",
        ] {
            registry.insert_with_rationale(
                CapabilityKind::Attribute,
                name,
                CapabilityState::Implemented,
                "the frontend preserves the attribute's control-flow, type, alignment, or symbol-emission effect",
            );
        }
        registry.insert_with_rationale(
            CapabilityKind::Attribute,
            "visibility",
            CapabilityState::Implemented,
            "the frontend carries ELF visibility through ABI planning and object emission",
        );
        for name in ["gnu_inline", "__gnu_inline__"] {
            registry.insert_with_rationale(
                CapabilityKind::Attribute,
                name,
                CapabilityState::ParseOnly,
                "the attribute has an observable diagnostic, semantic, layout, or emission effect",
            );
        }
        registry.insert_with_rationale(
            CapabilityKind::Builtin,
            "__builtin_offsetof",
            CapabilityState::Implemented,
            "the operator uses the canonical target layout engine",
        );
        registry.insert_with_rationale(
            CapabilityKind::Builtin,
            "__builtin_expect",
            CapabilityState::Implemented,
            "the GNU 4.2 operator requires a folded compile-time integer expectation and evaluates only the long-converted value operand; backend branch metadata is optional",
        );
        registry.insert_with_rationale(
            CapabilityKind::Builtin,
            "__builtin_huge_val",
            CapabilityState::Implemented,
            "the operator produces the target binary64 positive-infinity constant",
        );
        registry.insert_with_rationale(
            CapabilityKind::Builtin,
            "__builtin_inff",
            CapabilityState::Implemented,
            "the operator produces the target binary32 positive-infinity constant",
        );
        registry.insert_with_rationale(
            CapabilityKind::Builtin,
            "__builtin_nanf",
            CapabilityState::Implemented,
            "the certified empty narrow string-literal form produces a canonical target binary32 quiet NaN",
        );
        for name in [
            "__builtin_bswap64",
            "__builtin_clz",
            "__builtin_clzl",
            "__builtin_clzll",
            "__builtin_ctz",
            "__builtin_ctzll",
            "__builtin_popcount",
            "__builtin_popcountll",
        ] {
            registry.insert_with_rationale(
                CapabilityKind::Builtin,
                name,
                CapabilityState::Implemented,
                "the exact integer signature is typed by the frontend and lowered to a native Cranelift integer intrinsic",
            );
        }
        registry.insert_with_rationale(
            CapabilityKind::Builtin,
            "__builtin_prefetch",
            CapabilityState::BehaviorCompatibleNoOp,
            "the address expression is evaluated exactly once and validated constant hints are discarded without introducing a faulting access",
        );
        registry.insert_with_rationale(
            CapabilityKind::Pragma,
            "GCC optimize",
            CapabilityState::BehaviorCompatibleNoOp,
            "the optimization hint does not alter the C abstract-machine behavior and CCC does not expose per-function optimization controls",
        );
        for name in [
            "__builtin_va_start",
            "__builtin_va_arg",
            "__builtin_va_copy",
            "__builtin_va_end",
        ] {
            registry.insert_with_rationale(
                CapabilityKind::Builtin,
                name,
                CapabilityState::Implemented,
                "the operator is typed by the frontend and lowered through the target ABI plan",
            );
        }
        registry.insert_with_rationale(
            CapabilityKind::Builtin,
            "__sync_synchronize",
            CapabilityState::Implemented,
            "the operator is typed and lowered as a sequentially consistent full memory fence",
        );
        for name in [
            "__sync_add_and_fetch",
            "__sync_fetch_and_add",
            "__sync_sub_and_fetch",
            "__sync_bool_compare_and_swap",
            "__sync_val_compare_and_swap",
            "__sync_lock_test_and_set",
        ] {
            registry.insert_with_rationale(
                CapabilityKind::Builtin,
                name,
                CapabilityState::Implemented,
                "the operator is typed and lowered as a native sequentially consistent atomic read-modify-write",
            );
        }
        for name in [
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
            "__ccc_atomic_is_lock_free",
            "__atomic_thread_fence",
            "__atomic_signal_fence",
        ] {
            registry.insert_with_rationale(
                CapabilityKind::Builtin,
                name,
                CapabilityState::Implemented,
                "the operator supports naturally aligned 1, 2, 4, and 8-byte integer and pointer objects and strengthens every accepted memory order to sequential consistency",
            );
        }
        registry.insert_with_rationale(
            CapabilityKind::Feature,
            "c11-native-scalar-atomics",
            CapabilityState::Implemented,
            "the compiler-owned header and native lowering support naturally aligned 1, 2, 4, and 8-byte integer and pointer atomics; the complete C atomic-type capability remains unadvertised",
        );
        registry.insert_with_rationale(
            CapabilityKind::Feature,
            "c11-vla",
            CapabilityState::Implemented,
            "variably modified declarations and type names retain exactly-once bounds; checked runtime layout, pointer strides, sizeof, and hosted automatic storage are implemented on every enabled target",
        );

        registry
    }

    pub fn insert(
        &mut self,
        kind: CapabilityKind,
        name: impl Into<String>,
        state: CapabilityState,
    ) -> Option<CapabilityEntry> {
        self.entries.insert(
            CapabilityKey::new(kind, name),
            CapabilityEntry {
                state,
                rationale: None,
            },
        )
    }

    pub fn insert_with_rationale(
        &mut self,
        kind: CapabilityKind,
        name: impl Into<String>,
        state: CapabilityState,
        rationale: impl Into<String>,
    ) -> Option<CapabilityEntry> {
        self.entries.insert(
            CapabilityKey::new(kind, name),
            CapabilityEntry {
                state,
                rationale: Some(rationale.into()),
            },
        )
    }

    pub fn entry(&self, kind: CapabilityKind, name: &str) -> Option<&CapabilityEntry> {
        self.entries.get(&CapabilityKey::new(kind, name))
    }

    /// Unknown entries are unsupported rather than optimistically accepted.
    pub fn state(&self, kind: CapabilityKind, name: &str) -> CapabilityState {
        self.entry(kind, name)
            .map_or(CapabilityState::Unsupported, |entry| entry.state)
    }

    pub fn is_available(&self, kind: CapabilityKind, name: &str) -> bool {
        self.state(kind, name).is_available()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&CapabilityKey, &CapabilityEntry)> {
        self.entries.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EffectiveCompilationConfig;

    #[test]
    fn compatibility_scopes_form_ordered_certification_ceilings() {
        assert!(CompatibilityScope::CodeGeneration.includes(CompatibilityScope::Preprocessing));
        assert!(CompatibilityScope::CodeGeneration.includes(CompatibilityScope::SemanticAnalysis));
        assert!(CompatibilityScope::Parsing.includes(CompatibilityScope::Parsing));
        assert!(!CompatibilityScope::Parsing.includes(CompatibilityScope::SemanticAnalysis));
        assert!(!CompatibilityScope::Preprocessing.includes(CompatibilityScope::Parsing));
        assert_eq!(
            GnuCompatibilityProfile::gcc_4_2_1().scope,
            CompatibilityScope::CodeGeneration
        );
    }

    #[test]
    fn default_registry_classifies_the_hosted_gnu_declaration_surface() {
        let registry = &EffectiveCompilationConfig::default().capabilities;

        for name in [
            "__const",
            "__inline__",
            "__restrict__",
            "__signed__",
            "__alignof__",
            "__thread",
            "gnu-declaration-asm-labels",
            "gnu-function-name-aliases",
            "gnu-statement-expressions",
        ] {
            assert_eq!(
                registry.state(CapabilityKind::Extension, name),
                CapabilityState::Implemented,
                "unexpected state for {name}"
            );
        }
        for name in ["__asm__", "__attribute__", "__typeof__", "gnu-typeof"] {
            assert_eq!(
                registry.state(CapabilityKind::Extension, name),
                CapabilityState::ParseOnly,
                "unexpected state for {name}"
            );
            assert!(!registry.is_available(CapabilityKind::Extension, name));
        }
        for name in ["__extension__", "gnu-extension-marker"] {
            assert_eq!(
                registry.state(CapabilityKind::Extension, name),
                CapabilityState::BehaviorCompatibleNoOp,
                "unexpected state for {name}"
            );
            assert!(registry.is_available(CapabilityKind::Extension, name));
        }
        for name in [
            "__nothrow__",
            "__pure__",
            "__const__",
            "__malloc__",
            "__format__",
            "format_arg",
            "__format_arg__",
            "__nonnull__",
            "__warn_unused_result__",
            "unused",
            "__unused__",
            "__deprecated__",
            "may_alias",
            "__may_alias__",
            "alloc_size",
            "__alloc_size__",
        ] {
            assert_eq!(
                registry.state(CapabilityKind::Attribute, name),
                CapabilityState::BehaviorCompatibleNoOp,
                "unexpected state for {name}"
            );
            assert!(registry.is_available(CapabilityKind::Attribute, name));
        }
        for name in [
            "noinline",
            "__noinline__",
            "always_inline",
            "__always_inline__",
            "__noreturn__",
            "packed",
            "__packed__",
            "__mode__",
            "__aligned__",
            "weak",
            "__weak__",
            "transparent_union",
            "__transparent_union__",
            "tls_model",
            "__tls_model__",
        ] {
            assert_eq!(
                registry.state(CapabilityKind::Attribute, name),
                CapabilityState::Implemented,
                "unexpected state for {name}"
            );
            assert!(registry.is_available(CapabilityKind::Attribute, name));
        }
        assert_eq!(
            registry.state(CapabilityKind::Attribute, "visibility"),
            CapabilityState::Implemented
        );
        assert_eq!(
            registry.state(CapabilityKind::Attribute, "__gnu_inline__"),
            CapabilityState::ParseOnly
        );
        assert!(!registry.is_available(CapabilityKind::Attribute, "__gnu_inline__"));
        for name in [
            "__builtin_offsetof",
            "__builtin_expect",
            "__builtin_huge_val",
            "__builtin_inff",
            "__builtin_nanf",
            "__builtin_bswap64",
            "__builtin_clz",
            "__builtin_clzl",
            "__builtin_clzll",
            "__builtin_ctz",
            "__builtin_ctzll",
            "__builtin_popcount",
            "__builtin_popcountll",
            "__builtin_memcpy",
            "__builtin_memmove",
            "__builtin_memset",
            "__builtin_va_start",
            "__builtin_va_arg",
            "__builtin_va_copy",
            "__builtin_va_end",
            "__sync_add_and_fetch",
            "__sync_fetch_and_add",
            "__sync_sub_and_fetch",
            "__sync_bool_compare_and_swap",
            "__sync_val_compare_and_swap",
            "__sync_lock_test_and_set",
            "__sync_synchronize",
        ] {
            assert_eq!(
                registry.state(CapabilityKind::Builtin, name),
                CapabilityState::Implemented
            );
            assert!(registry.is_available(CapabilityKind::Builtin, name));
        }
        assert_eq!(
            registry.state(CapabilityKind::Builtin, "__builtin_prefetch"),
            CapabilityState::BehaviorCompatibleNoOp
        );
        assert!(registry.is_available(CapabilityKind::Builtin, "__builtin_prefetch"));
        assert_eq!(
            registry.state(CapabilityKind::Builtin, "__builtin_bswap32"),
            CapabilityState::Unsupported
        );
        assert!(!registry.is_available(CapabilityKind::Builtin, "__builtin_bswap32"));
    }

    #[test]
    fn unknown_capabilities_are_not_advertised() {
        let mut registry = CapabilityRegistry::new();
        assert_eq!(
            registry.state(CapabilityKind::Builtin, "__builtin_unknown"),
            CapabilityState::Unsupported
        );
        registry.insert(
            CapabilityKind::Attribute,
            "unused",
            CapabilityState::BehaviorCompatibleNoOp,
        );
        assert!(registry.is_available(CapabilityKind::Attribute, "unused"));
        registry.insert(
            CapabilityKind::Builtin,
            "__builtin_parse_only",
            CapabilityState::ParseOnly,
        );
        assert!(!registry.is_available(CapabilityKind::Builtin, "__builtin_parse_only"));
    }
}
