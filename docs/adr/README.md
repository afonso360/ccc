# Architecture Decision Records

Each ADR captures one significant, cross-cutting decision: its context, the decision, the alternatives weighed, the consequences, and what would make us revisit it. New decisions are added as new numbered files; superseded ADRs are marked, not deleted.

The high-level introduction is [`../ARCHITECTURE.md`](../ARCHITECTURE.md); topical design lives in [`../design/`](../design/).

| ADR                                                          | Decision                                                          | Status   |
| ------------------------------------------------------------ | ----------------------------------------------------------------- | -------- |
| [0001](0001-ccc-ir-middle-layer.md)                          | Adopt a CCC-IR middle layer (don't lower AST→CLIF directly)       | accepted |
| [0002](0002-syntax-owned-typedef-classification.md)          | Syntax owns a shared typedef-classification event model           | accepted |
| [0003](0003-first-target-triple.md)                          | First target triple: `x86_64-unknown-linux-gnu`                   | accepted |
| [0004](0004-recursive-descent-parser.md)                     | Hand-written recursive-descent parser                             | accepted |
| [0005](0005-preprocessor-owns-pp-token-lexing.md)            | Preprocessor owns pp-token lexing                                 | accepted |
| [0006](0006-link-via-target-driver.md)                       | Link via a resolved target compiler driver                        | accepted |
| [0007](0007-long-double-and-complex.md)                      | Preserve target `long double` ABI; reject ABI-changing overrides  | accepted |
| [0008](0008-pin-cranelift.md)                                | Pin Cranelift; upgrade deliberately                               | accepted |
| [0009](0009-shared-type-and-layout-crate.md)                 | Shared `ccc-types` crate: one type representation + layout engine | accepted |
| [0010](0010-generate-abi-bridges-as-assembly.md)             | Generate ABI bridges as auditable target assembly                 | accepted |
| [0011](0011-arena-backed-runtime-sized-automatic-storage.md) | Back runtime-sized automatic objects with a scoped arena          | accepted |
