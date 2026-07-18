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
    Aarch64Architecture, Architecture, BinaryFormat, CallingConvention, Environment,
    OperatingSystem, PointerWidth, Riscv64Architecture, Triple, Vendor,
};

/// A CCC-owned C ABI identity.
///
/// `target-lexicon` calling conventions are backend-facing categories and are
/// not precise enough to key C layout, aggregate classification, variadics,
/// or generated boundary artifacts. This identity is therefore the stable
/// planner and digest key.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum AbiIdentity {
    SysvAmd64Lp64,
    Aapcs64Lp64,
    RiscvLp64d,
    DarwinArm64,
}

impl AbiIdentity {
    pub const fn name(self) -> &'static str {
        match self {
            Self::SysvAmd64Lp64 => "sysv-amd64-lp64",
            Self::Aapcs64Lp64 => "aapcs64-lp64",
            Self::RiscvLp64d => "riscv-lp64d",
            Self::DarwinArm64 => "darwin-arm64",
        }
    }

    pub const fn calling_convention(self) -> CallingConvention {
        match self {
            Self::DarwinArm64 => CallingConvention::AppleAarch64,
            Self::SysvAmd64Lp64 | Self::Aapcs64Lp64 | Self::RiscvLp64d => {
                CallingConvention::SystemV
            }
        }
    }

    pub const fn is_linux(self) -> bool {
        matches!(
            self,
            Self::SysvAmd64Lp64 | Self::Aapcs64Lp64 | Self::RiscvLp64d
        )
    }

    /// Whether CCC has complete object, link, and execution evidence for
    /// thread-local storage under this ABI profile.
    pub const fn supports_tls_codegen(self) -> bool {
        matches!(self, Self::SysvAmd64Lp64)
    }
}

/// The relocation and executable-output contract used by code generation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RelocationModel {
    /// Generate code that may only be linked at a fixed address.
    Static,
    /// Generate position-independent code suitable for PIE and shared objects.
    Pic,
    /// Generate position-independent code and advertise a PIE compilation.
    Pie,
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
    pub abi: AbiIdentity,
    pub int_align: u8,
    pub data_layout: TargetDataLayout,
}

