use std::{path::PathBuf, str::FromStr};

const HELP: &str = "\
ccc
USAGE:
  ccc [OPTIONS] [FILE]
FLAGS:
  -h, --help            Prints help information
OPTIONS:
  -o <file>             Place the output into <file>.
  --dump-ast            Dump the AST after parsing.
  --dump-ir             Dump the IR after lowering.
ARGS:
  <FILE>
";

#[derive(Debug, Clone, PartialEq)]
pub struct AppArgs {
    pub input: PathBuf,
    pub output: PathBuf,
    pub dump_ast: bool,
    pub dump_ir: bool,
}

pub fn parse_args() -> Result<AppArgs, pico_args::Error> {
    let mut pargs = pico_args::Arguments::from_env();

    // Help has a higher priority and should be handled separately.
    if pargs.contains(["-h", "--help"]) {
        print!("{}", HELP);
        std::process::exit(0);
    }

    let args = AppArgs {
        dump_ir: pargs.contains("--dump-ir"),
        dump_ast: pargs.contains("--dump-ast"),
        output: pargs
            .opt_value_from_os_str("-o", parse_path)?
            .unwrap_or_else(|| PathBuf::from_str("./a.out").unwrap()),
        input: pargs.free_from_str()?,
    };

    // It's up to the caller what to do with the remaining arguments.
    let remaining = pargs.finish();
    if !remaining.is_empty() {
        eprintln!("Warning: unused arguments left: {:?}.", remaining);
    }

    Ok(args)
}

fn parse_path(s: &std::ffi::OsStr) -> Result<std::path::PathBuf, &'static str> {
    Ok(s.into())
}
