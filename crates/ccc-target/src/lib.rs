//! Target defaults used by the driver and later compiler phases.

/// The object format selected by a target.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObjectFormat {
    Elf,
}

/// Immutable defaults for an enabled target.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TargetSpec {
    pub triple: &'static str,
    pub object_format: ObjectFormat,
    pub elf_machine: u16,
}

/// CCC's first enabled target, selected in ADR-0003.
pub const X86_64_UNKNOWN_LINUX_GNU: TargetSpec = TargetSpec {
    triple: "x86_64-unknown-linux-gnu",
    object_format: ObjectFormat::Elf,
    // ELF's EM_X86_64 value.
    elf_machine: 62,
};
