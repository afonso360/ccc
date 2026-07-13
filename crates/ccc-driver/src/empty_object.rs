//! Relocatable empty-object emission.

use std::fs;
use std::io;
use std::path::Path;

use ccc_target::{ObjectFormat, TargetSpec};
use object::write::Object;
use object::{Architecture, BinaryFormat, Endianness};

/// Emits an empty ELF64 relocatable object using the `object` writer.
pub fn write_empty_elf64_relocatable(path: &Path, target: TargetSpec) -> io::Result<()> {
    let architecture = match (target.object_format, target.elf_machine) {
        (ObjectFormat::Elf, 62) => Architecture::X86_64,
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!("unsupported object target `{}`", target.triple),
            ));
        }
    };

    let object = Object::new(BinaryFormat::Elf, architecture, Endianness::Little);
    let bytes = object.write().map_err(io::Error::other)?;
    fs::write(path, bytes)
}

/// Validates that bytes describe an empty x86-64 ELF relocatable object.
pub fn is_empty_elf64_relocatable(bytes: &[u8]) -> bool {
    use object::read::{File, Object as _, ObjectSection as _};

    let Ok(object) = File::parse(bytes) else {
        return false;
    };

    object.format() == BinaryFormat::Elf
        && object.architecture() == Architecture::X86_64
        && object.entry() == 0
        && object.symbols().next().is_none()
        && object.sections().all(|section| {
            !matches!(
                section.name(),
                Ok(".text") | Ok(".data") | Ok(".bss") | Ok(".rodata")
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ccc_target::X86_64_UNKNOWN_LINUX_GNU;

    #[test]
    fn writes_a_structurally_valid_empty_object() {
        let path = std::env::temp_dir().join(format!("ccc-empty-object-{}.o", std::process::id()));
        write_empty_elf64_relocatable(&path, X86_64_UNKNOWN_LINUX_GNU).unwrap();
        let bytes = fs::read(&path).unwrap();
        assert!(is_empty_elf64_relocatable(&bytes));
        fs::remove_file(path).unwrap();
    }
}
