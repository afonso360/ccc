//! Namespace and function-label state used by semantic analysis.

use std::collections::HashMap;

use ccc_session::Span;
use ccc_types::{QualifiedType, TypeId};

use super::model::{FullFunctionId, FullLocalId, GlobalId, LabelId, TypedefId};

#[derive(Clone, Debug)]
pub(super) enum OrdinarySymbol {
    Global(GlobalId, QualifiedType),
    Function(FullFunctionId, TypeId),
    Local(FullLocalId, QualifiedType),
    Typedef(TypedefId, QualifiedType),
    Enumerator(i128, QualifiedType),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum TagCategory {
    Struct,
    Union,
    Enum,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct TagSymbol {
    pub category: TagCategory,
    pub ty: TypeId,
}

#[derive(Default)]
struct SemanticScope {
    ordinary: HashMap<String, OrdinarySymbol>,
    tags: HashMap<String, TagSymbol>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum OrdinaryBindingConflict {
    CurrentScope,
    FileScope,
}

/// The nested ordinary-identifier and tag namespaces for one translation unit.
///
/// The first entry is the permanent file scope. Each block adds one entry, so
/// ordinary identifiers and tags shadow independently while retaining the C
/// namespace split.
pub(super) struct ScopeStack {
    scopes: Vec<SemanticScope>,
}

impl ScopeStack {
    pub fn new() -> Self {
        Self {
            scopes: vec![SemanticScope::default()],
        }
    }

    pub fn push(&mut self) {
        self.scopes.push(SemanticScope::default());
    }

    pub fn pop(&mut self) {
        assert!(self.scopes.len() > 1, "the file scope is permanent");
        self.scopes.pop();
    }

    pub fn current_ordinary(&self, name: &str) -> Option<&OrdinarySymbol> {
        self.current().ordinary.get(name)
    }

    pub fn lookup_ordinary(&self, name: &str) -> Option<&OrdinarySymbol> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.ordinary.get(name))
    }

    pub fn lookup_file_ordinary(&self, name: &str) -> Option<&OrdinarySymbol> {
        self.file().ordinary.get(name)
    }

    pub fn bind_current_ordinary(
        &mut self,
        name: String,
        symbol: OrdinarySymbol,
    ) -> Result<(), OrdinaryBindingConflict> {
        let ordinary = &mut self.current_mut().ordinary;
        if ordinary.contains_key(&name) {
            return Err(OrdinaryBindingConflict::CurrentScope);
        }
        ordinary.insert(name, symbol);
        Ok(())
    }

    pub fn bind_file_ordinary(
        &mut self,
        name: String,
        symbol: OrdinarySymbol,
    ) -> Result<(), OrdinaryBindingConflict> {
        let ordinary = &mut self.file_mut().ordinary;
        if ordinary.contains_key(&name) {
            return Err(OrdinaryBindingConflict::FileScope);
        }
        ordinary.insert(name, symbol);
        Ok(())
    }

    pub fn replace_current_ordinary(&mut self, name: String, symbol: OrdinarySymbol) {
        let replaced = self.current_mut().ordinary.insert(name, symbol);
        assert!(replaced.is_some(), "the current binding must already exist");
    }

    pub fn replace_file_ordinary(&mut self, name: String, symbol: OrdinarySymbol) {
        let replaced = self.file_mut().ordinary.insert(name, symbol);
        assert!(
            replaced.is_some(),
            "the file-scope binding must already exist"
        );
    }

    pub fn bind_current_tag(&mut self, name: String, tag: TagSymbol) -> Result<(), ()> {
        let tags = &mut self.current_mut().tags;
        if tags.contains_key(&name) {
            return Err(());
        }
        tags.insert(name, tag);
        Ok(())
    }

    pub fn lookup_tag(&self, name: &str) -> Option<TagSymbol> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.tags.get(name).copied())
    }

    fn current(&self) -> &SemanticScope {
        self.scopes.last().expect("the file scope is permanent")
    }

    fn current_mut(&mut self) -> &mut SemanticScope {
        self.scopes.last_mut().expect("the file scope is permanent")
    }

    fn file(&self) -> &SemanticScope {
        &self.scopes[0]
    }

    fn file_mut(&mut self) -> &mut SemanticScope {
        &mut self.scopes[0]
    }
}

