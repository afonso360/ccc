use clang::TypeKind;
use cranelift::prelude as clif;

pub trait ClangTypeExt {
    fn get_cranelift_type(&self) -> clif::Type;
}

impl<'a> ClangTypeExt for clang::Type<'a> {
    fn get_cranelift_type(&self) -> clif::Type {
        match self.get_kind() {
            TypeKind::Int => {
                clif::Type::int_with_byte_size(self.get_sizeof().unwrap() as u16).unwrap()
            }
            // TODO: Do not fix this to 64 bits
            TypeKind::Pointer => clif::types::I64,
            _ => unimplemented!("Unsupported type {:?}", self),
        }
    }
}
