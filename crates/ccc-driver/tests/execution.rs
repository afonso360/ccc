use std::fs;
use std::process::Command;

#[test]
fn empty_translation_unit_emits_a_valid_object() {
    let directory = std::env::temp_dir().join(format!(
        "ccc-execution-test-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("unnamed")
    ));
    let _ = fs::remove_dir_all(&directory);
    fs::create_dir_all(&directory).unwrap();

    let input = directory.join("empty.c");
    let output = directory.join("empty.o");
    fs::write(
        &input,
        include_str!("../../../tests/execution/cases/empty.c"),
    )
    .unwrap();

    let result = Command::new(env!("CARGO_BIN_EXE_ccc"))
        .arg("-c")
        .arg(&input)
        .arg("-o")
        .arg(&output)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "ccc failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );

    let object = fs::read(&output).unwrap();
    assert!(ccc_driver::is_empty_elf64_relocatable(&object));
    fs::remove_dir_all(directory).unwrap();
}
