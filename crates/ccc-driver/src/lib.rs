//! The `ccc` command-line driver.

mod empty_object;

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use ccc_abi::plan;
use ccc_codegen::{Options as CodegenOptions, Output as CodegenOutput};
use ccc_diag::Diagnostic;
use ccc_ir::Module;
use ccc_pp::{LexError, lex};
use ccc_sema::analyze_with_config;
use ccc_session::{Session, SourceMap};
use ccc_syntax::{TranslationUnit, convert_pp_tokens, dump_ast, parse};
use ccc_target::EffectiveCompilationConfig;

pub use empty_object::is_empty_elf64_relocatable;

const HELP: &str = "Usage: ccc [-c] [-o <output>] <input.c>\n\
                   ccc --dump-tokens <input.c>\n\
                   ccc --dump-ast <input.c>\n\
                   ccc --dump-ir <input.c>\n\
                   ccc --emit=clif <input.c>\n";

static TEMPORARY_ID: AtomicU64 = AtomicU64::new(0);

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DumpKind {
    Tokens,
    Ast,
    Ir,
    Clif,
}

#[derive(Debug, Eq, PartialEq)]
enum Command {
    Compile {
        input: PathBuf,
        output: PathBuf,
        link: bool,
    },
    Dump {
        input: PathBuf,
        kind: DumpKind,
    },
    Help,
}

pub fn run(arguments: impl IntoIterator<Item = String>) -> Result<DriverOutput, DriverError> {
    match parse_arguments(arguments)? {
        Command::Help => Ok(DriverOutput {
            stdout: HELP.to_owned(),
        }),
        Command::Dump { input, kind } => dump(input, kind),
        Command::Compile {
            input,
            output,
            link,
        } => compile(input, output, link),
    }
}

fn dump(input: PathBuf, kind: DumpKind) -> Result<DriverOutput, DriverError> {
    match kind {
        DumpKind::Tokens => dump_tokens(input),
        DumpKind::Ast => {
            let parsed = parse_source(&input)?;
            Ok(DriverOutput {
                stdout: dump_ast(&parsed.ast),
            })
        }
        DumpKind::Ir => {
            let (_, ir) = lower_source(&input)?;
            Ok(DriverOutput {
                stdout: ccc_ir::dump(&ir),
            })
        }
        DumpKind::Clif => {
            let (parsed, ir) = lower_source(&input)?;
            let output = codegen(&ir, &parsed.session.config, true)?;
            Ok(DriverOutput {
                stdout: output.clif,
            })
        }
    }
}

fn parse_arguments(arguments: impl IntoIterator<Item = String>) -> Result<Command, DriverError> {
    let mut arguments = arguments.into_iter();
    let mut compile_only = false;
    let mut dump = None;
    let mut output = None;
    let mut inputs = Vec::new();

    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "-c" => compile_only = true,
            "--dump-tokens" => select_dump(&mut dump, DumpKind::Tokens)?,
            "--dump-ast" => select_dump(&mut dump, DumpKind::Ast)?,
            "--dump-ir" => select_dump(&mut dump, DumpKind::Ir)?,
            "--emit=clif" => select_dump(&mut dump, DumpKind::Clif)?,
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

    if let Some(kind) = dump {
        if compile_only || output.is_some() {
            return Err(DriverError::new(
                "ccc: dump modes cannot be combined with `-c` or `-o`",
            ));
        }
        return Ok(Command::Dump { input, kind });
    }

    let link = !compile_only;
    let output = output.unwrap_or_else(|| {
        if compile_only {
            input.with_extension("o")
        } else {
            PathBuf::from("a.out")
        }
    });
    Ok(Command::Compile {
        input,
        output,
        link,
    })
}

fn select_dump(slot: &mut Option<DumpKind>, kind: DumpKind) -> Result<(), DriverError> {
    if slot.replace(kind).is_some() {
        return Err(DriverError::new(
            "ccc: only one dump or emit mode may be selected",
        ));
    }
    Ok(())
}

struct ParsedSource {
    session: Session,
    ast: TranslationUnit,
}

