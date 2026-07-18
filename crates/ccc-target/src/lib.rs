//! Target defaults and the effective configuration shared by compiler phases.

mod compat;
mod layout;

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub use compat::{
    CapabilityEntry, CapabilityKey, CapabilityKind, CapabilityRegistry, CapabilityState,
    CompatibilityScope, CompatibilityVersion, GnuCompatibilityProfile,
};
pub use layout::{
    BitfieldLayoutPolicy, BitfieldOrder, ByteOrder, PackingPolicy, ScalarLayout, TargetDataLayout,
    TargetScalarKind,
};
pub use target_lexicon::{
    Architecture, BinaryFormat, CallingConvention, Environment, OperatingSystem, PointerWidth,
    Triple, Vendor,
};

/// The relocation contract used by generated objects.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RelocationModel {
    Static,
}

/// Compiler-provided C types whose representation is selected by the target
/// ABI rather than by the language's arithmetic type system.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum TargetBuiltinType {
    VaList,
}

/// The accepted source-language dialect.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum LanguageMode {
    /// C11 with the documented GNU compatibility profile enabled.
    #[default]
    Gnu11,
    /// Strict ISO C11 language rules.
    C11,
}

impl LanguageMode {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Gnu11 => "gnu11",
            Self::C11 => "c11",
        }
    }

    pub const fn accepts_gnu_extensions(self) -> bool {
        matches!(self, Self::Gnu11)
    }
}

/// How translation-phase trigraph replacement is selected.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum TrigraphPolicy {
    /// Follow the selected language mode: enabled for strict C11 and disabled
    /// for GNU C11.
    #[default]
    LanguageDefault,
    Enabled,
    Disabled,
}

impl TrigraphPolicy {
    pub const fn is_enabled(self, language_mode: LanguageMode) -> bool {
        match self {
            Self::LanguageDefault => matches!(language_mode, LanguageMode::C11),
            Self::Enabled => true,
            Self::Disabled => false,
        }
    }
}

/// Language choices that affect preprocessing and later compiler phases.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LanguageOptions {
    pub mode: LanguageMode,
    pub trigraphs: TrigraphPolicy,
}

impl LanguageOptions {
    pub const fn trigraphs_enabled(&self) -> bool {
        self.trigraphs.is_enabled(self.mode)
    }
}

/// Target-derived predefined macro spellings and replacement text.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PredefinedMacroFacts {
    values: BTreeMap<String, String>,
}

impl PredefinedMacroFacts {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(
        &mut self,
        name: impl Into<String>,
        replacement: impl Into<String>,
    ) -> Option<String> {
        self.values.insert(name.into(), replacement.into())
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(String::as_str)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.values
            .iter()
            .map(|(name, replacement)| (name.as_str(), replacement.as_str()))
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

/// Immutable defaults for an enabled target.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TargetSpec {
    pub triple: Triple,
    pub int_align: u8,
    pub data_layout: TargetDataLayout,
}

impl TargetSpec {
    pub fn pointer_width(&self) -> Option<u8> {
        self.triple.pointer_width().ok().map(PointerWidth::bits)
    }

    pub fn int_width(&self) -> Option<u8> {
        self.triple
            .data_model()
            .ok()
            .map(|model| model.int_size().bits())
    }

    pub fn calling_convention(&self) -> Option<CallingConvention> {
        self.triple.default_calling_convention().ok()
    }

    pub const fn scalar_layout(&self, kind: TargetScalarKind) -> ScalarLayout {
        self.data_layout.scalar(kind)
    }

