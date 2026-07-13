# ADR-0002 — Syntax-owned typedef classification

Status: accepted (2026-07-13)

## Context

C's grammar is context-sensitive: `T * x;` is a multiplication or a pointer declaration depending on whether `T` is a visible typedef name. Classification also changes at exact points within a declaration, so a parser cannot consult only a completed semantic symbol table.

## Decision

`ccc-syntax` owns a reusable `NameClassEnv` for the ordinary-identifier namespace. The parser uses it while parsing; the AST records scope and binding events; `ccc-sema`, which already depends on syntax, replays and validates the same event model. There is no callback or dependency from syntax to sema and no separately invented sema version of point-of-declaration rules.

Bindings are `{TypedefName, Ordinary}`. Tags, labels, and members use their separate C namespaces and cannot accidentally shadow a typedef in the ordinary namespace.

`NameClassEnv` models file, function-prototype, function-parameter, block, and `for` scopes. It applies these transitions:

- a declarator's ordinary/typedef binding becomes visible immediately after that declarator is complete and before its initializer is parsed;
- each parameter becomes visible after its declarator, with prototype scope ending at the end of a non-definition declarator and definition parameters entering the function scope according to C's rules;
- an enumerator enters the ordinary namespace after its defining enumerator is complete;
- inner ordinary declarations hide typedef names until their scope ends;
- each declarator in a comma-separated declaration updates the environment before the next declarator is parsed;
- parser recovery rolls back or commits environment updates transactionally with the recovered declaration, so malformed input cannot poison later classification.

The implementation has direct tests for point-of-declaration and shadowing cases such as:

```c
typedef int T;
void f(void) {
    int T = sizeof(T);  /* the declarator has already made T ordinary */
}
```

and for prototype scopes, nested blocks, `for` declarations, enum constants, function definitions, and recovery after malformed declarators.

## Alternatives

- **Semantic callback into the lexer/parser.** Couples syntax to sema, risks a crate cycle, and still requires partially committed declaration state.
- **Two independent scope implementations.** Keeps crates separate but invites parser/sema disagreement on the hardest declaration cases.
- **General backtracking ambiguity nodes.** Defers the issue but complicates diagnostics and every later pass.

## Consequences

- Syntax owns a small but precisely specified scope engine.
- Semantic analysis remains responsible for type validity, redeclaration compatibility, linkage, and all non-syntactic namespace rules.
- Recorded scope events make parser/sema disagreement testable and observable.

## Revisit if

A supported language extension requires ambiguity that cannot be represented by ordinary-identifier classification plus transactional scope events. Any replacement must retain an acyclic crate graph and one shared point-of-declaration model.