fn read_source(path: &Path) -> Result<(Session, ccc_session::FileId), DriverError> {
    let bytes = fs::read(path).map_err(|error| {
        owner_error(
            "CCC6001",
            format!("cannot read {}: {error}", path.display()),
        )
    })?;
    let source = String::from_utf8(bytes).map_err(|_| {
        owner_error(
            "CCC6002",
            format!("source file {} is not valid UTF-8", path.display()),
        )
    })?;
    let mut session = Session::new(EffectiveCompilationConfig::default());
    let file = session.sources.add_file(path.display().to_string(), source);
    Ok((session, file))
}

fn parse_source(input: &Path) -> Result<ParsedSource, DriverError> {
    let (session, file) = read_source(input)?;
    let tokens = lex(
        file,
        session
            .sources
            .source(file)
            .expect("file was just inserted"),
    )
    .map_err(|error| lex_error(&session.sources, error))?;
    let tokens = convert_pp_tokens(tokens);
    let ast = parse(&tokens).map_err(|error| {
        diagnostic_error(
            &session.sources,
            Diagnostic::error(error.code, error.message).with_primary(error.span, "while parsing"),
        )
    })?;
    Ok(ParsedSource { session, ast })
}

fn lower_source(input: &Path) -> Result<(ParsedSource, Module), DriverError> {
    let parsed = parse_source(input)?;
    let typed = analyze_with_config(&parsed.ast, &parsed.session.config)
        .map_err(|diagnostics| diagnostics_error(&parsed.session.sources, diagnostics))?;
    let ir = ccc_ir::lower(&typed).map_err(|error| owner_error(error.code, error.message))?;
    Ok((parsed, ir))
}

fn codegen(
    ir: &Module,
    config: &EffectiveCompilationConfig,
    emit_clif: bool,
) -> Result<CodegenOutput, DriverError> {
    let plan = plan(ir, config).map_err(|error| owner_error(error.code, error.message))?;
    ccc_codegen::emit(ir, &plan, config, CodegenOptions { emit_clif })
        .map_err(|error| owner_error(error.code, error.message))
}