    pub fn predefined_macro_facts(&self) -> PredefinedMacroFacts {
        let mut facts = PredefinedMacroFacts::new();
        facts.insert("__CHAR_BIT__", self.data_layout.char_width.to_string());
        facts.insert(
            "__SIZEOF_SHORT__",
            (self.data_layout.short_width / 8).to_string(),
        );
        facts.insert(
            "__SIZEOF_INT__",
            (self.data_layout.int_width / 8).to_string(),
        );
        facts.insert(
            "__SIZEOF_LONG__",
            (self.data_layout.long_width / 8).to_string(),
        );
        facts.insert(
            "__SIZEOF_LONG_LONG__",
            (self.data_layout.long_long_width / 8).to_string(),
        );
        facts.insert(
            "__SIZEOF_POINTER__",
            (self.data_layout.pointer_width / 8).to_string(),
        );
        facts.insert(
            "__SIZEOF_FLOAT__",
            (self.data_layout.float_width / 8).to_string(),
        );
        facts.insert(
            "__SIZEOF_DOUBLE__",
            (self.data_layout.double_width / 8).to_string(),
        );
        facts.insert(
            "__SIZEOF_LONG_DOUBLE__",
            (self.data_layout.long_double_width / 8).to_string(),
        );
        // A 64-bit double in the enabled target data-layout contract uses the
        // IEEE 754 binary64 representation. Keep the hosted `float.h` family
        // together so its precision, ranges, and exact boundary values agree.
        if self.data_layout.double_width == 64 {
            insert_binary64_compatibility_facts(&mut facts);
        }
        facts.insert(
            "__SIZEOF_SIZE_T__",
            (self.data_layout.pointer_width / 8).to_string(),
        );
        facts.insert(
            "__SIZEOF_PTRDIFF_T__",
            (self.data_layout.pointer_width / 8).to_string(),
        );
        facts.insert(
            "__SIZEOF_WCHAR_T__",
            (self.data_layout.wchar_width / 8).to_string(),
        );
        facts.insert(
            "__SIZEOF_WINT_T__",
            (self.data_layout.wint_width / 8).to_string(),
        );
        insert_integer_compatibility_facts(self, &mut facts);
        facts.insert("__ORDER_LITTLE_ENDIAN__", "1234");
        facts.insert("__ORDER_BIG_ENDIAN__", "4321");
        facts.insert("__ORDER_PDP_ENDIAN__", "3412");
        facts.insert("__BYTE_ORDER__", "__ORDER_LITTLE_ENDIAN__");

        if self.triple.architecture == Architecture::X86_64 {
            for name in ["__x86_64__", "__x86_64", "__amd64__", "__amd64"] {
                facts.insert(name, "1");
            }
        }
        if self.triple.operating_system == OperatingSystem::Linux {
            for name in ["__linux__", "__linux", "__unix__", "__unix"] {
                facts.insert(name, "1");
            }
        }
        if self.triple.binary_format == BinaryFormat::Elf {
            facts.insert("__ELF__", "1");
        }
        if self.triple.architecture == Architecture::X86_64
            && self.triple.binary_format == BinaryFormat::Elf
            && self.calling_convention() == Some(CallingConvention::SystemV)
        {
            facts.insert("__USER_LABEL_PREFIX__", "");
        }
        if self.data_layout.pointer_width == 64 && self.data_layout.long_width == 64 {
            facts.insert("__LP64__", "1");
            facts.insert("_LP64", "1");
        }
        facts
    }
}

fn insert_binary64_compatibility_facts(facts: &mut PredefinedMacroFacts) {
    facts.insert("__FLT_RADIX__", "2");
    facts.insert("__FLT_EVAL_METHOD__", "0");
    for (suffix, replacement) in [
        ("MANT_DIG", "53"),
        ("DIG", "15"),
        ("MIN_EXP", "(-1021)"),
        ("MIN_10_EXP", "(-307)"),
        ("MAX_EXP", "1024"),
        ("MAX_10_EXP", "308"),
        ("DECIMAL_DIG", "17"),
        ("HAS_DENORM", "1"),
        ("HAS_INFINITY", "1"),
        ("HAS_QUIET_NAN", "1"),
        ("MAX", "0x1.fffffffffffffp+1023"),
        ("NORM_MAX", "0x1.fffffffffffffp+1023"),
        ("EPSILON", "0x1p-52"),
        ("MIN", "0x1p-1022"),
        ("DENORM_MIN", "0x1p-1074"),
    ] {
        facts.insert(format!("__DBL_{suffix}__"), replacement);
    }
}

fn insert_integer_compatibility_facts(target: &TargetSpec, facts: &mut PredefinedMacroFacts) {
    let layout = &target.data_layout;
    for (name, width, suffix) in [
        ("__SCHAR_MAX__", layout.char_width, ""),
        ("__SHRT_MAX__", layout.short_width, ""),
        ("__INT_MAX__", layout.int_width, ""),
        ("__LONG_MAX__", layout.long_width, "L"),
        ("__LONG_LONG_MAX__", layout.long_long_width, "LL"),
    ] {
        facts.insert(name, format!("{}{suffix}", signed_max(width)));
    }
    for (stem, width) in [
        ("__INT8", 8),
        ("__INT16", 16),
        ("__INT32", 32),
        ("__INT64", 64),
    ] {
        if let Some(integer) = integer_type_for_width(layout, width) {
            insert_integer_family(facts, stem, width, integer);
            insert_named_integer_pair(facts, &format!("__INT_LEAST{width}"), width, integer);
            let (fast_width, fast) = if width == layout.char_width {
                (width, integer)
            } else {
                (
                    layout.long_width,
                    integer_type_for_width(layout, layout.long_width)
                        .expect("the target long type is available"),
                )
            };
            insert_named_integer_pair(facts, &format!("__INT_FAST{width}"), fast_width, fast);
        }
    }

    let pointer = integer_type_for_width(layout, layout.pointer_width)
        .expect("the pointer width has a matching C integer type");
    insert_named_integer_pair(facts, "__INTPTR", layout.pointer_width, pointer);
    insert_single_integer(facts, "__PTRDIFF", layout.pointer_width, pointer, true);
    facts.insert("__SIZE_TYPE__", pointer.unsigned_name);
    facts.insert(
        "__SIZE_MAX__",
        format!(
            "{}{}",
            unsigned_max(layout.pointer_width),
            pointer.unsigned_suffix
        ),
    );
    facts.insert("__SIZE_WIDTH__", layout.pointer_width.to_string());

    let maximum_width = layout.long_width.max(layout.long_long_width);
    let maximum = integer_type_for_width(layout, maximum_width)
        .expect("the maximum integer width has a matching C integer type");
    insert_named_integer_pair(facts, "__INTMAX", maximum_width, maximum);
    facts.insert("__INTMAX_C_SUFFIX__", maximum.signed_suffix);
    facts.insert("__UINTMAX_C_SUFFIX__", maximum.unsigned_suffix);

    let wchar = integer_type_for_width(layout, layout.wchar_width)
        .expect("wchar_t has a matching C integer type");
    insert_single_integer(
        facts,
        "__WCHAR",
        layout.wchar_width,
        wchar,
        layout.wchar_is_signed,
    );
    let wint = integer_type_for_width(layout, layout.wint_width)
        .expect("wint_t has a matching C integer type");
    insert_single_integer(
        facts,
        "__WINT",
        layout.wint_width,
        wint,
        layout.wint_is_signed,
    );

    let signal = integer_type_for_width(layout, layout.int_width)
        .expect("sig_atomic_t has a matching C integer type");
    facts.insert("__SIG_ATOMIC_TYPE__", signal.signed_name);
    facts.insert(
        "__SIG_ATOMIC_MAX__",
        format!("{}{}", signed_max(layout.int_width), signal.signed_suffix),
    );
    facts.insert("__SIG_ATOMIC_WIDTH__", layout.int_width.to_string());
    facts.insert("__CHAR16_TYPE__", "unsigned short");
    facts.insert("__CHAR32_TYPE__", "unsigned int");
    facts.insert("__POINTER_WIDTH__", layout.pointer_width.to_string());
    facts.insert("__INT_WIDTH__", layout.int_width.to_string());
    facts.insert("__LONG_WIDTH__", layout.long_width.to_string());
    facts.insert("__LLONG_WIDTH__", layout.long_long_width.to_string());
    facts.insert("__SHRT_WIDTH__", layout.short_width.to_string());
}

#[derive(Clone, Copy)]
struct IntegerTypeSpelling {
    signed_name: &'static str,
    unsigned_name: &'static str,
    signed_suffix: &'static str,
    unsigned_suffix: &'static str,
}

fn integer_type_for_width(layout: &TargetDataLayout, width: u8) -> Option<IntegerTypeSpelling> {
    [
        (
            layout.char_width,
            IntegerTypeSpelling {
                signed_name: "signed char",
                unsigned_name: "unsigned char",
                signed_suffix: "",
                unsigned_suffix: "",
            },
        ),
        (
            layout.short_width,
            IntegerTypeSpelling {
                signed_name: "short",
                unsigned_name: "unsigned short",
                signed_suffix: "",
                unsigned_suffix: "",
            },
        ),
        (
            layout.int_width,
            IntegerTypeSpelling {
                signed_name: "int",
                unsigned_name: "unsigned int",
                signed_suffix: "",
                unsigned_suffix: "U",
            },
        ),
        (
            layout.long_width,
            IntegerTypeSpelling {
                signed_name: "long int",
                unsigned_name: "long unsigned int",
                signed_suffix: "L",
                unsigned_suffix: "UL",
            },
        ),
        (
            layout.long_long_width,
            IntegerTypeSpelling {
                signed_name: "long long int",
                unsigned_name: "long long unsigned int",
                signed_suffix: "LL",
                unsigned_suffix: "ULL",
            },
        ),
    ]
    .into_iter()
    .find_map(|(candidate_width, spelling)| (candidate_width == width).then_some(spelling))
}

fn insert_integer_family(
    facts: &mut PredefinedMacroFacts,
    signed_stem: &str,
    width: u8,
    spelling: IntegerTypeSpelling,
) {
    insert_named_integer_pair(facts, signed_stem, width, spelling);
    facts.insert(format!("{signed_stem}_C_SUFFIX__"), spelling.signed_suffix);
    let unsigned_stem = signed_stem.replacen("__INT", "__UINT", 1);
    facts.insert(
        format!("{unsigned_stem}_C_SUFFIX__"),
        spelling.unsigned_suffix,
    );
}

fn insert_named_integer_pair(
    facts: &mut PredefinedMacroFacts,
    signed_stem: &str,
    width: u8,
    spelling: IntegerTypeSpelling,
) {
    facts.insert(format!("{signed_stem}_TYPE__"), spelling.signed_name);
    facts.insert(
        format!("{signed_stem}_MAX__"),
        format!("{}{}", signed_max(width), spelling.signed_suffix),
    );
    facts.insert(format!("{signed_stem}_WIDTH__"), width.to_string());
    let unsigned_stem = signed_stem.replacen("__INT", "__UINT", 1);
    facts.insert(format!("{unsigned_stem}_TYPE__"), spelling.unsigned_name);
    facts.insert(
        format!("{unsigned_stem}_MAX__"),
        format!("{}{}", unsigned_max(width), spelling.unsigned_suffix),
    );
    facts.insert(format!("{unsigned_stem}_WIDTH__"), width.to_string());
}

fn insert_single_integer(
    facts: &mut PredefinedMacroFacts,
    stem: &str,
    width: u8,
    spelling: IntegerTypeSpelling,
    signed: bool,
) {
    let (name, suffix, maximum) = if signed {
        (
            spelling.signed_name,
            spelling.signed_suffix,
            signed_max(width),
        )
    } else {
        (
            spelling.unsigned_name,
            spelling.unsigned_suffix,
            unsigned_max(width),
        )
    };
    facts.insert(format!("{stem}_TYPE__"), name);
    facts.insert(format!("{stem}_MAX__"), format!("{maximum}{suffix}"));
    facts.insert(format!("{stem}_WIDTH__"), width.to_string());
}

fn signed_max(width: u8) -> u128 {
    (1_u128 << (width - 1)) - 1
}

fn unsigned_max(width: u8) -> u128 {
    if width == 128 {
        u128::MAX
    } else {
        (1_u128 << width) - 1
    }
}

/// A command and the fixed arguments selected by toolchain resolution.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolCommandSpec {
    pub program: PathBuf,
    pub arguments: Vec<OsString>,
}

