use std::sync::Arc;

use clang::TranslationUnit;
use cranelift::prelude::{
    isa::TargetIsa,
    settings::{self, Flags},
    Configurable,
};
use cranelift_module::default_libcall_names;
use cranelift_object::{ObjectBuilder, ObjectModule};
use target_lexicon::Triple;

use crate::cli::AppArgs;

pub struct Compiler {
    module: ObjectModule,
    triple: Triple,
    isa: Arc<dyn TargetIsa>,
}

impl Compiler {
    pub fn new(args: AppArgs, tu: TranslationUnit, triple: Triple) -> Self {
        let name = args.output.file_name().unwrap().to_str().unwrap();

        let isa = {
            let mut flag_builder = settings::builder();

            // Enable the verifier
            flag_builder
                .enable("enable_verifier")
                .expect("enable_verifier should be a valid option");

            let flags = Flags::new(flag_builder);

            cranelift::codegen::isa::lookup(triple.clone())
                .unwrap_or_else(|_| panic!("platform not supported: {}", triple))
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
