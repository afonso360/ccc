#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Output};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_WORKSPACE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
pub struct TestWorkspace {
    path: PathBuf,
    retain_on_failure: bool,
}

impl TestWorkspace {
    pub fn new(suite: &str, name: &str) -> Self {
        let suite = path_component(suite);
        let name = path_component(name);
        let path = loop {
            let serial = NEXT_WORKSPACE.fetch_add(1, Ordering::Relaxed);
            let candidate = std::env::temp_dir().join(format!(
                "ccc-{suite}-{}-{serial}-{name}",
                std::process::id()
            ));
            match fs::create_dir(&candidate) {
                Ok(()) => break candidate,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    panic!(
                        "failed to create test workspace `{}`: {error}",
                        candidate.display()
                    )
                }
            }
        };
        Self {
            path,
            retain_on_failure: false,
        }
    }

    #[must_use]
    pub fn retain_on_failure(mut self) -> Self {
        self.retain_on_failure = true;
        self
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn join(&self, path: impl AsRef<Path>) -> PathBuf {
        self.path.join(path)
    }

    pub fn write(&self, path: impl AsRef<Path>, contents: impl AsRef<[u8]>) -> PathBuf {
        let path = self.join(path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap_or_else(|error| {
                panic!(
                    "failed to create test workspace directory `{}`: {error}",
                    parent.display()
                )
            });
        }
        fs::write(&path, contents).unwrap_or_else(|error| {
            panic!(
                "failed to write test workspace file `{}`: {error}",
                path.display()
            )
        });
        path
    }

    #[track_caller]
    pub fn assert_command_success(&self, context: &str, output: &Output) {
        assert_command_status(context, output, true, Some(self.path()));
    }

    #[track_caller]
    pub fn assert_command_failure(&self, context: &str, output: &Output) {
        assert_command_status(context, output, false, Some(self.path()));
    }
}

impl AsRef<Path> for TestWorkspace {
    fn as_ref(&self) -> &Path {
        self.path()
    }
}

impl Drop for TestWorkspace {
    fn drop(&mut self) {
        if self.retain_on_failure && std::thread::panicking() {
            eprintln!(
                "retained failed test workspace at `{}`",
                self.path.display()
            );
            return;
        }
        match fs::remove_dir_all(&self.path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => eprintln!(
                "failed to remove test workspace `{}`: {error}",
                self.path.display()
            ),
        }
    }
}

#[track_caller]
pub fn assert_command_success(context: &str, output: &Output) {
    assert_command_status(context, output, true, None);
}

#[track_caller]
pub fn assert_command_failure(context: &str, output: &Output) {
    assert_command_status(context, output, false, None);
}

#[track_caller]
pub fn assert_command_text_success(
    context: &str,
    status: &ExitStatus,
    stdout: &str,
    stderr: &str,
    workspace: Option<&Path>,
) {
    assert_command_status_parts(
        context,
        status,
        stdout.as_bytes(),
        stderr.as_bytes(),
        true,
        workspace,
    );
}

#[track_caller]
pub fn assert_command_text_failure(
    context: &str,
    status: &ExitStatus,
    stdout: &str,
    stderr: &str,
    workspace: Option<&Path>,
) {
    assert_command_status_parts(
        context,
        status,
        stdout.as_bytes(),
        stderr.as_bytes(),
        false,
        workspace,
    );
}

#[track_caller]
fn assert_command_status(
    context: &str,
    output: &Output,
    expected_success: bool,
    workspace: Option<&Path>,
) {
    assert_command_status_parts(
        context,
        &output.status,
        &output.stdout,
        &output.stderr,
        expected_success,
        workspace,
    );
}

#[track_caller]
fn assert_command_status_parts(
    context: &str,
    status: &ExitStatus,
    stdout: &[u8],
    stderr: &[u8],
    expected_success: bool,
    workspace: Option<&Path>,
) {
    let expectation = if expected_success { "succeed" } else { "fail" };
    assert!(
        status.success() == expected_success,
        "{context} was expected to {expectation}, but exited with {}{}\
         \nstdout:\n{}\
         \nstderr:\n{}",
        status,
        workspace
            .map(|path| format!("\nworkspace: {}", path.display()))
            .unwrap_or_default(),
        String::from_utf8_lossy(stdout),
        String::from_utf8_lossy(stderr)
    );
}

fn path_component(value: &str) -> String {
    let component = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    if component.is_empty() {
        "test".to_owned()
    } else {
        component
    }
}