impl ToolCommandSpec {
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            arguments: Vec::new(),
        }
    }

    pub fn with_arguments<I, S>(program: impl Into<PathBuf>, arguments: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        Self {
            program: program.into(),
            arguments: arguments.into_iter().map(Into::into).collect(),
        }
    }

    pub fn display(&self) -> String {
        std::iter::once(self.program.to_string_lossy().into_owned())
            .chain(
                self.arguments
                    .iter()
                    .map(|argument| argument.to_string_lossy().into_owned()),
            )
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// The semantics of an include directory discovered from a target driver.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SystemIncludeKind {
    Quote,
    Builtin,
    System,
    Framework,
    After,
}

/// One ordered target include-search entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SystemIncludeEntry {
    pub path: PathBuf,
    pub kind: SystemIncludeKind,
}

impl SystemIncludeEntry {
    pub fn new(path: impl Into<PathBuf>, kind: SystemIncludeKind) -> Self {
        Self {
            path: path.into(),
            kind,
        }
    }
}

/// Inputs that identify the result of toolchain probing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolchainFingerprint {
    pub driver_path: PathBuf,
    pub driver_version: String,
    pub reported_target: Triple,
    pub target_arguments: Vec<OsString>,
    pub sysroot: Option<PathBuf>,
    pub resource_dir: Option<PathBuf>,
    pub system_includes: Vec<SystemIncludeEntry>,
    pub digest: String,
}

