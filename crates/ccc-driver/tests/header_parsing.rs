use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

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

    #[cfg(all(target_arch = "x86_64", target_os = "linux"))]
    fn write(&self, relative: &str, contents: &str) -> PathBuf {
        let path = self.path.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, contents).unwrap();
        path
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_ccc"));
        command
            .current_dir(&self.path)
            .env("LC_ALL", "C")
            .env("LANG", "C");
        command
    }
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
    let output = directory
        .command()
        .args(["--dump-ast", "-nostdinc", "-isystem"])
        .arg(include)
        .arg(source)
        .output()
        .unwrap();
    assert_success(&output, "curated hosted-header parsing failed");
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

#[cfg(all(target_arch = "x86_64", target_os = "linux"))]
fn installed_environment_identity() -> String {
    let features = fs::read_to_string("/usr/include/features.h")
        .expect("the installed-header parser gate requires /usr/include/features.h");
    assert!(
        features.contains("__GLIBC__"),
        "features.h is not from glibc"
    );

    let compiler = Command::new("cc")
        .arg("--version")
        .output()
        .expect("the installed-header parser gate requires cc");
    assert!(compiler.status.success(), "cc --version failed");
    let target = Command::new("cc")
        .arg("-dumpmachine")
        .output()
        .expect("the installed-header parser gate requires cc -dumpmachine");
    assert!(target.status.success(), "cc -dumpmachine failed");
    let libc = Command::new("getconf")
        .arg("GNU_LIBC_VERSION")
        .output()
        .expect("the installed-header parser gate requires getconf");
    assert!(libc.status.success(), "getconf GNU_LIBC_VERSION failed");

    format!(
        "compiler={}; target={}; libc={}",
        String::from_utf8_lossy(&compiler.stdout)
            .lines()
            .next()
            .unwrap_or("unknown compiler"),
        String::from_utf8_lossy(&target.stdout).trim(),
        String::from_utf8_lossy(&libc.stdout).trim()
    )
}

#[cfg(all(target_arch = "x86_64", target_os = "linux"))]
#[test]
fn installed_target_glibc_declarations_reach_the_ast_intact() {
    let identity = installed_environment_identity();
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

#[cfg(all(target_arch = "x86_64", target_os = "linux"))]
#[test]
fn installed_target_glibc_declarations_compile_link_and_execute() {
    let identity = installed_environment_identity();
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
