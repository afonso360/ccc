const HELP: &str = "\
ccc
USAGE:
  ccc [OPTIONS] [FILE]
FLAGS:
  -h, --help            Prints help information
OPTIONS:
  -o <file>             Place the output into <file>.
  --dump-ast            Dump the AST after parsing.
ARGS:
  <FILE>
";

#[derive(Debug)]
pub struct AppArgs {
    pub input: std::path::PathBuf,
    pub output: Option<std::path::PathBuf>,
    pub dump_ast: bool,
}

pub fn parse_args() -> Result<AppArgs, pico_args::Error> {
    let mut pargs = pico_args::Arguments::from_env();

    // Help has a higher priority and should be handled separately.
    if pargs.contains(["-h", "--help"]) {
        print!("{}", HELP);
        std::process::exit(0);
    }

    let args = AppArgs {
        dump_ast: pargs.contains("--dump-ast"),
        output: pargs.opt_value_from_os_str("--output", parse_path)?,
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