impl TargetSpec {
    /// Selects one of the complete target profiles enabled by CCC.
    pub fn enabled(triple: Triple) -> Result<Self, String> {
        [
            X86_64_UNKNOWN_LINUX_GNU,
            AARCH64_UNKNOWN_LINUX_GNU,
            RISCV64_UNKNOWN_LINUX_GNU,
            AARCH64_APPLE_DARWIN,
        ]
        .into_iter()
        .find(|profile| profile.triple == triple)
        .ok_or_else(|| format!("target `{triple}` is not an enabled CCC target profile"))
    }

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
        Some(self.abi.calling_convention())
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
        // A 32-bit float in the enabled target data-layout contract uses the
        // IEEE 754 binary32 representation.  Hosted GCC headers spell their
        // <float.h> limits in terms of this predefined-macro family.
        if self.data_layout.float_width == 32 {
            insert_binary32_compatibility_facts(&mut facts);
        }
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
        if !self.data_layout.char_is_signed {
            facts.insert("__CHAR_UNSIGNED__", "1");
        }
        if !self.data_layout.wchar_is_signed {
            facts.insert("__WCHAR_UNSIGNED__", "1");
        }
        if !self.data_layout.wint_is_signed {
            facts.insert("__WINT_UNSIGNED__", "1");
        }
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
        if matches!(
            self.triple.architecture,
            Architecture::Aarch64(Aarch64Architecture::Aarch64)
        ) {
            facts.insert("__aarch64__", "1");
            facts.insert("__AARCH64EL__", "1");
            facts.insert("__ARM_ARCH", "8");
            facts.insert("__ARM_ARCH_ISA_A64", "1");
            facts.insert("__ARM_64BIT_STATE", "1");
            facts.insert("__ARM_PCS_AAPCS64", "1");
            facts.insert("__ARM_SIZEOF_MINIMAL_ENUM", "4");
            facts.insert("__ARM_SIZEOF_WCHAR_T", "4");
            if self.abi == AbiIdentity::DarwinArm64 {
                facts.insert("__ARM64_ARCH_8__", "1");
                facts.insert("__ARM_ARCH_PROFILE", "'A'");
                facts.insert("__ARM_FP", "0xE");
                facts.insert("__ARM_ALIGN_MAX_STACK_PWR", "4");
                facts.insert("__BIGGEST_ALIGNMENT__", "8");
            } else {
                facts.insert("__ARM_ARCH_8A", "1");
                facts.insert("__ARM_ARCH_PROFILE", "65");
                facts.insert("__ARM_FP", "14");
                facts.insert("__ARM_ALIGN_MAX_PWR", "28");
                facts.insert("__ARM_ALIGN_MAX_STACK_PWR", "16");
                facts.insert("__BIGGEST_ALIGNMENT__", "16");
            }
        }
        if matches!(self.abi, AbiIdentity::RiscvLp64d) {
            facts.insert("__riscv", "1");
            facts.insert("__riscv_xlen", "64");
            facts.insert("__riscv_float_abi_double", "1");
            facts.insert("__riscv_flen", "64");
            for (name, version) in [
                ("__riscv_i", "2001000"),
                ("__riscv_m", "2000000"),
                ("__riscv_a", "2001000"),
                ("__riscv_c", "2000000"),
                ("__riscv_f", "2002000"),
                ("__riscv_d", "2002000"),
                ("__riscv_zicsr", "2000000"),
                ("__riscv_zifencei", "2000000"),
            ] {
                facts.insert(name, version);
            }
            for name in [
                "__riscv_atomic",
                "__riscv_compressed",
                "__riscv_div",
                "__riscv_mul",
                "__riscv_muldiv",
                "__riscv_fdiv",
                "__riscv_fsqrt",
            ] {
                facts.insert(name, "1");
            }
            facts.insert("__BIGGEST_ALIGNMENT__", "16");
        }
        if self.triple.operating_system == OperatingSystem::Linux {
            for name in ["__linux__", "__linux", "__unix__", "__unix"] {
                facts.insert(name, "1");
            }
        }
        if matches!(
            self.triple.operating_system,
            OperatingSystem::Darwin(_) | OperatingSystem::MacOSX(_)
        ) {
            facts.insert("__APPLE__", "1");
            facts.insert("__APPLE_CC__", "6000");
            facts.insert("__MACH__", "1");
            facts.insert("__arm64", "1");
            facts.insert("__arm64__", "1");
        }
        if self.triple.binary_format == BinaryFormat::Elf {
            facts.insert("__ELF__", "1");
        }
        facts.insert(
            "__USER_LABEL_PREFIX__",
            if self.triple.binary_format == BinaryFormat::Macho {
                "_"
            } else {
                ""
            },
        );
        insert_long_double_compatibility_facts(self, &mut facts);
        if self.data_layout.pointer_width == 64 && self.data_layout.long_width == 64 {
            facts.insert("__LP64__", "1");
            facts.insert("_LP64", "1");
        }
        facts
    }
}