/// Phase-scoped tools and paths resolved for an effective configuration.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ToolchainSpec {
    pub compiler_driver: Option<ToolCommandSpec>,
    pub assembler: Option<ToolCommandSpec>,
    pub linker_driver: Option<ToolCommandSpec>,
    /// Object-file rewriting tool selected by the target compiler driver.
    ///
    /// This is resolved only for artifact bundles that contain generated
    /// assembly and therefore need exact symbol localization after a partial
    /// link.
    pub object_copier: Option<ToolCommandSpec>,
    pub archiver: Option<ToolCommandSpec>,
    pub ranlib: Option<ToolCommandSpec>,
    pub sysroot: Option<PathBuf>,
    pub resource_dir: Option<PathBuf>,
    pub system_includes: Vec<SystemIncludeEntry>,
    pub fingerprint: Option<ToolchainFingerprint>,
}

impl ToolchainSpec {
    pub fn has_system_headers(&self) -> bool {
        !self.system_includes.is_empty()
    }

    pub fn sysroot(&self) -> Option<&Path> {
        self.sysroot.as_deref()
    }
}

/// The effective configuration passed unchanged through the compiler.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EffectiveCompilationConfig {
    pub target: TargetSpec,
    pub language: LanguageOptions,
    pub gnu_profile: Option<GnuCompatibilityProfile>,
    pub capabilities: CapabilityRegistry,
    pub target_macros: PredefinedMacroFacts,
    pub resource_dir: Option<PathBuf>,
    pub toolchain: ToolchainSpec,
    pub relocation_model: RelocationModel,
}

