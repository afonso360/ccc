use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(unix)]
use std::process::Stdio;

use object::{Object as _, ObjectKind};

static TEST_ID: AtomicU64 = AtomicU64::new(0);

fn test_directory(name: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!(
        "ccc-link-input-test-{}-{}-{name}",
        std::process::id(),
        TEST_ID.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&directory);
    fs::create_dir_all(&directory).unwrap();
    directory
}

fn ccc() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ccc"));
    command.arg("-nostdinc");
    command
}

fn assert_success(output: std::process::Output) {
    assert!(
        output.status.success(),
        "command failed with {}:\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn links_multiple_c_inputs_in_command_line_order() {
    let directory = test_directory("multiple-c");
    let main = directory.join("main.c");
    let answer = directory.join("answer.c");
    let executable = directory.join("program");
    fs::write(
        &main,
        "extern int answer(void); int main(void) { return answer() == 42 ? 0 : 1; }\n",
    )
    .unwrap();
    fs::write(&answer, "int answer(void) { return 42; }\n").unwrap();

    assert_success(
        ccc()
            .arg(&main)
            .arg(&answer)
            .arg("-o")
            .arg(&executable)
            .output()
            .unwrap(),
    );
    assert_eq!(Command::new(&executable).status().unwrap().code(), Some(0));
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn explicit_c_language_compiles_an_extensionless_input() {
    let directory = test_directory("explicit-language");
    let source = directory.join("program-source");
    let executable = directory.join("program");
    fs::write(&source, "int main(void) { return 0; }\n").unwrap();

    assert_success(
        ccc()
            .args(["-x", "c"])
            .arg(&source)
            .arg("-o")
            .arg(&executable)
            .output()
            .unwrap(),
    );
    assert_eq!(Command::new(&executable).status().unwrap().code(), Some(0));
    fs::remove_dir_all(directory).unwrap();
}

#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
#[test]
fn preprocesses_and_assembles_uppercase_assembly_inputs() {
    let directory = test_directory("preprocessed-assembly");
    let main = directory.join("main.c");
    let assembly = directory.join("answer.S");
    let executable = directory.join("program");
    fs::write(
        &main,
        "extern int assembly_answer(void); int main(void) { return assembly_answer() == 42 ? 0 : 1; }\n",
    )
    .unwrap();
    fs::write(
        &assembly,
        "#if defined(__x86_64__)\n\
         .text\n.globl assembly_answer\nassembly_answer:\n movl $42, %eax\n ret\n\
         #elif defined(__aarch64__) && defined(__APPLE__)\n\
         .text\n.globl _assembly_answer\n.p2align 2\n_assembly_answer:\n mov w0, #42\n ret\n\
         #elif defined(__aarch64__)\n\
         .text\n.globl assembly_answer\n.p2align 2\nassembly_answer:\n mov w0, #42\n ret\n\
         #else\n#error unsupported execution-test architecture\n#endif\n",
    )
    .unwrap();

    assert_success(
        ccc()
            .arg(&main)
            .arg(&assembly)
            .arg("-o")
            .arg(&executable)
            .output()
            .unwrap(),
    );
    assert_eq!(Command::new(&executable).status().unwrap().code(), Some(0));
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn command_plan_is_non_executing_and_contains_replayable_phase_commands() {
    let directory = test_directory("command-plan");
    let source = directory.join("source with spaces.c");
    let executable = directory.join("program");
    fs::write(&source, "int main(void) { return 0; }\n").unwrap();

    let output = ccc()
        .arg("-###")
        .arg(&source)
        .arg("-o")
        .arg(&executable)
        .output()
        .unwrap();
    assert_success(output.clone());
    assert!(!executable.exists());
    let plan = String::from_utf8(output.stderr).unwrap();
    assert!(plan.contains(" -c "), "{plan}");
    assert!(plan.contains("source with spaces.c'"), "{plan}");
    assert!(plan.contains(".ccc-command-plan-0.o"), "{plan}");
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn command_plan_preserves_effective_compile_and_link_options() {
    let directory = test_directory("command-plan-options");
    let source = directory.join("source.c");
    let executable = directory.join("program");
    let dependencies = directory.join("source.d");
    fs::write(&source, "int main(void) { return 0; }\n").unwrap();

    let output = ccc()
        .args([
            "-###",
            "-g",
            "-trigraphs",
            "-Werror",
            "-Wpedantic",
            "-ferror-limit=7",
            "-MD",
            "-MF",
        ])
        .arg(&dependencies)
        .arg("-fPIC")
        .arg(&source)
        .arg("-o")
        .arg(&executable)
        .output()
        .unwrap();
    assert_success(output.clone());
    let plan = String::from_utf8(output.stderr).unwrap();
    let lines = plan.lines().collect::<Vec<_>>();
    assert_eq!(
        lines.len(),
        if cfg!(target_os = "macos") { 3 } else { 2 },
        "{plan}"
    );
    for option in [
        "-g",
        "-trigraphs",
        "-Werror",
        "-Wpedantic",
        "-ferror-limit=7",
        "-MD",
        "-MF",
        "-fPIC",
    ] {
        assert!(lines[0].contains(option), "missing {option}: {plan}");
    }
    assert!(
        lines[0].contains(&dependencies.display().to_string()),
        "{plan}"
    );
    assert!(lines[1].contains("-fPIC"), "{plan}");
    if cfg!(target_os = "macos") {
        assert!(lines[2].contains("dsymutil"), "{plan}");
        assert!(lines[2].contains("program.dSYM"), "{plan}");
    }
    assert!(!executable.exists());
    assert!(!dependencies.exists());
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn compiler_identity_queries_are_available_without_inputs() {
    let machine = Command::new(env!("CARGO_BIN_EXE_ccc"))
        .arg("-dumpmachine")
        .output()
        .unwrap();
    assert_success(machine.clone());
    assert!(!String::from_utf8(machine.stdout).unwrap().trim().is_empty());

    let version = Command::new(env!("CARGO_BIN_EXE_ccc"))
        .arg("-dumpversion")
        .output()
        .unwrap();
    assert_success(version.clone());
    assert_eq!(String::from_utf8(version.stdout).unwrap(), "4.2.1\n");

    let effective = Command::new(env!("CARGO_BIN_EXE_ccc"))
        .args(["-std=c11", "-fPIC", "--print-effective-config"])
        .output()
        .unwrap();
    assert_success(effective.clone());
    let effective = String::from_utf8(effective.stdout).unwrap();
    assert!(effective.contains("language=c11\n"), "{effective}");
    assert!(effective.contains("relocation=pic\n"), "{effective}");
    assert!(effective.contains("gnu-profile=4.2.1\n"), "{effective}");
    assert!(effective.contains("compiler-driver="), "{effective}");
}

#[test]
fn response_file_can_drive_compile_and_link() {
    let directory = test_directory("response");
    let source = directory.join("source with spaces.c");
    let executable = directory.join("program");
    let response = directory.join("command.rsp");
    fs::write(&source, "int main(void) { return 0; }\n").unwrap();
    fs::write(
        &response,
        format!("\"{}\" -o \"{}\"\n", source.display(), executable.display()),
    )
    .unwrap();

    assert_success(
        ccc()
            .arg(format!("@{}", response.display()))
            .output()
            .unwrap(),
    );
    assert_eq!(Command::new(&executable).status().unwrap().code(), Some(0));
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn emits_a_shared_library_with_position_independent_code() {
    let directory = test_directory("shared");
    let source = directory.join("answer.c");
    let output = if cfg!(target_os = "macos") {
        directory.join("libanswer.dylib")
    } else {
        directory.join("libanswer.so")
    };
    fs::write(&source, "int answer(void) { return 42; }\n").unwrap();

    assert_success(
        ccc()
            .arg("-shared")
            .arg(&source)
            .arg("-o")
            .arg(&output)
            .output()
            .unwrap(),
    );
    let bytes = fs::read(&output).unwrap();
    let object = object::File::parse(bytes.as_slice()).unwrap();
    assert_eq!(object.kind(), ObjectKind::Dynamic);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn links_objects_from_a_static_archive() {
    let directory = test_directory("archive");
    let main = directory.join("main.c");
    let answer = directory.join("answer.c");
    let answer_object = directory.join("answer.o");
    let archive = directory.join("libanswer.a");
    let executable = directory.join("program");
    fs::write(
        &main,
        "extern int answer(void); int main(void) { return answer() == 42 ? 0 : 1; }\n",
    )
    .unwrap();
    fs::write(&answer, "int answer(void) { return 42; }\n").unwrap();

    assert_success(
        ccc()
            .arg("-c")
            .arg(&answer)
            .arg("-o")
            .arg(&answer_object)
            .output()
            .unwrap(),
    );
    assert_success(
        Command::new("ar")
            .arg("crs")
            .arg(&archive)
            .arg(&answer_object)
            .output()
            .unwrap(),
    );
    assert_success(
        ccc()
            .arg(&main)
            .arg("-L")
            .arg(&directory)
            .arg("-lanswer")
            .arg("-o")
            .arg(&executable)
            .output()
            .unwrap(),
    );
    assert_eq!(Command::new(&executable).status().unwrap().code(), Some(0));
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn compile_only_accepts_multiple_sources_and_derives_object_names() {
    let directory = test_directory("compile-many");
    let first = directory.join("first.c");
    let second = directory.join("second.c");
    fs::write(&first, "int first(void) { return 1; }\n").unwrap();
    fs::write(&second, "int second(void) { return 2; }\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_ccc"))
        .current_dir(&directory)
        .arg("-nostdinc")
        .arg("-c")
        .arg(&first)
        .arg(&second)
        .output()
        .unwrap();
    assert_success(output);
    assert!(directory.join("first.o").is_file());
    assert!(directory.join("second.o").is_file());
    fs::remove_dir_all(directory).unwrap();
}

#[cfg(unix)]
#[test]
fn termination_signal_removes_temporaries_and_preserves_the_destination() {
    use std::os::unix::fs::PermissionsExt as _;
    use std::os::unix::process::ExitStatusExt as _;
    use std::thread;
    use std::time::{Duration, Instant};

    let directory = test_directory("signal-cleanup");
    let main = directory.join("main.c");
    let answer = directory.join("answer.c");
    let executable = directory.join("program");
    let compiler = directory.join("blocking-cc");
    let marker = directory.join("linker.pid");
    let real_compiler =
        std::env::var_os("CCC_CC").unwrap_or_else(|| std::ffi::OsString::from("cc"));
    fs::write(
        &main,
        "extern int answer(const char *, ...);\n\
         int main(void) { return answer(\"ccc\", 1); }\n",
    )
    .unwrap();
    fs::write(
        &answer,
        "int answer(const char *ignored, ...) { return ignored == 0; }\n",
    )
    .unwrap();
    fs::write(&executable, "existing destination\n").unwrap();
    fs::write(
        &compiler,
        format!(
            "#!/bin/sh\n\
             for argument in \"$@\"; do\n\
               case \"$argument\" in\n\
                 *.o) echo $$ > '{}'; exec sleep 30 ;;\n\
               esac\n\
             done\n\
             exec \"$CCC_TEST_REAL_CC\" \"$@\"\n",
            marker.display()
        ),
    )
    .unwrap();
    let mut permissions = fs::metadata(&compiler).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&compiler, permissions).unwrap();

    let mut child = ccc()
        .arg(&main)
        .arg(&answer)
        .arg("-o")
        .arg(&executable)
        .env("CCC_CC", &compiler)
        .env("CCC_TEST_REAL_CC", real_compiler)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while !marker.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        marker.exists(),
        "the blocking target driver was not reached"
    );

    let artifact_marker = format!(".ccc-artifact-{}-", child.id());
    let packaging_workspace_exists = fs::read_dir(std::env::temp_dir())
        .unwrap()
        .filter_map(Result::ok)
        .any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .contains(&artifact_marker)
        });
    assert!(
        packaging_workspace_exists,
        "the signal test did not reach generated-artifact packaging"
    );

    let compiler_pid = fs::read_to_string(&marker).unwrap();
    let status = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(status.success());
    let status = child.wait().unwrap();
    let _ = Command::new("kill")
        .args(["-TERM", compiler_pid.trim()])
        .status();
    assert_eq!(status.signal(), Some(signal_hook::consts::SIGTERM));

    let prefix = format!("ccc-{}-", child.id());
    let leaked_process_temporaries = fs::read_dir(std::env::temp_dir())
        .unwrap()
        .filter_map(Result::ok)
        .any(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name.starts_with(&prefix) || name.contains(&artifact_marker)
        });
    assert!(!leaked_process_temporaries);
    let leaked_output_temporaries = fs::read_dir(&directory)
        .unwrap()
        .filter_map(Result::ok)
        .any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .contains(&format!(".ccc-{}-", child.id()))
        });
    assert!(!leaked_output_temporaries);
    assert_eq!(
        fs::read_to_string(&executable).unwrap(),
        "existing destination\n"
    );
    fs::remove_dir_all(directory).unwrap();
}
