use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use ccc_target::ENABLED_TARGET_SPECS;

mod support;

static TEST_ID: AtomicU64 = AtomicU64::new(0);

struct TestDirectory {
    path: PathBuf,
}

impl TestDirectory {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "ccc-header-parsing-test-{}-{}-{name}",
            std::process::id(),
            TEST_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn write(&self, relative: &str, contents: &str) -> PathBuf {
        let path = self.path.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, contents).unwrap();
        path
    }

    #[cfg(any(
        all(target_arch = "x86_64", target_os = "linux"),
        all(target_arch = "aarch64", target_os = "linux"),
        all(target_arch = "riscv64", target_os = "linux")
    ))]
    fn command(&self) -> Command {
        self.command_for_target(support::native_linux_target_triple())
    }

    fn command_for_target(&self, target: &str) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_ccc"));
        command
            .current_dir(&self.path)
            .arg(format!("--target={target}"))
            .env("LC_ALL", "C")
            .env("LANG", "C");
        command
    }
}

#[cfg(all(target_arch = "aarch64", target_os = "macos"))]
fn macos_sdk_root() -> String {
    let output = Command::new("xcrun")
        .args(["--sdk", "macosx", "--show-sdk-path"])
        .output()
        .expect("the Darwin hosted-header gate requires xcrun");
    assert!(
        output.status.success(),
        "xcrun could not locate the macOS SDK"
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn repository(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative)
}

fn assert_success(output: &Output, context: &str) {
    assert!(
        output.status.success(),
        "{context}\nstatus: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "{context} wrote stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_ast_lines(output: &Output, expected: &[&str]) {
    let stdout = String::from_utf8(output.stdout.clone()).unwrap();
    let lines = stdout.lines().map(str::trim).collect::<Vec<_>>();
    for expected in expected {
        assert!(
            lines.contains(expected),
            "AST dump is missing exact line {expected:?}:\n{stdout}"
        );
    }
}

#[test]
fn curated_hosted_declarations_reach_the_ast_intact() {
    let include = repository("test-corpus/libc-headers/glibc-like");
    let source = include.join("probe.c");
    let directory = TestDirectory::new("curated");
    for profile in ENABLED_TARGET_SPECS {
        let target = profile.triple.to_string();
        let output = directory
            .command_for_target(&target)
            .args(["--dump-ast", "-nostdinc", "-isystem"])
            .arg(&include)
            .arg(&source)
            .output()
            .unwrap();
        assert_success(
            &output,
            &format!("curated hosted-header parsing failed for {target}"),
        );
        assert_ast_lines(
            &output,
            &[
                "declarator fixture_record_t",
                "declarator fixture_read(3)",
                "attribute __attribute__ __nothrow__",
                "asm-label __asm__ \"fixture_read_impl\"",
                "function-definition fixture_identity",
                "declarator hosted_header_preprocessing_sentinel",
            ],
        );
    }
}

#[cfg(all(target_arch = "aarch64", target_os = "macos"))]
#[test]
fn apple_math_private_classification_helpers_reach_the_ast_without_public_replacement() {
    let directory = TestDirectory::new("apple-math-private-helpers");
    let source = directory.write(
        "apple-math-ast.c",
        "#include <math.h>\n\
         long double ccc_public_fabsl(long double value) { return fabsl(value); }\n\
         int ccc_public_isfinite(double value) { return isfinite(value); }\n",
    );
    let sdk = macos_sdk_root();
    let output = Command::new(env!("CARGO_BIN_EXE_ccc"))
        .current_dir(&directory.path)
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .args(["--target=aarch64-apple-darwin", "--sdk-root"])
        .arg(sdk)
        .arg("--dump-ast")
        .arg(source)
        .output()
        .unwrap();
    assert_success(&output, "Apple math hosted-header parsing failed");
    assert_ast_lines(
        &output,
        &[
            "function-definition __inline_isfinitef",
            "function-definition __inline_isfinitel",
            "function-definition ccc_public_fabsl",
            "function-definition ccc_public_isfinite",
        ],
    );
}

#[cfg(any(
    all(target_arch = "x86_64", target_os = "linux"),
    all(target_arch = "aarch64", target_os = "linux"),
    all(target_arch = "riscv64", target_os = "linux")
))]
#[test]
fn installed_target_glibc_declarations_reach_the_ast_intact() {
    let identity = support::installed_glibc_identity();
    eprintln!("installed-header parser gate: {identity}");
    let directory = TestDirectory::new("installed-glibc");
    let source = directory.write(
        "installed.c",
        concat!(
            "#define _GNU_SOURCE 1\n",
            "#include <features.h>\n",
            "#include <stddef.h>\n",
            "#include <stdint.h>\n",
            "#include <sys/types.h>\n",
            "#include <unistd.h>\n",
            "#include <string.h>\n",
            "__extension__ typedef __typeof__(sizeof(0)) installed_typeof_sentinel_t;\n",
            "typedef __signed__ int *installed_pointer_sentinel_t;\n",
            "__restrict__ installed_pointer_sentinel_t installed_restrict_sentinel;\n",
            "extern __signed__ int installed_asm_sentinel(\n",
            "    __const__ char *__restrict__ value)\n",
            "    __asm__(\"installed_asm_target\") __attribute__((__nothrow__));\n",
            "static __inline__ __signed__ int installed_inline_sentinel(\n",
            "    __const__ __signed__ int *__restrict__ value) { return *value; }\n",
            "int installed_glibc_parse_sentinel;\n",
        ),
    );
    let output = directory
        .command()
        .arg("--dump-ast")
        .arg(source)
        .output()
        .unwrap();
    assert_success(
        &output,
        &format!("installed hosted-header parsing failed for {identity}"),
    );
    assert_ast_lines(
        &output,
        &[
            "declarator size_t",
            "declarator ssize_t",
            "declarator read(3)",
            "declarator *memcpy(3)",
            "declarator installed_typeof_sentinel_t",
            "declarator installed_restrict_sentinel",
            "declarator installed_asm_sentinel(1)",
            "asm-label __asm__ \"installed_asm_target\"",
            "attribute __attribute__ __nothrow__",
            "function-definition installed_inline_sentinel",
            "declarator installed_glibc_parse_sentinel",
            "extension",
            "type Typeof",
            "qualifier Restrict",
            "function-specifier Inline",
        ],
    );
}

