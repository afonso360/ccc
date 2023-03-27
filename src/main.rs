use std::{path::PathBuf, str::FromStr};

use anyhow::Result;
use cranelift::{codegen::Context, prelude::settings};
use cranelift_module::{default_libcall_names, Linkage, Module};
use cranelift_object::{ObjectBuilder, ObjectModule};
use lang_c::driver::{parse, Config};
use lower::AstLowerer;

mod cli;
mod lower;

fn main() -> Result<()> {
    let args = cli::parse_args()?;

    let config = Config::default();
    let ast = match parse(&config, &args.input) {
        Ok(ast) => ast,
        Err(e) => {
            eprintln!("Error parsing {}\n{}", args.input.display(), e);
            std::process::exit(1);
        }
    };

    if args.dump_ast {
        println!("{:#?}", ast);
    }

    let lowerer = AstLowerer::new(ast)?;
    let ir = match lowerer.lower() {
        Ok(ir) => ir,
        Err(e) => {
            eprintln!("Error lowering IR for file {}\n{}", args.input.display(), e);
            std::process::exit(1);
        }
    };

    if args.dump_ir {
        for func in &ir {
            println!("{:#?}", func);
        }
    }

    let flag_builder = settings::builder();
    let isa_builder = cranelift::codegen::isa::lookup_by_name("x86_64-unknown-linux-gnu")?;
    let isa = isa_builder.finish(settings::Flags::new(flag_builder))?;
    let mut module = ObjectModule::new(ObjectBuilder::new(isa, "foo", default_libcall_names())?);

    for func in ir {
        let func_id = module.declare_function("main", Linkage::Local, &func.signature)?;
        let mut ctx = Context::for_function(func);
        module.define_function(func_id, &mut ctx)?;
    }

    let obj = module.finish();
    let bytes = obj.emit()?;
    std::fs::write(args.output, bytes)?;

    Ok(())
}
