use std::str::FromStr;
use std::sync::Arc;

use clang::{Target, TranslationUnit};
use cranelift::prelude::{
    isa::TargetIsa,
    settings::{self, Flags},
    Configurable,
};
use cranelift_module::{default_libcall_names, Module};
use cranelift_object::{ObjectBuilder, ObjectModule};
use target_lexicon::Triple;

use crate::cli::AppArgs;

fn parse_triple(triple: &str) -> Result<Triple, target_lexicon::ParseError> {
    let cleantriple = if triple.contains("msvc") {
        let parts = triple.split("-");
        let mut new_triple = String::new();
        for part in parts {
            // Clang on windows sometimes returns a msvc triple with the
            // version number, e.g. x86_64-pc-windows-msvc19.0.24215.1
            // target-lexicon doesn't support this, so we strip it out
            if part.starts_with("msvc") {
                new_triple.push_str("msvc");
            } else {
                new_triple.push_str(part);
            }
        }
        new_triple
    } else {
        triple.to_string()
    };

    Triple::from_str(&cleantriple)
}

pub struct Compiler {
    module: ObjectModule,
    triple: Triple,
    isa: Arc<dyn TargetIsa>,
}

impl Compiler {
    pub fn new(args: AppArgs, tu: TranslationUnit) -> Self {
        let target = tu.get_target();
        let name = args.output.file_name().unwrap().to_str().unwrap();
        let triple = parse_triple(&target.triple).expect("Invalid triple");

        let isa = {
            let mut flag_builder = settings::builder();

            // Enable the verifier
            flag_builder
                .enable("enable_verifier")
                .expect("enable_verifier should be a valid option");

            let flags = Flags::new(flag_builder);

            cranelift::codegen::isa::lookup(triple.clone())
                .unwrap_or_else(|_| panic!("platform not supported: {}", target.triple))
                .finish(flags)
                .expect("Failed to create target ISA")
        };
        let builder = ObjectBuilder::new(isa.clone(), name, default_libcall_names())
            .expect("Failed to instantiate ObjectBuilder");
        let module = ObjectModule::new(builder);

        Self {
            module,
            triple,
            isa,
        }
    }

    pub fn finish(self) -> ObjectModule {
        self.module
    }
}
