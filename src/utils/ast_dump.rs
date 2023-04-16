use crate::AppArgs;
use std::io::{self, Write};
use std::process::Command;

pub fn dump_ast(args: &AppArgs) {
    // Call clang to dump the AST
    let mut cmd = Command::new("clang");
    cmd.arg("-Xclang")
        .arg("-ast-dump")
        .arg("-fsyntax-only")
        .arg("-fcolor-diagnostics")
        .arg(&args.input);
    println!("// Running: {:?}", cmd);

    let output = cmd.output().expect("Failed to execute clang");

    let stdout = io::stdout();
    let mut handle = stdout.lock();
    handle.write_all(&output.stdout).unwrap();

    let stderr = io::stderr();
    let mut handle = stderr.lock();
    handle.write_all(&output.stderr).unwrap();
}