fn dump_tokens(input: PathBuf) -> Result<DriverOutput, DriverError> {
    let (session, file) = read_source(&input)?;
    let tokens = lex(
        file,
        session
            .sources
            .source(file)
            .expect("file was just inserted"),
    )
    .map_err(|error| lex_error(&session.sources, error))?;
    let mut stdout = String::new();

    for token in convert_pp_tokens(tokens) {
        let start = session
            .sources
            .location(file, token.span.start)
            .expect("lexer spans source boundaries");
        let end = session
            .sources
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

fn compile(input: PathBuf, output: PathBuf, link: bool) -> Result<DriverOutput, DriverError> {
    let (parsed, ir) = lower_source(&input)?;
    let generated = codegen(&ir, &parsed.session.config, false)?;
    if link {
        let mut temporary = TemporaryObject::create()?;
        temporary.write_all(&generated.object)?;
        ccc_link::link_executable(temporary.path(), &output, &parsed.session.config)
            .map_err(|error| owner_error(error.code, error.message))?;
    } else {
        fs::write(&output, generated.object).map_err(|error| {
            DriverError::new(format!("ccc: cannot write {}: {error}", output.display()))
        })?;
    }
    Ok(DriverOutput {
        stdout: String::new(),
    })
}

struct TemporaryObject {
    path: PathBuf,
    file: File,
}

impl TemporaryObject {
    fn create() -> Result<Self, DriverError> {
        for _ in 0..100 {
            let id = TEMPORARY_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!("ccc-{}-{id}.o", std::process::id()));
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(file) => return Ok(Self { path, file }),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => {
                    return Err(DriverError::new(format!(
                        "ccc: cannot create a temporary object: {error}"
                    )));
                }
            }
        }
        Err(DriverError::new(
            "ccc: cannot allocate a collision-free temporary object path",
        ))
    }

    fn write_all(&mut self, bytes: &[u8]) -> Result<(), DriverError> {
        self.file.write_all(bytes).map_err(|error| {
            DriverError::new(format!("ccc: cannot write temporary object: {error}"))
        })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TemporaryObject {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn lex_error(sources: &SourceMap, error: LexError) -> DriverError {
    diagnostic_error(
        sources,
        Diagnostic::error(error.code, error.message).with_primary(error.span, "while lexing"),
    )
}

fn diagnostic_error(sources: &SourceMap, diagnostic: Diagnostic) -> DriverError {
    DriverError::new(format!("ccc: {}", diagnostic.render(sources).trim_end()))
}

fn owner_error(code: &'static str, message: impl Into<String>) -> DriverError {
    diagnostic_error(&SourceMap::new(), Diagnostic::error(code, message))
}

fn diagnostics_error(sources: &SourceMap, diagnostics: Vec<Diagnostic>) -> DriverError {
    let rendered = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.render(sources))
        .collect::<Vec<_>>()
        .join("");
    DriverError::new(format!("ccc: {}", rendered.trim_end()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary_source(name: &str, source: &str) -> (PathBuf, PathBuf) {
        let directory = std::env::temp_dir().join(format!(
            "ccc-driver-test-{}-{}",
            std::process::id(),
            TEMPORARY_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&directory).unwrap();
        let input = directory.join(name);
        fs::write(&input, source).unwrap();
        (directory, input)
    }

    #[test]
    fn token_dump_is_stable() {
        let (directory, input) = temporary_source("trivial.c", "int x = 42;\n");
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

    #[test]
    fn renders_ast_ir_and_clif() {
        let (directory, input) =
            temporary_source("program.c", "int main(void) { int x = 40; return x + 2; }");
        let input = input.display().to_string();
        let ast = run(["--dump-ast".to_owned(), input.clone()])
            .unwrap()
            .stdout;
        assert!(ast.contains("function main"));
        assert_eq!(
            ast,
            run(["--dump-ast".to_owned(), input.clone()])
                .unwrap()
                .stdout
        );
        let ir = run(["--dump-ir".to_owned(), input.clone()]).unwrap().stdout;
        assert!(ir.contains("function @main"));
        assert!(!ir.contains("iconst 0"));
        assert_eq!(
            ir,
            run(["--dump-ir".to_owned(), input.clone()]).unwrap().stdout
        );
        let clif = run(["--emit=clif".to_owned(), input.clone()])
            .unwrap()
            .stdout;
        assert!(clif.contains("function main"));
        assert_eq!(clif, run(["--emit=clif".to_owned(), input]).unwrap().stdout);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn compiles_a_function_object() {
        use object::{Object as _, ObjectSymbol as _};

        let (directory, input) = temporary_source("program.c", "int main(void) { return 42; }");
        let output = directory.join("program.o");
        run([
            "-c".to_owned(),
            input.display().to_string(),
            "-o".to_owned(),
            output.display().to_string(),
        ])
        .unwrap();
        let bytes = fs::read(&output).unwrap();
        let object = object::File::parse(bytes.as_slice()).unwrap();
        assert!(object.symbols().any(|symbol| symbol.name() == Ok("main")));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn reports_semantic_errors_with_source_locations() {
        let (directory, input) =
            temporary_source("invalid.c", "int main(void) { return missing; }");
        let error = run(["-c".to_owned(), input.display().to_string()]).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("CCC2005"));
        assert!(message.contains("invalid.c:1:"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn unsupported_or_invalid_programs_do_not_emit_objects() {
        let cases = [
            (
                "directive.c",
                "#define X 1\nint main(void) { return X; }",
                "CCC1001",
            ),
            (
                "pointer.c",
                "int main(void) { int *p; return 0; }",
                "CCC1001",
            ),
            (
                "wide-literal.c",
                "int main(void) { return 2147483648; }",
                "CCC2004",
            ),
            (
                "wrong-arity.c",
                "int f(int x) { return x; } int main(void) { return f(); }",
                "CCC2009",
            ),
        ];
        for (name, source, code) in cases {
            let (directory, input) = temporary_source(name, source);
            let output = directory.join("invalid.o");
            let error = run([
                "-c".to_owned(),
                input.display().to_string(),
                "-o".to_owned(),
                output.display().to_string(),
            ])
            .unwrap_err();
            assert!(error.to_string().contains(code), "{name}: {error}");
            assert!(!output.exists(), "{name} unexpectedly emitted an object");
            fs::remove_dir_all(directory).unwrap();
        }
    }

    #[test]
    fn reports_non_utf8_source_with_a_driver_diagnostic() {
        let (directory, input) = temporary_source("invalid.c", "");
        fs::write(&input, b"int main(void) { /* \xff */ return 0; }").unwrap();
        let error = run(["-c".to_owned(), input.display().to_string()]).unwrap_err();
        assert!(error.to_string().contains("CCC6002"));
        assert!(error.to_string().contains("not valid UTF-8"));
        fs::remove_dir_all(directory).unwrap();
    }
}
