//! The `ccc` command-line driver.

mod empty_object;

use std::fmt;
use std::fs;
use std::path::PathBuf;

use ccc_diag::Diagnostic;
use ccc_pp::{LexError, lex};
use ccc_session::SourceMap;
use ccc_syntax::convert_pp_tokens;
use ccc_target::X86_64_UNKNOWN_LINUX_GNU;

pub use empty_object::is_empty_elf64_relocatable;

const HELP: &str = "Usage: ccc -c [-o <output>] <input.c>\n       ccc --dump-tokens <input.c>\n";

#[derive(Debug, Eq, PartialEq)]
pub struct DriverError {
    message: String,
}

impl DriverError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for DriverError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.message.fmt(formatter)
    }
}

impl std::error::Error for DriverError {}

#[derive(Debug, Eq, PartialEq)]
pub struct DriverOutput {
    pub stdout: String,
}

#[derive(Debug, Eq, PartialEq)]
enum Command {
    Compile { input: PathBuf, output: PathBuf },
    DumpTokens { input: PathBuf },
    Help,
}

/// Runs the currently supported driver commands.
pub fn run(arguments: impl IntoIterator<Item = String>) -> Result<DriverOutput, DriverError> {
    match parse_arguments(arguments)? {
        Command::Help => Ok(DriverOutput {
            stdout: HELP.to_owned(),
        }),
        Command::DumpTokens { input } => dump_tokens(input),
        Command::Compile { input, output } => compile_empty_translation_unit(input, output),
    }
}

fn parse_arguments(arguments: impl IntoIterator<Item = String>) -> Result<Command, DriverError> {
    let mut arguments = arguments.into_iter();
    let mut compile_only = false;
    let mut dump_tokens = false;
    let mut output = None;
    let mut inputs = Vec::new();

    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "-c" => compile_only = true,
            "--dump-tokens" => dump_tokens = true,
            "-o" => {
                let path = arguments
                    .next()
                    .ok_or_else(|| DriverError::new("ccc: `-o` requires an output path"))?;
                output = Some(PathBuf::from(path));
            }
            "-h" | "--help" => return Ok(Command::Help),
            "--" => inputs.extend(arguments.by_ref().map(PathBuf::from)),
            _ if argument.starts_with('-') => {
                return Err(DriverError::new(format!(
                    "ccc: unsupported option `{argument}`"
                )));
            }
            _ => inputs.push(PathBuf::from(argument)),
        }
    }

    if inputs.len() != 1 {
        return Err(DriverError::new(
            "ccc: accepts exactly one C source input; use `ccc --help` for usage",
        ));
    }
    let input = inputs.pop().expect("length was checked");

    if dump_tokens {
        if compile_only || output.is_some() {
            return Err(DriverError::new(
                "ccc: `--dump-tokens` cannot be combined with `-c` or `-o`",
            ));
        }
        return Ok(Command::DumpTokens { input });
    }

    if !compile_only {
        return Err(DriverError::new(
            "ccc: only compile-only mode is currently supported; pass `-c`",
        ));
    }

    Ok(Command::Compile {
        output: output.unwrap_or_else(|| input.with_extension("o")),
        input,
    })
}

fn read_source(path: &PathBuf) -> Result<(SourceMap, ccc_session::FileId), DriverError> {
    let source = fs::read_to_string(path).map_err(|error| {
        DriverError::new(format!("ccc: cannot read {}: {error}", path.display()))
    })?;
    let mut sources = SourceMap::new();
    let file = sources.add_file(path.display().to_string(), source);
    Ok((sources, file))
}

fn dump_tokens(input: PathBuf) -> Result<DriverOutput, DriverError> {
    let (sources, file) = read_source(&input)?;
    let tokens = lex(file, sources.source(file).expect("file was just inserted"))
        .map_err(|error| lex_error(&sources, error))?;
    let mut stdout = String::new();

    for token in convert_pp_tokens(tokens) {
        let start = sources
            .location(file, token.span.start)
            .expect("lexer spans source boundaries");
        let end = sources
            .location(file, token.span.end)
            .expect("lexer spans source boundaries");
        stdout.push_str(&format!(
            "{} {:?} {}:{}-{}:{}\n",
            token.kind.as_str(),
            token.spelling,
            start.line,
            start.column,
            end.line,
            end.column
        ));
    }

    Ok(DriverOutput { stdout })
}

fn compile_empty_translation_unit(
    input: PathBuf,
    output: PathBuf,
) -> Result<DriverOutput, DriverError> {
    let (sources, file) = read_source(&input)?;
    let tokens = lex(file, sources.source(file).expect("file was just inserted"))
        .map_err(|error| lex_error(&sources, error))?;
    if !tokens.is_empty() {
        return Err(DriverError::new(
            "ccc: can emit an object only for an empty translation unit; C parsing is not implemented yet",
        ));
    }

    empty_object::write_empty_elf64_relocatable(&output, X86_64_UNKNOWN_LINUX_GNU).map_err(
        |error| DriverError::new(format!("ccc: cannot write {}: {error}", output.display())),
    )?;
    Ok(DriverOutput {
        stdout: String::new(),
    })
}

fn lex_error(sources: &SourceMap, error: LexError) -> DriverError {
    let rendered =
        Diagnostic::error("CCC0001", error.message).with_primary(error.span, "while lexing");
    DriverError::new(format!("ccc: {}", rendered.render(sources).trim_end()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_dump_is_stable() {
        let directory =
            std::env::temp_dir().join(format!("ccc-driver-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        let input = directory.join("trivial.c");
        fs::write(&input, "int x = 42;\n").unwrap();

        let output = run(["--dump-tokens".to_owned(), input.display().to_string()]).unwrap();
        assert_eq!(
            output.stdout,
            concat!(
                "keyword \"int\" 1:1-1:4\n",
                "identifier \"x\" 1:5-1:6\n",
                "punctuator \"=\" 1:7-1:8\n",
                "integer-constant \"42\" 1:9-1:11\n",
                "punctuator \";\" 1:11-1:12\n",
            )
        );
        fs::remove_dir_all(directory).unwrap();
    }
}
