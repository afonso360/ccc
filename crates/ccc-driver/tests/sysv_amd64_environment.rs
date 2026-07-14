#[cfg(all(
    feature = "ci-sysv-amd64",
    not(all(target_arch = "x86_64", target_os = "linux"))
))]
compile_error!("ci-sysv-amd64 requires a Linux x86-64 test host");

#[cfg(all(feature = "ci-sysv-amd64", target_arch = "x86_64", target_os = "linux"))]
#[test]
fn required_sysv_amd64_environment_is_complete() {
    use std::process::Command;

    assert_eq!(
        std::env::var("CCC_REQUIRE_SYSV_ABI_TESTS").as_deref(),
        Ok("1"),
        "the required native ABI suite needs CCC_REQUIRE_SYSV_ABI_TESTS=1"
    );
    for tool in [
        "gcc", "clang", "objdump", "readelf", "gdb", "objcopy", "timeout",
    ] {
        let output = Command::new(tool)
            .arg("--version")
            .output()
            .unwrap_or_else(|error| {
                panic!("required native ABI tool `{tool}` is missing: {error}")
            });
        assert!(
            output.status.success(),
            "required native ABI tool `{tool}` failed its identity query: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let identity = String::from_utf8_lossy(&output.stdout);
        let first_line = identity.lines().next().unwrap_or("<empty version output>");
        eprintln!("{tool}: {first_line}");
    }
}
