use clang::{EntityKind, EntityVisitResult, EvaluationResult, TypeKind};
use cranelift::{
    codegen::ir::{Function, UserFuncName},
    prelude::{
        isa::{CallConv, TargetIsa},
        Variable, *,
    },
};
use cranelift_module::Linkage;
use quickscope::ScopeMap;
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
    pub fn cranelift_sig(&self) -> Signature {
        let mut sig = Signature::new(self.calling_conv);

        for arg in &self.args {
            sig.params.push(AbiParam::new(arg.cranelift_type()));
        }

        sig.returns
            .push(AbiParam::new(self.ret_ty.cranelift_type()));

        sig
    }

    // TODO: Maybe this should be a trait on clang::Linkage?
    pub fn cranelift_linkage(&self) -> Linkage {
        match self.linkage {
            // The AST entity has automatic storage (e.g., variables or parameters).
            clang::Linkage::Automatic => Linkage::Local,
            // The AST entity is a static variable or static function.
            clang::Linkage::Internal => Linkage::Local,
            // The AST entity has external linkage.
            clang::Linkage::External => Linkage::Export,
            // The AST entity has external linkage and lives in a C++ anonymous namespace.
            clang::Linkage::UniqueExternal => Linkage::Export,
        }
    }
}

#[derive(Debug, PartialEq, Clone)]
pub struct ASTVariable<'tu> {
    pub name: String,
    pub ty: clang::Type<'tu>,
    pub var: Variable,
}

pub struct FuncCompiler<'tu> {
    triple: &'tu Triple,
    isa: &'tu dyn TargetIsa,
    variables: ScopeMap<String, ASTVariable<'tu>>,
}

impl<'tu> FuncCompiler<'tu> {
    pub fn new(triple: &'tu Triple, isa: &'tu dyn TargetIsa) -> Self {
        Self {
            triple,
            isa,
            variables: ScopeMap::new(),
        }
    }

    pub fn parse_signature<'a>(&self, func: &'a clang::Entity) -> CompileResult<FuncSignature<'a>> {
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

    pub fn visit_children<F>(&mut self, entity: &clang::Entity<'tu>, mut cb: F) -> CompileResult<()>
    where
        F: FnMut(
            &mut Self,
            clang::Entity<'tu>,
            clang::Entity<'tu>,
        ) -> CompileResult<EntityVisitResult>,
    {
        let mut res = Ok(());
        entity.visit_children(|child, parent| {
            if child.is_unexposed() {
                return EntityVisitResult::Recurse;
            }

            match cb(self, child, parent) {
                Ok(res) => res,
                Err(e) => {
                    res = Err(e);
                    EntityVisitResult::Break
                }
            }
        });
        res
    }

    pub fn compile(mut self, func: clang::Entity<'tu>) -> CompileResult<Function> {
        debug_assert_eq!(func.get_kind(), EntityKind::FunctionDecl);

        let mut function = Function::new();
        let mut fn_builder_ctx = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut function, &mut fn_builder_ctx);

        builder.func.name = UserFuncName::user(0, 0);
        builder.func.signature = self.parse_signature(&func)?.cranelift_sig();

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
        stmt: clang::Entity<'tu>,
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
                EntityKind::DeclStmt => {
                    compiler.compile_decl_stmt(builder, child)?;
                }
                e => unimplemented!("Invalid Entity at translate {:?}", e),
            }
            Ok(EntityVisitResult::Continue)
        })?;

        Ok(())
    }

    pub fn compile_decl_stmt(
        &mut self,
        builder: &mut FunctionBuilder,
        stmt: clang::Entity<'tu>,
    ) -> CompileResult<()> {
        debug_assert_eq!(stmt.get_kind(), EntityKind::DeclStmt);

        self.visit_children(&stmt, |compiler, child, _| {
            match child.get_kind() {
                EntityKind::VarDecl => {
                    compiler.compile_var_decl(builder, child)?;
                }
                e => unimplemented!("Invalid Entity at translate {:?}", e),
            }
            Ok(EntityVisitResult::Continue)
        })?;

        Ok(())
    }

    pub fn compile_var_decl(
        &mut self,
        builder: &mut FunctionBuilder,
        stmt: clang::Entity<'tu>,
    ) -> CompileResult<()> {
        debug_assert_eq!(stmt.get_kind(), EntityKind::VarDecl);

        let ty = stmt.get_type().unwrap();
        let name = stmt.get_mangled_name().unwrap();
        let var = Variable::new(self.variables.len());

        builder.declare_var(var, ty.cranelift_type());
        let zero = self.create_zero_for_type(builder, ty);
        builder.def_var(var, zero);

        self.variables
            .define(name.clone(), ASTVariable { name, ty, var });

        self.visit_children(&stmt, |compiler, child, _| {
            let val = compiler.compile_expr_stmt(builder, child)?;
            // TODO: Check Type and cast?
            builder.def_var(var, val);

            Ok(EntityVisitResult::Continue)
        })?;

        Ok(())
    }

    pub fn compile_return_stmt(
        &mut self,
        builder: &mut FunctionBuilder,
        stmt: clang::Entity<'tu>,
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
        match stmt.get_kind() {
            // DeclRefExpr is just a variable reference
            EntityKind::DeclRefExpr => {
                let def = stmt.get_definition().unwrap();
                let name = def.get_mangled_name().unwrap();
                let var = self.variables.get(&name).expect("Unknown Variable");
                Ok(builder.use_var(var.var))
            }
            kind => {
                let val = stmt.evaluate().unwrap();
                let ty = stmt.get_type().unwrap();
                match (ty.get_kind(), val) {
                    (TypeKind::Int, EvaluationResult::SignedInteger(i)) => {
                        Ok(builder.ins().iconst(ty.cranelift_type(), i))
                    }
                    (tk, val) => unimplemented!("Invalid Entity at translate ({:?} {:?})", tk, val),
                }
            }
        }
    }
}

impl<'tu> FuncCompiler<'tu> {
    fn create_zero_for_type(&self, builder: &mut FunctionBuilder, ty: clang::Type) -> Value {
        match ty.get_kind() {
            TypeKind::Int => builder.ins().iconst(ty.cranelift_type(), 0),
            TypeKind::Float => builder.ins().f32const(0.0),
            TypeKind::Double => builder.ins().f64const(0.0),
            TypeKind::Void => builder.ins().iconst(ty.cranelift_type(), 0),
            tk => unimplemented!("Invalid Entity at translate {:?}", tk),
        }
    }
}
