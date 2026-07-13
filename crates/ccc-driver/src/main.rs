use std::io::{self, Write};

fn main() {
    match ccc_driver::run(std::env::args().skip(1)) {
        Ok(output) => {
            print!("{}", output.stdout);
        }
        Err(error) => {
            let _ = writeln!(io::stderr(), "{error}");
            std::process::exit(1);
        }
    }
}