#[cfg(any(
    all(target_arch = "x86_64", target_os = "linux"),
    all(target_arch = "aarch64", target_os = "linux"),
    all(target_arch = "riscv64", target_os = "linux")
))]
pub const fn native_linux_target_triple() -> &'static str {
    if cfg!(target_arch = "x86_64") {
        "x86_64-unknown-linux-gnu"
    } else if cfg!(target_arch = "aarch64") {
        "aarch64-unknown-linux-gnu"
    } else {
        "riscv64-unknown-linux-gnu"
    }
}

#[cfg(any(
    all(target_arch = "x86_64", target_os = "linux"),
    all(target_arch = "aarch64", target_os = "linux"),
    all(target_arch = "riscv64", target_os = "linux")
))]
pub fn target_compiler_command() -> std::process::Command {
    let value = std::env::var_os("CCC_CC")
        .or_else(|| std::env::var_os("CC"))
        .unwrap_or_else(|| "cc".into());
    let mut words = value
        .to_string_lossy()
        .split_whitespace()
        .map(std::ffi::OsString::from)
        .collect::<Vec<_>>();
    assert!(!words.is_empty(), "CCC_CC/CC must name a compiler driver");
    let mut command = std::process::Command::new(words.remove(0));
    command.args(words);
    command
}

#[cfg(any(
    all(target_arch = "x86_64", target_os = "linux"),
    all(target_arch = "aarch64", target_os = "linux"),
    all(target_arch = "riscv64", target_os = "linux")
))]
pub fn assert_native_linux_compiler_target(mut command: std::process::Command) -> String {
    let output = command
        .arg("-dumpmachine")
        .output()
        .expect("the target compiler must support -dumpmachine");
    assert!(
        output.status.success(),
        "target compiler -dumpmachine failed with {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let target = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let expected_architecture = native_linux_target_triple().split_once('-').unwrap().0;
    assert!(
        target == expected_architecture || target.starts_with(&format!("{expected_architecture}-")),
        "CCC_CC/CC targets `{target}` rather than `{}`",
        native_linux_target_triple()
    );
    target
}

#[cfg(any(
    all(target_arch = "x86_64", target_os = "linux"),
    all(target_arch = "aarch64", target_os = "linux"),
    all(target_arch = "riscv64", target_os = "linux")
))]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstalledGlibcIdentity {
    pub compiler: String,
    pub target: String,
    pub libc: String,
}

#[cfg(any(
    all(target_arch = "x86_64", target_os = "linux"),
    all(target_arch = "aarch64", target_os = "linux"),
    all(target_arch = "riscv64", target_os = "linux")
))]
impl std::fmt::Display for InstalledGlibcIdentity {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "compiler={}; target={}; libc={}",
            self.compiler, self.target, self.libc
        )
    }
}

#[cfg(any(
    all(target_arch = "x86_64", target_os = "linux"),
    all(target_arch = "aarch64", target_os = "linux"),
    all(target_arch = "riscv64", target_os = "linux")
))]
pub fn installed_glibc_identity() -> InstalledGlibcIdentity {
    let compiler = target_compiler_command()
        .arg("--version")
        .output()
        .expect("the hosted-header gate requires CCC_CC/CC");
    assert!(
        compiler.status.success(),
        "CCC_CC/CC --version failed with {}\nstdout:\n{}\nstderr:\n{}",
        compiler.status,
        String::from_utf8_lossy(&compiler.stdout),
        String::from_utf8_lossy(&compiler.stderr)
    );
    let target = assert_native_linux_compiler_target(target_compiler_command());

    let libc = target_compiler_command()
        .args([
            "-dM",
            "-E",
            "-include",
            "features.h",
            "-x",
            "c",
            "/dev/null",
        ])
        .output()
        .expect("the hosted-header gate requires target glibc headers");
    assert!(
        libc.status.success(),
        "CCC_CC/CC could not preprocess target features.h with {}\nstdout:\n{}\nstderr:\n{}",
        libc.status,
        String::from_utf8_lossy(&libc.stdout),
        String::from_utf8_lossy(&libc.stderr)
    );
    let libc = String::from_utf8_lossy(&libc.stdout);
    let glibc_major = libc
        .lines()
        .find_map(|line| line.strip_prefix("#define __GLIBC__ "))
        .expect("target features.h did not define __GLIBC__");
    let glibc_minor = libc
        .lines()
        .find_map(|line| line.strip_prefix("#define __GLIBC_MINOR__ "))
        .expect("target features.h did not define __GLIBC_MINOR__");

    InstalledGlibcIdentity {
        compiler: String::from_utf8_lossy(&compiler.stdout)
            .lines()
            .next()
            .unwrap_or("unknown compiler")
            .to_owned(),
        target,
        libc: format!("glibc {glibc_major}.{glibc_minor}"),
    }
}