fn insert_binary32_compatibility_facts(facts: &mut PredefinedMacroFacts) {
    for (suffix, replacement) in [
        ("MANT_DIG", "24"),
        ("DIG", "6"),
        ("MIN_EXP", "(-125)"),
        ("MIN_10_EXP", "(-37)"),
        ("MAX_EXP", "128"),
        ("MAX_10_EXP", "38"),
        ("DECIMAL_DIG", "9"),
        ("HAS_DENORM", "1"),
        ("HAS_INFINITY", "1"),
        ("HAS_QUIET_NAN", "1"),
        ("MAX", "0x1.fffffep+127F"),
        ("NORM_MAX", "0x1.fffffep+127F"),
        ("EPSILON", "0x1p-23F"),
        ("MIN", "0x1p-126F"),
        ("DENORM_MIN", "0x1p-149F"),
    ] {
        facts.insert(format!("__FLT_{suffix}__"), replacement);
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

fn insert_long_double_compatibility_facts(target: &TargetSpec, facts: &mut PredefinedMacroFacts) {
    match target.abi {
        AbiIdentity::DarwinArm64 => {
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
                ("MAX", "0x1.fffffffffffffp+1023L"),
                ("NORM_MAX", "0x1.fffffffffffffp+1023L"),
                ("EPSILON", "0x1p-52L"),
                ("MIN", "0x1p-1022L"),
                ("DENORM_MIN", "0x1p-1074L"),
            ] {
                facts.insert(format!("__LDBL_{suffix}__"), replacement);
            }
        }
        AbiIdentity::SysvAmd64Lp64 => {
            for (suffix, replacement) in [
                ("MANT_DIG", "64"),
                ("DIG", "18"),
                ("MIN_EXP", "(-16381)"),
                ("MIN_10_EXP", "(-4931)"),
                ("MAX_EXP", "16384"),
                ("MAX_10_EXP", "4932"),
                ("DECIMAL_DIG", "21"),
                ("HAS_DENORM", "1"),
                ("HAS_INFINITY", "1"),
                ("HAS_QUIET_NAN", "1"),
                ("MAX", "0xf.fffffffffffffffp+16380L"),
                ("EPSILON", "0x8p-66L"),
                ("MIN", "0x8p-16385L"),
                ("DENORM_MIN", "0x0.000000000000001p-16385L"),
            ] {
                facts.insert(format!("__LDBL_{suffix}__"), replacement);
            }
        }
        AbiIdentity::Aapcs64Lp64 | AbiIdentity::RiscvLp64d => {
            // The Linux profiles store `long double` as IEEE binary128. These
            // are representation facts only; arithmetic and native transport
            // remain capability-checked by semantic and ABI analysis.
            facts.insert("__LONG_DOUBLE_128__", "1");
            for (suffix, replacement) in [
                ("MANT_DIG", "113"),
                ("DIG", "33"),
                ("MIN_EXP", "(-16381)"),
                ("MIN_10_EXP", "(-4931)"),
                ("MAX_EXP", "16384"),
                ("MAX_10_EXP", "4932"),
                ("DECIMAL_DIG", "36"),
                ("HAS_DENORM", "1"),
                ("HAS_INFINITY", "1"),
                ("HAS_QUIET_NAN", "1"),
                ("MAX", "0x1.ffffffffffffffffffffffffffffp+16383L"),
                ("EPSILON", "0x1p-112L"),
                ("MIN", "0x1p-16382L"),
                ("DENORM_MIN", "0x1p-16494L"),
            ] {
                facts.insert(format!("__LDBL_{suffix}__"), replacement);
            }
        }
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
    pub target_arch: Option<String>,
    pub target_cpu: Option<String>,
    pub target_abi: Option<String>,
    pub sdk_root: Option<PathBuf>,
    pub deployment_target: Option<String>,
}

pub const DARWIN_ARM64_MINIMUM_DEPLOYMENT_TARGET: &str = "11.0";

impl EffectiveCompilationConfig {
    pub fn x86_64_unknown_linux_gnu() -> Self {
        Self::for_spec(X86_64_UNKNOWN_LINUX_GNU)
    }

    pub fn aarch64_unknown_linux_gnu() -> Self {
        Self::for_spec(AARCH64_UNKNOWN_LINUX_GNU)
    }

    pub fn riscv64_unknown_linux_gnu() -> Self {
        Self::for_spec(RISCV64_UNKNOWN_LINUX_GNU)
    }

    pub fn aarch64_apple_darwin() -> Self {
        Self::for_spec(AARCH64_APPLE_DARWIN)
    }

    pub fn for_target(triple: Triple) -> Result<Self, String> {
        TargetSpec::enabled(triple).map(Self::for_spec)
    }

    /// Selects the native host only when that host has a complete enabled
    /// profile. There is deliberately no architecture fallback.
    pub fn host() -> Result<Self, String> {
        Self::for_host_triple(target_lexicon::HOST)
    }

    fn for_host_triple(host: Triple) -> Result<Self, String> {
        let profile = match (
            &host.architecture,
            &host.vendor,
            &host.operating_system,
            &host.environment,
        ) {
            (Architecture::X86_64, _, OperatingSystem::Linux, Environment::Gnu) => {
                X86_64_UNKNOWN_LINUX_GNU
            }
            (
                Architecture::Aarch64(Aarch64Architecture::Aarch64),
                _,
                OperatingSystem::Linux,
                Environment::Gnu,
            ) => AARCH64_UNKNOWN_LINUX_GNU,
            (
                Architecture::Riscv64(Riscv64Architecture::Riscv64),
                _,
                OperatingSystem::Linux,
                Environment::Gnu,
            ) => RISCV64_UNKNOWN_LINUX_GNU,
            (
                Architecture::Aarch64(Aarch64Architecture::Aarch64),
                Vendor::Apple,
                OperatingSystem::Darwin(_) | OperatingSystem::MacOSX(_),
                Environment::Unknown,
            ) => AARCH64_APPLE_DARWIN,
            _ => {
                return Err(format!(
                    "native host target `{host}` is not an enabled CCC target profile"
                ));
            }
        };
        Ok(Self::for_spec(profile))
    }

    fn for_spec(target: TargetSpec) -> Self {
        let target_macros = target.predefined_macro_facts();
        Self {
            target,
            language: LanguageOptions::default(),
            gnu_profile: Some(GnuCompatibilityProfile::gcc_4_2_1()),
            capabilities: CapabilityRegistry::gnu_frontend(),
            target_macros,
            resource_dir: None,
            toolchain: ToolchainSpec::default(),
            relocation_model: RelocationModel::Pie,
            target_arch: None,
            target_cpu: None,
            target_abi: None,
            sdk_root: None,
            deployment_target: None,
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

    pub fn with_target_arch(mut self, architecture: impl Into<String>) -> Self {
        self.target_arch = Some(architecture.into());
        self
    }

    pub fn with_target_cpu(mut self, cpu: impl Into<String>) -> Self {
        self.target_cpu = Some(cpu.into());
        self
    }

    pub fn with_target_abi(mut self, abi: impl Into<String>) -> Self {
        self.target_abi = Some(abi.into());
        self
    }

    pub const fn normalized_target_arch(&self) -> &'static str {
        match self.target.abi {
            AbiIdentity::SysvAmd64Lp64 => "x86-64",
            AbiIdentity::Aapcs64Lp64 | AbiIdentity::DarwinArm64 => "armv8-a",
            AbiIdentity::RiscvLp64d => "rv64gc",
        }
    }

    pub const fn normalized_target_abi(&self) -> &'static str {
        match self.target.abi {
            AbiIdentity::SysvAmd64Lp64 | AbiIdentity::Aapcs64Lp64 => "lp64",
            AbiIdentity::RiscvLp64d => "lp64d",
            AbiIdentity::DarwinArm64 => "darwin",
        }
    }

    /// CCC's enabled targets deliberately use a fixed, compiler-owned CPU
    /// baseline. `generic` names that baseline without importing host features
    /// or relying on another driver's interpretation of a CPU spelling.
    pub const fn normalized_target_cpu(&self) -> &'static str {
        "generic"
    }

    pub fn validate_target_profile_options(&self) -> Result<(), String> {
        if self.target.abi == AbiIdentity::DarwinArm64
            && self.relocation_model == RelocationModel::Static
        {
            return Err(
                "Darwin arm64 does not provide a non-PIE executable/object profile".to_owned(),
            );
        }
        if let Some(architecture) = self.target_arch.as_deref()
            && architecture != self.normalized_target_arch()
        {
            return Err(format!(
                "target architecture option `{architecture}` contradicts the enabled `{}` profile `{}`",
                self.target.abi.name(),
                self.normalized_target_arch()
            ));
        }
        if let Some(cpu) = self.target_cpu.as_deref()
            && cpu != self.normalized_target_cpu()
        {
            return Err(format!(
                "target CPU option `{cpu}` contradicts the enabled `{}` profile `{}`",
                self.target.abi.name(),
                self.normalized_target_cpu()
            ));
        }
        if let Some(abi) = self.target_abi.as_deref()
            && abi != self.normalized_target_abi()
        {
            return Err(format!(
                "target ABI option `{abi}` contradicts the enabled `{}` profile `{}`",
                self.target.abi.name(),
                self.normalized_target_abi()
            ));
        }
        Ok(())
    }

    pub fn with_sdk_root(mut self, sdk_root: impl Into<PathBuf>) -> Self {
        self.sdk_root = Some(sdk_root.into());
        self
    }

    pub fn with_deployment_target(mut self, deployment_target: impl Into<String>) -> Self {
        self.deployment_target = Some(deployment_target.into());
        self
    }

    pub fn normalized_deployment_target(&self) -> Option<&str> {
        (self.target.abi == AbiIdentity::DarwinArm64).then(|| {
            self.deployment_target
                .as_deref()
                .unwrap_or(DARWIN_ARM64_MINIMUM_DEPLOYMENT_TARGET)
        })
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
        if self.target.abi == AbiIdentity::RiscvLp64d {
            match self.relocation_model {
                RelocationModel::Static => {
                    macros.insert("__riscv_cmodel_medlow".to_owned(), "1".to_owned());
                }
                RelocationModel::Pic | RelocationModel::Pie => {
                    macros.insert("__riscv_cmodel_medany".to_owned(), "1".to_owned());
                    macros.insert("__riscv_cmodel_pic".to_owned(), "1".to_owned());
                }
            }
        }
        if let Some(version) = self.normalized_deployment_target()
            && let Some(encoded) = encode_apple_deployment_version(version)
        {
            macros.insert(
                "__ENVIRONMENT_MAC_OS_X_VERSION_MIN_REQUIRED__".to_owned(),
                encoded.clone(),
            );
            macros.insert(
                "__ENVIRONMENT_OS_VERSION_MIN_REQUIRED__".to_owned(),
                encoded,
            );
        }
        macros
    }
}

fn encode_apple_deployment_version(version: &str) -> Option<String> {
    let mut components = version.split('.');
    let major = components.next()?.parse::<u32>().ok()?;
    let minor = components.next().unwrap_or("0").parse::<u32>().ok()?;
    let patch = components.next().unwrap_or("0").parse::<u32>().ok()?;
    if components.next().is_some() || minor > 99 || patch > 99 {
        return None;
    }
    Some((major * 10_000 + minor * 100 + patch).to_string())
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
    abi: AbiIdentity::SysvAmd64Lp64,
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

/// The enabled AArch64 Linux GNU target using the base AAPCS64 data model.
pub const AARCH64_UNKNOWN_LINUX_GNU: TargetSpec = TargetSpec {
    triple: Triple {
        architecture: Architecture::Aarch64(Aarch64Architecture::Aarch64),
        vendor: Vendor::Unknown,
        operating_system: OperatingSystem::Linux,
        environment: Environment::Gnu,
        binary_format: BinaryFormat::Elf,
    },
    abi: AbiIdentity::Aapcs64Lp64,
    int_align: 4,
    data_layout: TargetDataLayout {
        char_is_signed: false,
        wchar_is_signed: false,
        ..LINUX_LP64_BINARY128_LAYOUT
    },
};

/// The enabled RISC-V 64-bit Linux GNU LP64D target.
pub const RISCV64_UNKNOWN_LINUX_GNU: TargetSpec = TargetSpec {
    triple: Triple {
        architecture: Architecture::Riscv64(Riscv64Architecture::Riscv64),
        vendor: Vendor::Unknown,
        operating_system: OperatingSystem::Linux,
        environment: Environment::Gnu,
        binary_format: BinaryFormat::Elf,
    },
    abi: AbiIdentity::RiscvLp64d,
    int_align: 4,
    data_layout: TargetDataLayout {
        char_is_signed: false,
        ..LINUX_LP64_BINARY128_LAYOUT
    },
};

/// The enabled arm64 Darwin target.
pub const AARCH64_APPLE_DARWIN: TargetSpec = TargetSpec {
    triple: Triple {
        architecture: Architecture::Aarch64(Aarch64Architecture::Aarch64),
        vendor: Vendor::Apple,
        operating_system: OperatingSystem::Darwin(None),
        environment: Environment::Unknown,
        binary_format: BinaryFormat::Macho,
    },
    abi: AbiIdentity::DarwinArm64,
    int_align: 4,
    data_layout: TargetDataLayout {
        char_is_signed: true,
        long_double_width: 64,
        long_double_align: 8,
        wint_is_signed: true,
        ..LINUX_LP64_BINARY128_LAYOUT
    },
};

const LINUX_LP64_BINARY128_LAYOUT: TargetDataLayout = TargetDataLayout {
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
        assert_eq!(config.target.abi, AbiIdentity::SysvAmd64Lp64);
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
        assert_eq!(config.relocation_model, RelocationModel::Pie);
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
    fn enabled_profiles_have_distinct_abi_identities_and_formats() {
        for (spelling, abi, format, long_double_width) in [
            (
                "x86_64-unknown-linux-gnu",
                AbiIdentity::SysvAmd64Lp64,
                BinaryFormat::Elf,
                128,
            ),
            (
                "aarch64-unknown-linux-gnu",
                AbiIdentity::Aapcs64Lp64,
                BinaryFormat::Elf,
                128,
            ),
            (
                "riscv64-unknown-linux-gnu",
                AbiIdentity::RiscvLp64d,
                BinaryFormat::Elf,
                128,
            ),
            (
                "aarch64-apple-darwin",
                AbiIdentity::DarwinArm64,
                BinaryFormat::Macho,
                64,
            ),
        ] {
            let triple = spelling.parse().unwrap();
            let config = EffectiveCompilationConfig::for_target(triple).unwrap();
            assert_eq!(config.target.abi, abi);
            assert_eq!(config.target.triple.binary_format, format);
            assert_eq!(
                config.target.data_layout.long_double_width,
                long_double_width
            );
        }
    }

    #[test]
    fn unsupported_hosts_do_not_fall_back_to_x86_64() {
        for spelling in [
            "wasm32-unknown-unknown",
            "x86_64-unknown-linux-musl",
            "aarch64-unknown-freebsd",
        ] {
            let error =
                EffectiveCompilationConfig::for_host_triple(spelling.parse().unwrap()).unwrap_err();
            assert!(
                error.contains("native host target")
                    && error.contains("not an enabled CCC target profile"),
                "unexpected diagnostic for {spelling}: {error}"
            );
        }
    }

    #[test]
    fn enabled_host_families_select_canonical_profiles() {
        for (host, expected) in [
            ("x86_64-unknown-linux-gnu", "x86_64-unknown-linux-gnu"),
            ("aarch64-unknown-linux-gnu", "aarch64-unknown-linux-gnu"),
            ("riscv64-unknown-linux-gnu", "riscv64-unknown-linux-gnu"),
            ("aarch64-apple-darwin25.5.0", "aarch64-apple-darwin"),
            ("aarch64-apple-macosx15.0.0", "aarch64-apple-darwin"),
        ] {
            let config =
                EffectiveCompilationConfig::for_host_triple(host.parse().unwrap()).unwrap();
            assert_eq!(config.target.triple.to_string(), expected, "{host}");
        }
    }

    #[test]
    fn near_miss_target_triples_do_not_enter_enabled_profiles() {
        for spelling in [
            "x86_64-pc-linux-gnu",
            "riscv64-apple-linux-gnu",
            "aarch64-unknown-darwin",
            "aarch64-apple-macosx14.0.0",
        ] {
            let error =
                EffectiveCompilationConfig::for_target(spelling.parse().unwrap()).unwrap_err();
            assert!(
                error.contains("not an enabled CCC target profile"),
                "unexpected diagnostic for {spelling}: {error}"
            );
        }
    }

    #[test]
    fn near_match_triples_do_not_inherit_an_incompatible_fixed_profile() {
        for triple in [
            "riscv64imac-unknown-linux-gnu",
            "riscv64a23-unknown-linux-gnu",
            "aarch64-apple-ios",
            "aarch64-apple-tvos",
            "aarch64-apple-watchos",
            "aarch64-apple-visionos",
        ] {
            let triple: Triple = triple.parse().unwrap();
            assert!(
                EffectiveCompilationConfig::for_target(triple.clone()).is_err(),
                "{triple}"
            );
        }
    }

    #[test]
    fn target_profile_options_are_exact_and_cannot_be_silently_discarded() {
        for (config, architecture, abi) in [
            (
                EffectiveCompilationConfig::x86_64_unknown_linux_gnu(),
                "x86-64",
                "lp64",
            ),
            (
                EffectiveCompilationConfig::aarch64_unknown_linux_gnu(),
                "armv8-a",
                "lp64",
            ),
            (
                EffectiveCompilationConfig::riscv64_unknown_linux_gnu(),
                "rv64gc",
                "lp64d",
            ),
            (
                EffectiveCompilationConfig::aarch64_apple_darwin(),
                "armv8-a",
                "darwin",
            ),
        ] {
            assert_eq!(config.normalized_target_arch(), architecture);
            assert_eq!(config.normalized_target_abi(), abi);
            assert_eq!(config.normalized_target_cpu(), "generic");
            config
                .clone()
                .with_target_arch(architecture)
                .with_target_cpu("generic")
                .with_target_abi(abi)
                .validate_target_profile_options()
                .unwrap();
            assert!(
                config
                    .clone()
                    .with_target_arch("contradictory-architecture")
                    .validate_target_profile_options()
                    .is_err()
            );
            assert!(
                config
                    .clone()
                    .with_target_cpu("native")
                    .validate_target_profile_options()
                    .is_err()
            );
            assert!(
                config
                    .with_target_abi("contradictory-abi")
                    .validate_target_profile_options()
                    .is_err()
            );
        }
    }

    #[test]
    fn architecture_macros_follow_the_selected_profile() {
        let x86 = EffectiveCompilationConfig::x86_64_unknown_linux_gnu();
        assert_eq!(x86.target_macros.get("__LDBL_MANT_DIG__"), Some("64"));
        assert_eq!(x86.target_macros.get("__LDBL_DECIMAL_DIG__"), Some("21"));
        assert_eq!(x86.target_macros.get("__LONG_DOUBLE_128__"), None);

        let aarch64 = EffectiveCompilationConfig::aarch64_unknown_linux_gnu();
        assert_eq!(aarch64.target_macros.get("__aarch64__"), Some("1"));
        assert_eq!(aarch64.target_macros.get("__ARM_ARCH"), Some("8"));
        assert_eq!(aarch64.target_macros.get("__ARM_ARCH_8A"), Some("1"));
        assert_eq!(aarch64.target_macros.get("__ARM64_ARCH_8__"), None);
        assert_eq!(aarch64.target_macros.get("__ARM_ARCH_PROFILE"), Some("65"));
        assert_eq!(aarch64.target_macros.get("__ARM_FP"), Some("14"));
        assert_eq!(
            aarch64.target_macros.get("__ARM_ALIGN_MAX_STACK_PWR"),
            Some("16")
        );
        assert_eq!(
            aarch64.target_macros.get("__BIGGEST_ALIGNMENT__"),
            Some("16")
        );
        assert_eq!(aarch64.target_macros.get("__ARM_PCS_AAPCS64"), Some("1"));
        assert_eq!(aarch64.target_macros.get("__LONG_DOUBLE_128__"), Some("1"));
        assert_eq!(aarch64.target_macros.get("__LDBL_MANT_DIG__"), Some("113"));
        assert_eq!(aarch64.target_macros.get("__CHAR_UNSIGNED__"), Some("1"));
        for unsupported_surface in [
            "__ARM_NEON",
            "__ARM_NEON__",
            "__ARM_FP16_ARGS",
            "__ARM_FP16_FORMAT_IEEE",
            "__ARM_FEATURE_CLZ",
        ] {
            assert_eq!(aarch64.target_macros.get(unsupported_surface), None);
        }

        let riscv = EffectiveCompilationConfig::riscv64_unknown_linux_gnu();
        assert_eq!(riscv.target_macros.get("__riscv_xlen"), Some("64"));
        assert_eq!(
            riscv.target_macros.get("__riscv_float_abi_double"),
            Some("1")
        );
        assert_eq!(riscv.target_macros.get("__riscv_m"), Some("2000000"));
        assert_eq!(riscv.target_macros.get("__riscv_zicsr"), Some("2000000"));
        assert_eq!(riscv.target_macros.get("__riscv_zifencei"), Some("2000000"));
        assert_eq!(riscv.target_macros.get("__riscv_atomic"), Some("1"));
        assert_eq!(riscv.target_macros.get("__LONG_DOUBLE_128__"), Some("1"));
        assert_eq!(riscv.target_macros.get("__LDBL_MANT_DIG__"), Some("113"));
        let pie = riscv.frontend_predefined_macros();
        assert_eq!(
            pie.get("__riscv_cmodel_medany").map(String::as_str),
            Some("1")
        );
        assert_eq!(pie.get("__riscv_cmodel_pic").map(String::as_str), Some("1"));
        assert_eq!(pie.get("__riscv_cmodel_medlow"), None);
        let mut static_riscv = riscv.clone();
        static_riscv.relocation_model = RelocationModel::Static;
        let static_macros = static_riscv.frontend_predefined_macros();
        assert_eq!(
            static_macros
                .get("__riscv_cmodel_medlow")
                .map(String::as_str),
            Some("1")
        );
        assert_eq!(static_macros.get("__riscv_cmodel_medany"), None);
        assert_eq!(static_macros.get("__riscv_cmodel_pic"), None);

        let darwin =
            EffectiveCompilationConfig::aarch64_apple_darwin().with_deployment_target("14.2.1");
        let macros = darwin.frontend_predefined_macros();
        assert_eq!(macros.get("__APPLE__").map(String::as_str), Some("1"));
        assert_eq!(macros.get("__APPLE_CC__").map(String::as_str), Some("6000"));
        assert_eq!(macros.get("__arm64__").map(String::as_str), Some("1"));
        assert_eq!(
            macros
                .get("__ENVIRONMENT_MAC_OS_X_VERSION_MIN_REQUIRED__")
                .map(String::as_str),
            Some("140201")
        );
        assert_eq!(darwin.target_macros.get("__USER_LABEL_PREFIX__"), Some("_"));
        assert_eq!(darwin.target_macros.get("__ARM_ARCH_8A"), None);
        assert_eq!(darwin.target_macros.get("__ARM64_ARCH_8__"), Some("1"));
        assert_eq!(darwin.target_macros.get("__ARM_ARCH_PROFILE"), Some("'A'"));
        assert_eq!(darwin.target_macros.get("__ARM_FP"), Some("0xE"));
        assert_eq!(
            darwin.target_macros.get("__ARM_ALIGN_MAX_STACK_PWR"),
            Some("4")
        );
        assert_eq!(darwin.target_macros.get("__BIGGEST_ALIGNMENT__"), Some("8"));
        assert_eq!(darwin.target_macros.get("__LDBL_MANT_DIG__"), Some("53"));
        assert_eq!(darwin.target_macros.get("__LONG_DOUBLE_128__"), None);
        for config in [x86, aarch64, riscv, darwin] {
            assert_eq!(
                config.frontend_predefined_macros().get("__SIZEOF_INT128__"),
                None,
                "{} must not advertise incomplete 128-bit value support",
                config.target.triple
            );
        }
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
            ("__FLT_MANT_DIG__", "24"),
            ("__FLT_DIG__", "6"),
            ("__FLT_MIN_EXP__", "(-125)"),
            ("__FLT_MIN_10_EXP__", "(-37)"),
            ("__FLT_MAX_EXP__", "128"),
            ("__FLT_MAX_10_EXP__", "38"),
            ("__FLT_DECIMAL_DIG__", "9"),
            ("__FLT_HAS_DENORM__", "1"),
            ("__FLT_HAS_INFINITY__", "1"),
            ("__FLT_HAS_QUIET_NAN__", "1"),
            ("__FLT_MAX__", "0x1.fffffep+127F"),
            ("__FLT_NORM_MAX__", "0x1.fffffep+127F"),
            ("__FLT_EPSILON__", "0x1p-23F"),
            ("__FLT_MIN__", "0x1p-126F"),
            ("__FLT_DENORM_MIN__", "0x1p-149F"),
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
    fn layout_only_float16_support_does_not_advertise_value_capabilities() {
        for config in [
            EffectiveCompilationConfig::default(),
            EffectiveCompilationConfig::aarch64_unknown_linux_gnu(),
            EffectiveCompilationConfig::riscv64_unknown_linux_gnu(),
            EffectiveCompilationConfig::aarch64_apple_darwin(),
        ] {
            assert!(
                config
                    .target_macros
                    .iter()
                    .all(|(name, _)| !name.starts_with("__FLT16_")),
                "{} advertises an unsupported `_Float16` value capability",
                config.target.triple
            );
        }
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
