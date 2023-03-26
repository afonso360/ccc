use lang_c::driver::{parse, Config};

fn main() {
    let config = Config::default();
    println!("{:#?}", parse(&config, "test.c"));
}
