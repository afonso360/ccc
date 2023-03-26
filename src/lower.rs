use anyhow::Result;
use cranelift::{
    codegen::ir::{Function, UserFuncName},
    prelude::{
        isa::CallConv, AbiParam, FunctionBuilder, FunctionBuilderContext, InstBuilder, Signature,
        Value,
    },
};
use lang_c::{ast::*, driver::Parse, span::Node};

mod clif {
    pub use cranelift::codegen::{
        ir::{types, types::Type, AbiParam, Signature},
        isa::CallConv,
    };
}

pub struct AstLowerer {
    ast: Parse,
}

impl AstLowerer {
    pub fn new(ast: Parse) -> Result<Self> {
        // let flag_builder = settings::builder();
        // let isa_builder = cranelift::codegen::isa::lookup_by_name("x86_64-unknown-linux-gnu")?;
        // let isa = isa_builder.finish(settings::Flags::new(flag_builder))?;
        // let module = ObjectModule::new(ObjectBuilder::new(isa, "foo", default_libcall_names())?);

        Ok(Self { ast })
    }

    pub fn lower(&self) -> Result<Vec<Function>> {
        self.lower_translation_unit(&self.ast.unit)
    }

    fn lower_translation_unit(&self, unit: &TranslationUnit) -> Result<Vec<Function>> {
        unit.0
            .iter()
            .map(|item| self.lower_external_declaration(&item.node))
            .collect::<Result<_>>()
    }

    fn lower_external_declaration(&self, item: &ExternalDeclaration) -> Result<Function> {
        match item {
            ExternalDeclaration::FunctionDefinition(def) => {
                return self.lower_function_definition(&def.node);
            }
            _ => unimplemented!("lower_external_declaration: {:?}", item),
        }
    }

    fn lower_function_definition(&self, def: &FunctionDefinition) -> Result<Function> {
        let return_type = self.specifiers_to_clif_type(&def.specifiers);
        let name = match &def.declarator.node.kind.node {
            DeclaratorKind::Identifier(ident) => ident.node.name.as_str(),
            e => unimplemented!("lower_function_definition: {:?}", e),
        };

        let sig = Signature {
            params: vec![],
            returns: vec![AbiParam::new(return_type)],
            call_conv: CallConv::SystemV,
        };

        let mut ctx = FunctionBuilderContext::new();
        let mut func = Function::with_name_signature(UserFuncName::testcase(name.as_bytes()), sig);
        let mut builder = FunctionBuilder::new(&mut func, &mut ctx);

        let entry_block = builder.create_block();
        builder.append_block_params_for_function_params(entry_block);
        builder.switch_to_block(entry_block);

        self.lower_statement(&def.statement.node, &mut builder)?;

        builder.seal_all_blocks();
        builder.finalize();
        Ok(func)
    }

    fn specifiers_to_clif_type(&self, specifiers: &[Node<DeclarationSpecifier>]) -> clif::Type {
        match specifiers {
            &[Node {
                node: DeclarationSpecifier::TypeSpecifier(ref ts),
                ..
            }] if ts.node == TypeSpecifier::Int => clif::types::I64,
            _ => unimplemented!("lower_specifiers_to_clif_type: {:?}", specifiers),
        }
    }

    fn lower_statement(&self, stmt: &Statement, builder: &mut FunctionBuilder) -> Result<()> {
        match stmt {
            Statement::Compound(statements) => {
                for stmt in statements {
                    self.lower_block_item(&stmt.node, builder)?;
                }
            }
            Statement::Return(expr) => {
                match expr {
                    Some(expr) => {
                        let val = self.lower_expression(&expr.node, builder)?;
                        builder.ins().return_(&[val])
                    }
                    None => builder.ins().return_(&[]),
                };
            }
            _ => unimplemented!("lower_statement: {:?}", stmt),
        }
        Ok(())
    }

    fn lower_block_item(&self, item: &BlockItem, builder: &mut FunctionBuilder) -> Result<()> {
        match item {
            BlockItem::Statement(stmt) => self.lower_statement(&stmt.node, builder)?,
            _ => unimplemented!("lower_block_item: {:?}", item),
        }
        Ok(())
    }

    fn lower_expression(&self, expr: &Expression, builder: &mut FunctionBuilder) -> Result<Value> {
        match expr {
            Expression::Constant(cons) => self.lower_constant(&cons.node, builder),
            _ => unimplemented!("lower_expression: {:?}", expr),
        }
    }

    fn lower_constant(&self, cons: &Constant, builder: &mut FunctionBuilder) -> Result<Value> {
        match cons {
            Constant::Integer(cons) => {
                // assert_eq!(cons.suffix, IntegerSuffix::None);
                assert_eq!(cons.base, IntegerBase::Decimal);
                let val = cons.number.parse::<i64>()?;
                Ok(builder.ins().iconst(clif::types::I64, val))
            }
            _ => unimplemented!("lower_constant: {:?}", cons),
        }
    }
}