impl EffectiveCompilationConfig {
    pub fn x86_64_unknown_linux_gnu() -> Self {
        let target = X86_64_UNKNOWN_LINUX_GNU;
        let target_macros = target.predefined_macro_facts();
        Self {
            target,
            language: LanguageOptions::default(),
            gnu_profile: Some(GnuCompatibilityProfile::gcc_4_2_1()),
            capabilities: CapabilityRegistry::gnu_frontend(),
            target_macros,
            resource_dir: None,
            toolchain: ToolchainSpec::default(),
            relocation_model: RelocationModel::Static,
        }
    }

    pub fn with_language_mode(mut self, mode: LanguageMode) -> Self {
        self.language.mode = mode;
        self
    }

    pub fn with_toolchain(mut self, toolchain: ToolchainSpec) -> Self {
        self.toolchain = toolchain;
        self
    }

    pub fn with_resource_dir(mut self, resource_dir: impl Into<PathBuf>) -> Self {
        self.resource_dir = Some(resource_dir.into());
        self
    }

    pub fn sysroot(&self) -> Option<&Path> {
        self.toolchain.sysroot()
    }

    pub fn system_includes(&self) -> &[SystemIncludeEntry] {
        &self.toolchain.system_includes
    }

    /// Standard, compatibility-profile, and target macros shared by every
    /// preprocessing entry point. Driver-owned compiler identity and feature
    /// denial macros are supplied separately.
    pub fn frontend_predefined_macros(&self) -> BTreeMap<String, String> {
        let mut macros = self
            .target_macros
            .iter()
            .map(|(name, replacement)| (name.to_owned(), replacement.to_owned()))
            .collect::<BTreeMap<_, _>>();
        macros.insert("__STDC__".to_owned(), "1".to_owned());
        macros.insert("__STDC_VERSION__".to_owned(), "201112L".to_owned());
        if self.language.mode == LanguageMode::C11 {
            macros.insert("__STRICT_ANSI__".to_owned(), "1".to_owned());
        }
        if let Some(profile) = &self.gnu_profile {
            macros.insert("__GNUC__".to_owned(), profile.version.major.to_string());
            macros.insert(
                "__GNUC_MINOR__".to_owned(),
                profile.version.minor.to_string(),
            );
            macros.insert(
                "__GNUC_PATCHLEVEL__".to_owned(),
                profile.version.patch.to_string(),
            );
        }
        macros
    }
}

