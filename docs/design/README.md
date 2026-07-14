# CCC design documents

Detailed design for CCC. The high-level introduction is [`../ARCHITECTURE.md`](../ARCHITECTURE.md); decision records are in [`../adr/`](../adr/).

| Document                                          | Covers                                                                                        |
| ------------------------------------------------- | --------------------------------------------------------------------------------------------- |
| [Targets & non-goals](targets.md)                 | Exact target triples, object formats, ABIs, and explicit non-goals                            |
| [Conformance policy](conformance.md)              | `long double`, `_Complex`, effective implementation-defined behavior, GNU capability registry |
| [Frontend capabilities](frontend-capabilities.md) | Syntax recognition, GNU declarations, capability states, and header phase certification       |
| [ABI & variadic functions](abi-and-varargs.md)    | ABI plans, aggregate boundaries, variadic calls/definitions, `va_list`, and target shims      |
| [Pipeline & crates](pipeline-and-crates.md)       | Annotated pipeline, per-component design, workspace/crate layout                              |
| [CCC-IR invariants](ccc-ir.md)                    | Places vs values, aggregates, bitfields, volatile, atomics; sema guarantees                   |
| [Resource directory](resource-dir.md)             | Shipped builtin headers, include search, runtime helper strategy                              |
| [Driver & CLI](driver-cli.md)                     | Flag surface, unknown-flag policy, observability dumps                                        |
| [Diagnostics](diagnostics.md)                     | Spans, macro/include backtraces, parser recovery, warning control                             |
| [Cranelift risk register](cranelift-risks.md)     | The hard backend problems (ABI, varargs, `long double`, inline asm, TLS, …)                   |
| [Testing strategy](testing.md)                    | Snapshot/execution/differential tiers, the ABI oracle, corpus licensing                       |
| [Toolchain & policy](toolchain.md)                | Dependencies, Cranelift pinning, MSRV, lint/dep policy                                        |
