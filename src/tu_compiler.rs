use std::borrow::BorrowMut;
use std::sync::Arc;

use clang::{Entity, EntityKind, EntityVisitResult, TranslationUnit};
use cranelift::codegen::{self, Context};
use cranelift::prelude::{
    isa::TargetIsa,
    settings::{self, Flags},
    Configurable,
};
use cranelift_module::{default_libcall_names, Module};
use cranelift_object::{ObjectBuilder, ObjectModule};
use target_lexicon::Triple;

use crate::{cli::AppArgs, func::FuncCompiler};

pub type CompileResult<T> = Result<T, anyhow::Error>;

pub struct TUCompiler {
    // TODO: Should this be here?
    args: AppArgs,
    module: ObjectModule,
    ctx: Context,
    triple: Triple,
    isa: Arc<dyn TargetIsa>,
}

impl TUCompiler {
    pub fn new(args: AppArgs, triple: Triple) -> Self {
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
        let ctx = module.make_context();

        Self {
            args,
            module,
            ctx,
            triple,
            isa,
        }
    }

    pub fn translate(&mut self, tu: TranslationUnit) -> CompileResult<()> {
        let entity = tu.get_entity();
        entity.visit_children(|child, _| {
            match child.get_kind() {
                EntityKind::FunctionDecl => {
                    self.lower_func_decl(child).unwrap();
                }
                e => unimplemented!("Invalid Entity at translate {:?}", e),
            }
            EntityVisitResult::Continue
        });
        Ok(())
    }

    fn lower_func_decl(&mut self, entity: Entity) -> CompileResult<()> {
        assert_eq!(entity.get_kind(), EntityKind::FunctionDecl);

        let func_compiler = FuncCompiler::new(&self.triple, &*self.isa);

        let sig = func_compiler.parse_signature(&entity)?;

        // Declare the function in the module
        let func_id = self.module.declare_function(
            &sig.name,
            sig.cranelift_linkage(),
            &sig.cranelift_sig(),
        )?;

        // TODO: Clang types are not Send, so we can't do the translation in parallel. But that
        // would be really neat
        let func = func_compiler.compile(entity)?;
        if self.args.dump_ir {
            println!("[{}]: {}", &func.name, func);
        }

        // if let Err(err) = codegen::verify_function(&func, &self.flags) {
        //     panic!(
        //         "verification error: {}\nnote: while compiling {}",
        //         err, func
        //     );
        // }
        self.ctx.func = func;
        self.module.define_function(func_id, &mut self.ctx)?;
        self.ctx.clear();

        // if let Err(err) = self.module.define_function(func_id, &mut ctx) {
        //     panic!(
        //         "definition error: {}\nnote: while compiling {}",
        //         err, ctx.func
        //     );
        // }

        Ok(())
    }

    pub fn finish(self) -> ObjectModule {
        self.module
    }
}

impl TUCompiler {
    // fn declare_func(&mut self, symbol: Symbol) -> CompileResult<FuncId> {
    //     // use saltwater_parser::get_str;
    //     // if !is_definition {
    //     //     // case 2 and 4
    //     //     if let Some(Id::Function(func_id)) = self.declarations.get(&symbol) {
    //     //         return Ok(*func_id);
    //     //     }
    //     // }
    //     // let metadata = symbol.get();
    //     // let func_type = match &metadata.ctype {
    //     //     Type::Function(func_type) => func_type,
    //     //     _ => unreachable!("bug in backend: only functions should be passed to `declare_func`"),
    //     // };
    //     // let signature = func_type.signature(self.module.isa());
    //     // let linkage = match metadata.storage_class {
    //     //     StorageClass::Auto | StorageClass::Extern if is_definition => Linkage::Export,
    //     //     StorageClass::Auto | StorageClass::Extern => Linkage::Import,
    //     //     StorageClass::Static => Linkage::Local,
    //     //     StorageClass::Register | StorageClass::Typedef => unreachable!(),
    //     // };
    //     // let func_id = self
    //     //     .module
    //     //     .declare_function(get_str!(metadata.id), linkage, &signature)
    //     //     .unwrap_or_else(|err| panic!("{}", err));
    //     // self.declarations.insert(symbol, Id::Function(func_id));
    //     // Ok(func_id)
    //     unimplemented!()
    // }
}