impl Default for EffectiveCompilationConfig {
    fn default() -> Self {
        Self::x86_64_unknown_linux_gnu()
    }
}

/// The enabled x86-64 Linux GNU target.
pub const X86_64_UNKNOWN_LINUX_GNU: TargetSpec = TargetSpec {
    triple: Triple {
        architecture: Architecture::X86_64,
        vendor: Vendor::Unknown,
        operating_system: OperatingSystem::Linux,
        environment: Environment::Gnu,
        binary_format: BinaryFormat::Elf,
    },
    int_align: 4,
    data_layout: TargetDataLayout {
        byte_order: ByteOrder::Little,
        char_is_signed: true,
        bool_width: 8,
        bool_align: 1,
        char_width: 8,
        char_align: 1,
        short_width: 16,
        short_align: 2,
        int_width: 32,
        int_align: 4,
        long_width: 64,
        long_align: 8,
        long_long_width: 64,
        long_long_align: 8,
        pointer_width: 64,
        pointer_align: 8,
        float_width: 32,
        float_align: 4,
        double_width: 64,
        double_align: 8,
        long_double_width: 128,
        long_double_align: 16,
        wchar_width: 32,
        wchar_is_signed: true,
        wint_width: 32,
        wint_is_signed: false,
        bitfields: BitfieldLayoutPolicy {
            order: BitfieldOrder::LeastSignificantFirst,
            may_cross_storage_units: false,
            coalesce_different_declared_types: true,
            packed_fields_are_contiguous: true,
            zero_width_uses_declared_alignment: true,
        },
        default_packing: PackingPolicy::NATIVE,
    },
};

#[cfg(test)]
mod tests {
    use target_lexicon::CDataModel;

    use super::*;

    #[test]
    fn primary_configuration_is_derived_from_its_triple() {
        let config = EffectiveCompilationConfig::default();
        assert_eq!(config.target.triple.to_string(), "x86_64-unknown-linux-gnu");
        assert_eq!(config.target.triple.architecture, Architecture::X86_64);
        assert_eq!(config.target.triple.binary_format, BinaryFormat::Elf);
        assert_eq!(config.target.triple.data_model(), Ok(CDataModel::LP64));
        assert_eq!(config.target.int_width(), Some(32));
        assert_eq!(config.target.pointer_width(), Some(64));
        let bitfields = config.target.data_layout.bitfields;
        assert_eq!(bitfields.order, BitfieldOrder::LeastSignificantFirst);
        assert!(!bitfields.may_cross_storage_units);
        assert!(bitfields.coalesce_different_declared_types);
        assert!(bitfields.packed_fields_are_contiguous);
        assert!(bitfields.zero_width_uses_declared_alignment);
        assert_eq!(
            config.target.calling_convention(),
            Some(CallingConvention::SystemV)
        );
        assert_eq!(config.language.mode, LanguageMode::Gnu11);
        assert!(!config.language.trigraphs_enabled());
        assert_eq!(
            config.gnu_profile.as_ref().map(|profile| profile.version),
            Some(CompatibilityVersion::new(4, 2, 1))
        );
        assert_eq!(
            config.gnu_profile.as_ref().map(|profile| profile.scope),
            Some(CompatibilityScope::CodeGeneration)
        );
    }

