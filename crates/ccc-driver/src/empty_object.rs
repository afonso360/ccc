//! Empty-object validation retained for the execution harness.

/// Validates that bytes describe an empty x86-64 ELF relocatable object.
pub fn is_empty_elf64_relocatable(bytes: &[u8]) -> bool {
    use object::read::{File, Object as _, ObjectSection as _, ObjectSymbol as _};
    use object::{Architecture, BinaryFormat, SymbolKind};

    let Ok(object) = File::parse(bytes) else {
        return false;
    };

    object.format() == BinaryFormat::Elf
        && object.architecture() == Architecture::X86_64
        && object.entry() == 0
        && object
            .symbols()
            .all(|symbol| symbol.kind() == SymbolKind::File)
        && object.sections().all(|section| {
            !matches!(
                section.name(),
                Ok(".text") | Ok(".data") | Ok(".bss") | Ok(".rodata")
            )
        })
}
