# Driver: CLI surface, flag policy, and observability

`ccc-driver` parses a GCC/Clang-compatible command line, constructs one [`EffectiveCompilationConfig`](targets.md#effective-compilation-configuration), selects compilation phases, resolves the [target toolchain](toolchain.md#target-toolchain-resolution), and executes the link plan.

## Inputs and outputs

The driver supports multiple C source, assembly (`.s`, and `.S` preprocessed first), object, archive, and library inputs with GCC-compatible `-c`, `-S`, `-E`, and link-mode `-o` validation. Assembly inputs are driven through the resolved target assembler; real build systems (and corpus targets such as musl) feed `$CC` assembly files directly. It preserves argument order for libraries and linker options, uses collision-free temporary paths, deletes temporaries on success/failure/signal, and prints a replayable command plan under `-###` without executing it.

Core options include `-x`, `-std=`, `-O`, `-g`, `-D`, `-U`, include/library paths, forced includes, dependency generation, response files, target/sysroot/toolchain selection, threading, PIC/PIE/shared/static selection, rpaths, and pass-through linker arguments.

Build-system introspection is part of the compatible surface: `--version`, `-v`, `-dumpmachine`, `-dumpversion`, `-dM` with `-E`, `-print-prog-name=`, `-print-file-name=`, `-print-search-dirs`, and `-###` produce GCC-compatible output derived from the effective configuration and resolved toolchain. autotools, libtool, and CMake identify and probe compilers with these before compiling anything.

`-S` means assemblable target output with the same symbols, relocatable expressions, visibility, and section semantics that assembling it would produce in object mode. Annotated disassembly is exposed separately as `--emit=asm`. If the selected backend cannot produce faithful assemblable output, `-S` is a clear unsupported-capability error; it never writes a disassembly file while claiming GCC-compatible `-S` behavior.

Response files use documented GCC-compatible quoting and nesting rules with a recursion limit. Linker response files are produced only for the selected linker flavor; driver response syntax is not assumed to match every linker.

## Flag classification

Every recognized option has a registry entry that states how it affects the effective configuration and one of these behaviors:

- **Implemented:** apply and test the complete semantics.
- **Behavior-compatible no-op:** accept only from an explicit allowlist whose entry explains why it cannot affect language semantics, ABI, predefined macros, object contents, linking, or observable output.
- **Diagnostic-only:** accept and emit the documented warning/remark behavior.
- **Degradable hardening option:** a recognized code-generation option whose absence weakens only hardening or quality of implementation — never language semantics, type layout, or call ABI (e.g. `-fstack-protector-strong`, `-fstack-clash-protection`, `-fcf-protection` while unimplemented). Accepted with a warning; the registry entry documents the weakened contract and any predefined macros deliberately left undefined. Distro-injected `CFLAGS` and non-probing build systems pass these unconditionally, so a hard error would fail builds CCC can compile correctly.
- **Unsupported semantic option:** hard error before compiling any input.

Unknown options are errors by default. CCC never classifies an option as harmless merely because it begins with `-f` or `-m`; flags such as character signedness, enum size, packing, builtin assumptions, overflow, ISA, TLS, visibility, and calling conventions are semantics- or ABI-changing. A build-system compatibility mode may downgrade a specifically allowlisted unknown diagnostic option, but cannot downgrade code-generation or target options; recognized hardening flags use the degradable-hardening state above instead of a downgrade.

`-mlong-double-64` is an explicit ABI compatibility choice described in [Conformance](conformance.md#long-double). Unsupported `-std=`, target, ABI, relocation, or overflow modes are fatal. Ignored-option diagnostics name the allowlist rule and are controllable with a dedicated warning category.

## Target and link planning

The target driver, sysroot/SDK, include paths, runtime helpers, CRT objects, default PIE behavior, linker flavor, and archive tools all come from `ToolchainResolver`. Link mode and code-generation relocation mode are selected together. An explicit non-PIE executable passes the resolved driver's disable-PIE option; shared/PIE output cannot consume non-PIC objects produced by the same invocation.

Before execution, the driver validates that every input object's architecture, object format, CCC ABI metadata, and long-double mode is compatible with the link. Non-CCC objects remain allowed and are checked using available object metadata plus the selected platform ABI.

## Observability

`-E`, `--dump-pp-tokens`, `--dump-tokens`, `--dump-ast`, `--dump-typed-ast`, `--dump-ir`, `--dump-abi`, `--emit=clif`, `--emit=obj`, `--emit=asm`, `-###`, and `--print-effective-config` expose stable, deterministic representations suitable for snapshot tests. Dumps separate semantic content from unstable entity numbering and absolute temporary paths.
