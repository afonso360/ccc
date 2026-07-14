//! Ordinary-identifier classification and scope event tracking.

use std::collections::HashMap;

use ccc_session::Span;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NameClass {
    TypedefName,
    Ordinary,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScopeKind {
    File,
    FunctionPrototype,
    Function,
    Block,
    For,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ScopeEventKind {
    Enter(ScopeKind),
    Leave(ScopeKind),
    Bind { name: String, class: NameClass },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScopeEvent {
    pub kind: ScopeEventKind,
    pub depth: usize,
    pub span: Span,
}

#[derive(Clone, Debug)]
struct ScopeFrame {
    kind: ScopeKind,
    bindings: HashMap<String, NameClass>,
}

/// A rollback point for a tentative declaration parse.
#[derive(Clone, Debug)]
pub struct NameClassCheckpoint {
    scopes: Vec<ScopeFrame>,
    event_len: usize,
}

/// Syntax-owned ordinary-identifier classification environment.
#[derive(Clone, Debug)]
pub struct NameClassEnv {
    scopes: Vec<ScopeFrame>,
    pub(super) events: Vec<ScopeEvent>,
}

impl Default for NameClassEnv {
    fn default() -> Self {
        Self::new()
    }
}

impl NameClassEnv {
    pub fn new() -> Self {
        Self {
            scopes: vec![ScopeFrame {
                kind: ScopeKind::File,
                bindings: HashMap::new(),
            }],
            events: Vec::new(),
        }
    }

    pub fn lookup(&self, name: &str) -> Option<NameClass> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.bindings.get(name).copied())
    }

    pub fn bind(&mut self, name: impl Into<String>, class: NameClass, span: Span) {
        let name = name.into();
        self.scopes
            .last_mut()
            .expect("the file scope is permanent")
            .bindings
            .insert(name.clone(), class);
        self.events.push(ScopeEvent {
            kind: ScopeEventKind::Bind { name, class },
            depth: self.scopes.len() - 1,
            span,
        });
    }

    pub fn enter_scope(&mut self, kind: ScopeKind, span: Span) {
        self.scopes.push(ScopeFrame {
            kind,
            bindings: HashMap::new(),
        });
        self.events.push(ScopeEvent {
            kind: ScopeEventKind::Enter(kind),
            depth: self.scopes.len() - 1,
            span,
        });
    }

    pub fn leave_scope(&mut self, span: Span) {
        let frame = self
            .scopes
            .pop()
            .expect("the parser does not leave the file scope");
        assert!(!self.scopes.is_empty(), "the file scope is permanent");
        self.events.push(ScopeEvent {
            kind: ScopeEventKind::Leave(frame.kind),
            depth: self.scopes.len(),
            span,
        });
    }

    pub fn checkpoint(&self) -> NameClassCheckpoint {
        NameClassCheckpoint {
            scopes: self.scopes.clone(),
            event_len: self.events.len(),
        }
    }

    pub fn commit(&mut self, _checkpoint: NameClassCheckpoint) {}

    pub fn rollback(&mut self, checkpoint: NameClassCheckpoint) {
        self.scopes = checkpoint.scopes;
        self.events.truncate(checkpoint.event_len);
    }

    pub fn events(&self) -> &[ScopeEvent] {
        &self.events
    }
}
