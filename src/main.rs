use anyhow::Result;
use lang_c::driver::{parse, Config};

mod cli;

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

    Ok(())
}
