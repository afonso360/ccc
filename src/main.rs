use anyhow::{anyhow, Result};
use clang::{Clang, Index};
use std::path::Path;
use std::{fs, str::FromStr};
use target_lexicon::Triple;

use crate::cli::AppArgs;
use crate::tu_compiler::TUCompiler;

mod cli;
mod func;
mod tu_compiler;
mod utils;

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

    let triple = args
        .target
        .clone()
        .map(|target| parse_triple(&target))
        .unwrap_or_else(|| {
            let target = tu.get_target();
            parse_triple(&target.triple)
        })?;
    dbg!(&triple);

    // TODO: Print Diagnostics

    if args.dump_ast {
        utils::ast_dump::dump_ast(&args);
    }

    let mut compiler = TUCompiler::new(args.clone(), triple.clone());
    compiler.translate(tu)?;

    let module = compiler.finish();
    let obj = module.finish();
    let bytes = obj.emit()?;

    // Write the prelinking file to temp file
    let outfile = tempfile::NamedTempFile::new()?;
    // fs::write(outfile.path(), bytes)?;
    let (_, tmppath) = outfile.keep()?;
    fs::write(&tmppath, bytes)?;

    // Link the final binary
    let link_res = link(tmppath.as_path(), &triple, &args);

    fs::remove_file(tmppath)?;

    link_res
}

pub fn link(obj_file: &Path, triple: &Triple, args: &AppArgs) -> Result<()> {
    // link the .o file using host linker

    let linker = if cfg!(windows) { "link.exe" } else { "cc" };
    let mut cmd = std::process::Command::new(linker);

    if cfg!(windows) {
        cmd.arg("/NOLOGO");
    }

    if cfg!(windows) {
        cmd.arg(format!("/OUT:{}", args.output.display()));
    } else {
        cmd.arg("-o").arg(&args.output);
    }

    // if !cfg!(windows) {
    //     cmd.arg("--target").arg(triple.to_string());
    // }

    // Link against libc
    if !cfg!(windows) {
        // cmd.arg("libcmt.lib");
        cmd.arg("-lc");
    }

    cmd.arg(&obj_file);

    dbg!(&cmd);

    let link_status = cmd.status()?;
    if !link_status.success() {
        Err(anyhow!("Linking failed: {}", link_status))
    } else {
        Ok(())
    }
}
