use anyhow::Result;
use clang::{Clang, Index};
use std::{fs, str::FromStr};
use target_lexicon::Triple;

use crate::compiler::Compiler;

mod cli;
mod compiler;

fn parse_triple(triple: &str) -> Result<Triple, target_lexicon::ParseError> {
    let cleantriple = if triple.contains("msvc") {
        triple
            .split("-")
            .map(|part| {
                // Clang on windows sometimes returns a msvc triple with the
                // version number, e.g. x86_64-pc-windows-msvc19.0.24215.1
                // target-lexicon doesn't support this, so we strip it out
                if part.starts_with("msvc") {
                    "msvc"
                } else {
                    part
                }
            })
            .collect::<Vec<_>>()
            .join("-")
    } else {
        triple.to_string()
    };

    Triple::from_str(&cleantriple)
}

fn main() -> Result<()> {
    let args = cli::parse_args()?;

    let clang = match Clang::new() {
        Ok(clang) => clang,
        Err(e) => {
            eprintln!("Error creating Clang instance: {}", e);
            std::process::exit(1);
        }
    };

    // Create a new `Index`
    let index = Index::new(&clang, false, false);

    // Parse a source file into a translation unit
    let tu = match index.parser(&args.input).parse() {
        Ok(tu) => tu,
        Err(e) => {
            eprintln!("Error parsing {}\n{}", args.input.display(), e);
            std::process::exit(1);
        }
    };

    let target = tu.get_target();
    let triple = parse_triple(&target.triple).expect("Invalid triple");

    // TODO: Print Diagnostics

    // TODO: Utils dumpast
    // if args.dump_ast {
    //     println!("{:#?}", ast);
    // }

    // tu.get_entity().visit_children(|entity, parent| {
    //     println!("Hello, {:?}, {:?}", entity, parent);
    //     EntityVisitResult::Recurse
    // });

    let compiler = Compiler::new(args.clone(), tu, triple);

    let module = compiler.finish();
    let obj = module.finish();
    let bytes = obj.emit()?;
    fs::write(&args.output, bytes)?;

    // Mark the binary as executable
    // TODO: Do we need something like this on Windows?
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&args.output, std::fs::Permissions::from_mode(0o755))?;
    }

    Ok(())
}
