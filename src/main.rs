use anyhow::Result;
use clang::{Clang, EntityVisitResult, Index};
use cranelift::{codegen::Context, prelude::settings};
use cranelift_module::{default_libcall_names, Linkage, Module};
use cranelift_object::{ObjectBuilder, ObjectModule};

use crate::compiler::Compiler;

mod cli;
mod compiler;

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

    // TODO: Print Diagnostics

    // TODO: Utils dumpast
    // if args.dump_ast {
    //     println!("{:#?}", ast);
    // }

    // tu.get_entity().visit_children(|entity, parent| {
    //     println!("Hello, {:?}, {:?}", entity, parent);
    //     EntityVisitResult::Recurse
    // });

    let compiler = Compiler::new(args.clone(), tu);

    let module = compiler.finish();
    let obj = module.finish();
    let bytes = obj.emit()?;
    std::fs::write(&args.output, bytes)?;

    // Mark the binary as executable
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&args.output, std::fs::Permissions::from_mode(0o755))?;

    Ok(())
}