    #[test]
    fn strict_c11_enables_language_default_trigraphs() {
        let config = EffectiveCompilationConfig::default().with_language_mode(LanguageMode::C11);
        assert!(config.language.trigraphs_enabled());

        let mut explicit = config;
        explicit.language.trigraphs = TrigraphPolicy::Disabled;
        assert!(!explicit.language.trigraphs_enabled());
    }

    #[test]
    fn target_macro_facts_follow_the_data_layout_and_triple() {
        let facts = &EffectiveCompilationConfig::default().target_macros;
        assert_eq!(facts.get("__SIZEOF_POINTER__"), Some("8"));
        assert_eq!(facts.get("__SIZEOF_LONG_DOUBLE__"), Some("16"));
        for (name, expected) in [
            ("__FLT_RADIX__", "2"),
            ("__FLT_EVAL_METHOD__", "0"),
            ("__DBL_MANT_DIG__", "53"),
            ("__DBL_DIG__", "15"),
            ("__DBL_MIN_EXP__", "(-1021)"),
            ("__DBL_MIN_10_EXP__", "(-307)"),
            ("__DBL_MAX_EXP__", "1024"),
            ("__DBL_MAX_10_EXP__", "308"),
            ("__DBL_DECIMAL_DIG__", "17"),
            ("__DBL_HAS_DENORM__", "1"),
            ("__DBL_HAS_INFINITY__", "1"),
            ("__DBL_HAS_QUIET_NAN__", "1"),
            ("__DBL_MAX__", "0x1.fffffffffffffp+1023"),
            ("__DBL_NORM_MAX__", "0x1.fffffffffffffp+1023"),
            ("__DBL_EPSILON__", "0x1p-52"),
            ("__DBL_MIN__", "0x1p-1022"),
            ("__DBL_DENORM_MIN__", "0x1p-1074"),
        ] {
            assert_eq!(facts.get(name), Some(expected), "unexpected {name}");
        }
        assert_eq!(facts.get("__SIZEOF_SIZE_T__"), Some("8"));
        assert_eq!(facts.get("__SIZE_TYPE__"), Some("long unsigned int"));
        assert_eq!(facts.get("__PTRDIFF_TYPE__"), Some("long int"));
        assert_eq!(facts.get("__WCHAR_TYPE__"), Some("int"));
        assert_eq!(facts.get("__WINT_TYPE__"), Some("unsigned int"));
        assert_eq!(facts.get("__INTMAX_TYPE__"), Some("long int"));
        assert_eq!(facts.get("__INT64_MAX__"), Some("9223372036854775807L"));
        assert_eq!(facts.get("__UINT64_MAX__"), Some("18446744073709551615UL"));
        assert_eq!(facts.get("__INT_FAST16_TYPE__"), Some("long int"));
        assert_eq!(facts.get("__BYTE_ORDER__"), Some("__ORDER_LITTLE_ENDIAN__"));
        assert_eq!(facts.get("__USER_LABEL_PREFIX__"), Some(""));
        assert_eq!(facts.get("__x86_64__"), Some("1"));
        assert_eq!(facts.get("__linux__"), Some("1"));
        assert_eq!(facts.get("linux"), None);
        assert_eq!(facts.get("unix"), None);
    }

    #[test]
    fn compiler_resources_are_distinct_from_toolchain_resources() {
        let config = EffectiveCompilationConfig::default()
            .with_resource_dir("/opt/ccc/resources")
            .with_toolchain(ToolchainSpec {
                resource_dir: Some(PathBuf::from("/opt/clang/resources")),
                ..ToolchainSpec::default()
            });

        assert_eq!(
            config.resource_dir.as_deref(),
            Some(Path::new("/opt/ccc/resources"))
        );
        assert_eq!(
            config.toolchain.resource_dir.as_deref(),
            Some(Path::new("/opt/clang/resources"))
        );
    }
}
