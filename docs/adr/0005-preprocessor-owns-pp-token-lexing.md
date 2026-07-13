# ADR-0005 — Preprocessor owns pp-token lexing

Status: accepted (2026-07-12)

## Context

C's translation phases distinguish **preprocessing tokens** (pp-numbers, header-names, un-interpreted punctuators) from the **parser tokens** the grammar consumes. Macro expansion is defined over pp-tokens; conversion to parser tokens is phase 7 ([Pipeline & crates](../design/pipeline-and-crates.md)).

## Decision

`ccc-pp` owns translation phases 1–4, including the **pp-token lexer**. `ccc-syntax` owns phases 5–7 (token conversion) and parsing.

## Alternatives

- **A single shared lexer producing parser tokens directly**, with the preprocessor operating on those.

## Rationale

Conflating pp-tokens with parser tokens corrupts the operations that are _defined_ on pp-tokens — stringization (`#`), token pasting (`##`), and header-name recognition inside `#include` — and mishandles pp-numbers that aren't valid parser constants. Modeling phases as C specifies them keeps macro expansion correct.

## Consequences

- An explicit phase-5–7 conversion step in `ccc-syntax` (escape/charset decode, string-literal concatenation, pp-number → typed constant, keyword recognition).
- `#if`/`#elif` evaluation is phase 4 and stays in `ccc-pp`, which therefore owns the single pp-number/character-constant decoder; phase 7 reuses it, so the preprocessor and parser cannot diverge on the same literal spelling.

## Revisit if

Unlikely — this mirrors the C standard's own model.