#[cfg(any(
    all(target_arch = "x86_64", target_os = "linux"),
    all(target_arch = "aarch64", target_os = "linux"),
    all(target_arch = "riscv64", target_os = "linux")
))]
#[test]
fn installed_target_glibc_declarations_compile_link_and_execute() {
    let identity = support::installed_glibc_identity();
    eprintln!("installed-header code-generation gate: {identity}");
    let directory = TestDirectory::new("installed-glibc-codegen");
    let source = directory.write(
        "installed-codegen.c",
        concat!(
            "#define _GNU_SOURCE 1\n",
            "#include <pthread.h>\n",
            "#include <stdint.h>\n",
            "#include <stdlib.h>\n",
            "#include <string.h>\n",
            "int main(void) {\n",
            "    char destination[4];\n",
            "    void *copied = memcpy(destination, \"ok\", 3);\n",
            "    return copied != destination || strcmp(destination, \"ok\") != 0;\n",
            "}\n",
        ),
    );
    let executable = directory.path.join("installed-codegen");
    let output = directory
        .command()
        .arg(source)
        .args(["-o"])
        .arg(&executable)
        .output()
        .unwrap();
    assert_success(
        &output,
        &format!("installed hosted-header code generation failed for {identity}"),
    );

    let output = Command::new(&executable).output().unwrap();
    assert_success(
        &output,
        &format!("installed hosted-header executable failed for {identity}"),
    );
}
