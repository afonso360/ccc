//! Target defaults shared by every compiler phase.

pub use target_lexicon::{
    Architecture, BinaryFormat, CallingConvention, Environment, OperatingSystem, PointerWidth,
    Triple, Vendor,
};

/// The relocation contract used by generated objects.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RelocationModel {
    Static,
}

/// Immutable defaults for an enabled target.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TargetSpec {
    pub triple: Triple,
    pub int_align: u8,
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
}

/// The effective configuration passed unchanged through the compiler.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EffectiveCompilationConfig {
    pub target: TargetSpec,
    pub relocation_model: RelocationModel,
}

impl EffectiveCompilationConfig {
    pub const fn x86_64_unknown_linux_gnu() -> Self {
        Self {
            target: X86_64_UNKNOWN_LINUX_GNU,
            relocation_model: RelocationModel::Static,
        }
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
        assert_eq!(
            config.target.calling_convention(),
            Some(CallingConvention::SystemV)
        );
    }
}