#[derive(Clone, Debug)]
struct LabelState {
    id: LabelId,
    definition: Option<Span>,
    uses: Vec<Span>,
}

/// The function-wide label namespace.
///
/// Labels have function scope in C and therefore deliberately live outside
/// [`ScopeStack`]. Definitions are reserved before statement analysis so
/// forward gotos resolve to stable identifiers.
#[derive(Default)]
pub(super) struct LabelScope {
    labels: HashMap<String, LabelState>,
}

impl LabelScope {
    pub fn reserve_definition(&mut self, name: &str) {
        if self.labels.contains_key(name) {
            return;
        }
        let id = LabelId(self.labels.len() as u32);
        self.labels.insert(
            name.to_owned(),
            LabelState {
                id,
                definition: None,
                uses: Vec::new(),
            },
        );
    }

    pub fn define(&mut self, name: &str, span: Span) -> Result<LabelId, ()> {
        let state = self
            .labels
            .get_mut(name)
            .expect("labels are reserved before statement analysis");
        let duplicate = state.definition.replace(span).is_some();
        if duplicate { Err(()) } else { Ok(state.id) }
    }

    pub fn note_use(&mut self, name: &str, span: Span) -> LabelId {
        let state = self
            .labels
            .entry(name.to_owned())
            .or_insert_with(|| LabelState {
                id: LabelId(u32::MAX),
                definition: None,
                uses: Vec::new(),
            });
        state.uses.push(span);
        state.id
    }

    pub fn undefined_uses(&self) -> Vec<(String, Span)> {
        self.labels
            .iter()
            .filter(|(_, label)| label.definition.is_none() && !label.uses.is_empty())
            .map(|(name, label)| (name.clone(), label.uses[0]))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use ccc_session::{SourceMap, Span};
    use ccc_types::{BuiltinType, QualifiedType, TypeStore};

    use super::*;

    #[test]
    fn ordinary_and_tag_namespaces_shadow_independently() {
        let types = TypeStore::default();
        let int = QualifiedType::unqualified(TypeId::INT);
        let unsigned = QualifiedType::unqualified(types.builtin(BuiltinType::UnsignedInt));
        let mut scopes = ScopeStack::new();
        scopes
            .bind_file_ordinary("value".to_owned(), OrdinarySymbol::Global(GlobalId(0), int))
            .unwrap();
        scopes
            .bind_current_tag(
                "value".to_owned(),
                TagSymbol {
                    category: TagCategory::Struct,
                    ty: TypeId::INT,
                },
            )
            .unwrap();

        scopes.push();
        scopes
            .bind_current_ordinary(
                "value".to_owned(),
                OrdinarySymbol::Local(FullLocalId(0), unsigned),
            )
            .unwrap();

        assert!(matches!(
            scopes.lookup_ordinary("value"),
            Some(OrdinarySymbol::Local(FullLocalId(0), ty)) if *ty == unsigned
        ));
        assert!(matches!(
            scopes.lookup_file_ordinary("value"),
            Some(OrdinarySymbol::Global(GlobalId(0), ty)) if *ty == int
        ));
        assert_eq!(
            scopes.lookup_tag("value").unwrap().category,
            TagCategory::Struct
        );

        scopes.pop();
        assert!(matches!(
            scopes.lookup_ordinary("value"),
            Some(OrdinarySymbol::Global(GlobalId(0), ty)) if *ty == int
        ));
    }

    #[test]
    fn labels_are_function_wide_and_report_the_first_undefined_use() {
        let mut sources = SourceMap::new();
        let file = sources.add_file("labels.c", "goto missing;");
        let first = Span::new(file, 0, 4);
        let second = Span::new(file, 5, 12);
        let mut labels = LabelScope::default();

        labels.reserve_definition("forward");
        let forward = labels.note_use("forward", first);
        assert_eq!(labels.define("forward", second), Ok(forward));
        labels.note_use("missing", first);
        labels.note_use("missing", second);

        assert_eq!(labels.undefined_uses(), vec![("missing".to_owned(), first)]);
    }
}
