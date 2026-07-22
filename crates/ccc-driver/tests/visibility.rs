#![cfg(all(
    target_os = "linux",
    any(
        target_arch = "x86_64",
        target_arch = "aarch64",
        target_arch = "riscv64"
    )
))]

use std::fs;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use object::read::{Object as _, ObjectSymbol as _};

static TEST_ID: AtomicU64 = AtomicU64::new(0);

#[test]
fn elf_visibility_is_exact_for_native_variadic_and_undefined_functions() {
    let directory = std::env::temp_dir().join(format!(
        "ccc-visibility-test-{}-{}",
        std::process::id(),
        TEST_ID.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&directory).unwrap();
    let source = directory.join("visibility.c");
    let output = directory.join("visibility.o");
    fs::write(
        &source,
        r#"
#define VISIBILITY(value) __attribute__((visibility(value)))

int native_default(void) VISIBILITY("default");
int native_default(void) { return 1; }
int native_hidden(void) VISIBILITY("hidden");
int native_hidden(void) { return 2; }
int native_protected(void) VISIBILITY("protected");
int native_protected(void) { return 3; }
int native_internal(void) VISIBILITY("internal");
int native_internal(void) { return 4; }

int variadic_default(int marker, ...) VISIBILITY("default");
int variadic_default(int marker, ...) { return marker; }
int variadic_hidden(int marker, ...) VISIBILITY("hidden");
int variadic_hidden(int marker, ...) { return marker; }
int variadic_protected(int marker, ...) VISIBILITY("protected");
int variadic_protected(int marker, ...) { return marker; }
int variadic_internal(int marker, ...) VISIBILITY("internal");
int variadic_internal(int marker, ...) { return marker; }

int undefined_default(void) VISIBILITY("default");
int undefined_hidden(void) VISIBILITY("hidden");
int undefined_protected(void) VISIBILITY("protected");
int undefined_internal(void) VISIBILITY("internal");

int retain_undefined_relocations(void) {
    return undefined_default() + undefined_hidden()
        + undefined_protected() + undefined_internal();
}
"#,
    )
    .unwrap();

    let compilation = Command::new(env!("CARGO_BIN_EXE_ccc"))
        .arg("-nostdinc")
        .arg("-c")
        .arg(&source)
        .arg("-o")
        .arg(&output)
        .output()
        .unwrap();
    assert!(
        compilation.status.success(),
        "ccc failed: {}",
        String::from_utf8_lossy(&compilation.stderr)
    );

    let bytes = fs::read(&output).unwrap();
    let object = object::File::parse(bytes.as_slice()).unwrap();
    for prefix in ["native", "variadic", "undefined"] {
        for (suffix, expected) in [
            ("default", object::elf::STV_DEFAULT),
            ("hidden", object::elf::STV_HIDDEN),
            ("protected", object::elf::STV_PROTECTED),
            ("internal", object::elf::STV_INTERNAL),
        ] {
            let name = format!("{prefix}_{suffix}");
            let symbol = object
                .symbols()
                .find(|symbol| symbol.name() == Ok(name.as_str()))
                .unwrap_or_else(|| panic!("missing symbol `{name}`"));
            assert_ne!(
                symbol.scope(),
                object::SymbolScope::Compilation,
                "`{name}` must retain external binding"
            );
            assert_eq!(
                symbol.flags().elf_visibility(),
                Some(expected),
                "wrong ELF visibility for `{name}`"
            );
            assert_eq!(
                symbol.is_undefined(),
                prefix == "undefined",
                "wrong definition state for `{name}`"
            );
        }
    }

    fs::remove_dir_all(directory).unwrap();
}
