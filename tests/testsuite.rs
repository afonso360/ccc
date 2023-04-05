use assert_cmd::Command;
use std::io::{self, BufRead, Read};
use std::path::Path;

include!(concat!(env!("OUT_DIR"), "/testsuite.rs"));

fn run_test(path: &str) -> Result<(), io::Error> {
    let path = Path::new(path);

    let path = path.to_owned();
    println!("testing {}", path.display());

    let mut reader = io::BufReader::new(std::fs::File::open(&path)?);

    for line in reader.by_ref().lines() {
        let line = line?;
        // As soon as we are done processing the test comments, we can stop
        if !line.starts_with("//") {
            break;
        }

        println!("Running test: \"{line}\"");

        let test_func: &fn(&Path) = match line.as_str().trim() {
            // make sure the test compiles, but don't run it
            "// test: compile" => &(assert_compiles as fn(&Path)),
            // Compiles the test, runs it, and expects a 0 exit code
            "// test: run" => &(assert_runs as fn(&Path)),
            line => panic!("Unrecognized test: {}", line),
        };

        test_func(&path);
    }

    Ok(())
}

fn assert_compiles(path: &Path) {
    let outfile = tempfile::NamedTempFile::new().unwrap();

    let mut cmd = Command::cargo_bin(env!("CARGO_PKG_NAME")).unwrap();
    cmd.arg(path)
        .arg("-O")
        .arg(outfile.path())
        .assert()
        .success();
}

fn assert_runs(path: &Path) {
    let outfile = tempfile::NamedTempFile::new().unwrap();
    let (_, outfile) = outfile.keep().unwrap();

    let mut cmd = Command::cargo_bin(env!("CARGO_PKG_NAME")).unwrap();
    cmd.arg(&path).arg("-o").arg(&outfile).assert().success();

    // Now run the binary
    let mut cmd = Command::new(&outfile);
    cmd.assert().success();

    // TODO: This leaks the file if the command fails.... not ideal
    // Delete the output file
    std::fs::remove_file(&outfile).unwrap();
}
