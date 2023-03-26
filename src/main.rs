use anyhow::Result;
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

    Ok(())
}
