use clang::{EntityKind, EntityVisitResult, EvaluationResult, TypeKind};
use cranelift::{
    codegen::ir::{Function, UserFuncName},
    prelude::{
        isa::{CallConv, TargetIsa},
        *,
    },
};
use target_lexicon::Triple;

use crate::tu_compiler::CompileResult;

use self::r#type::ClangTypeExt;

mod r#type;

pub struct FuncSignature<'tu> {
    pub ret_ty: clang::Type<'tu>,
    pub args: Vec<clang::Type<'tu>>,
    pub linkage: clang::Linkage,
    pub name: String,
    // TODO: Cranelift type for now... We should change to clang types
    pub calling_conv: CallConv,
}

impl<'tu> FuncSignature<'tu> {
    pub fn to_cranelift_sig(&self) -> Signature {
        let mut sig = Signature::new(self.calling_conv);

        for arg in &self.args {
            sig.params.push(AbiParam::new(arg.get_cranelift_type()));
        }

        sig.returns
            .push(AbiParam::new(self.ret_ty.get_cranelift_type()));

        sig
    }
}

pub struct FuncCompiler<'tu> {
    triple: &'tu Triple,
    isa: &'tu dyn TargetIsa,
}

impl<'tu> FuncCompiler<'tu> {
    pub fn new(triple: &'tu Triple, isa: &'tu dyn TargetIsa) -> Self {
        Self { triple, isa }
    }

    pub fn parse_signature(&self, func: &'tu clang::Entity) -> CompileResult<FuncSignature> {
        debug_assert_eq!(func.get_kind(), EntityKind::FunctionDecl);

        let ret_ty = func.get_result_type().unwrap();
        let args = func
            .get_arguments()
            .unwrap()
            .into_iter()
            .map(|arg| arg.get_type().unwrap())
            .collect::<Vec<_>>();

        let linkage = func.get_linkage().unwrap();
        let name = func.get_mangled_name().unwrap();

        // TODO: This doesen't exit? let calling_conv = func.get_calling_conv().unwrap();
        let calling_conv = CallConv::triple_default(self.triple);

        Ok(FuncSignature {
            ret_ty,
            args,
            linkage,
            name,
            calling_conv,
        })
    }

    pub fn visit_children<F>(&mut self, entity: &clang::Entity, mut cb: F) -> CompileResult<()>
    where
        F: FnMut(&mut Self, clang::Entity, clang::Entity) -> CompileResult<EntityVisitResult>,
    {
        let mut res = Ok(());
        entity.visit_children(|child, parent| match cb(self, child, parent) {
            Ok(res) => res,
            Err(e) => {
                res = Err(e);
                EntityVisitResult::Break
            }
        });
        res
    }

    pub fn compile(mut self, func: clang::Entity) -> CompileResult<Function> {
        debug_assert_eq!(func.get_kind(), EntityKind::FunctionDecl);

        let mut function = Function::new();
        let mut fn_builder_ctx = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut function, &mut fn_builder_ctx);

        builder.func.name = UserFuncName::user(0, 0);
        builder.func.signature = self.parse_signature(&func)?.to_cranelift_sig();

        let block0 = builder.create_block();
        builder.append_block_params_for_function_params(block0);
        builder.switch_to_block(block0);

        // Translate the function body
        self.visit_children(&func, |compiler, child, _| {
            match child.get_kind() {
                EntityKind::ParmDecl => {
                    println!("TODO!!!! ParamDecl: {:?}", child)
                }
                EntityKind::CompoundStmt => {
                    compiler.compile_compund_stmt(&mut builder, child)?;
                }
                e => unimplemented!("Invalid Entity at translate {:?}", e),
            }
            Ok(EntityVisitResult::Continue)
        })?;

        builder.seal_all_blocks();
        builder.finalize();
        Ok(function)
    }

    pub fn compile_compund_stmt(
        &mut self,
        builder: &mut FunctionBuilder,
        stmt: clang::Entity,
    ) -> CompileResult<()> {
        debug_assert_eq!(stmt.get_kind(), EntityKind::CompoundStmt);

        self.visit_children(&stmt, |compiler, child, _| {
            match child.get_kind() {
                // EntityKind::ParmDecl => {
                //     println!("TODO!!!! ParamDecl: {:?}", child)
                // }
                EntityKind::CompoundStmt => {
                    println!("TODO!!!! CompoundStmt: {:?}", child)
                }
                EntityKind::ReturnStmt => {
                    compiler.compile_return_stmt(builder, child)?;
                }
                e => unimplemented!("Invalid Entity at translate {:?}", e),
            }
            Ok(EntityVisitResult::Continue)
        })?;

        Ok(())
    }

    pub fn compile_return_stmt(
        &mut self,
        builder: &mut FunctionBuilder,
        stmt: clang::Entity,
    ) -> CompileResult<()> {
        debug_assert_eq!(stmt.get_kind(), EntityKind::ReturnStmt);

        self.visit_children(&stmt, |compiler, child, _| {
            let val = compiler.compile_expr_stmt(builder, child)?;
            builder.ins().return_(&[val]);

            Ok(EntityVisitResult::Continue)
        })?;

        Ok(())
    }

    pub fn compile_expr_stmt(
        &mut self,
        builder: &mut FunctionBuilder,
        stmt: clang::Entity,
    ) -> CompileResult<Value> {
        let val = stmt.evaluate().unwrap();
        let ty = stmt.get_type().unwrap();
        match (ty.get_kind(), val) {
            (TypeKind::Int, EvaluationResult::SignedInteger(i)) => {
                Ok(builder.ins().iconst(ty.get_cranelift_type(), i))
            }
            (tk, val) => unimplemented!("Invalid Entity at translate ({:?} {:?})", tk, val),
        }
    }
}
