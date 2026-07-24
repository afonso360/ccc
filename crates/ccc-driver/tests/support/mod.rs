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
