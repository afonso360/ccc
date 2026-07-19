# CCC

CCC is the Cranelift C Compiler. It compiles a pragmatic C11 and GNU C subset
through a fully typed AST, a target-independent CCC-IR control-flow graph,
explicit per-target ABI plans, and Cranelift. Enabled profiles cover x86-64,
AArch64, and RISC-V64 Linux ELF plus AArch64 Darwin Mach-O. Implemented
behavior includes scalar and aggregate types and calls, variadic functions,
thread-local storage, position-independent code, runtime-sized automatic
arrays, native-width integer and pointer atomics, nonlocal control transfer,
and source-level DWARF. Type widths, plain-`char` signedness, layout, packing,
conversions, relocation model, and volatile accesses come from the effective
target configuration.

The C11 preprocessing pipeline supports object-like, function-like, and
variadic macros; stringization and token pasting; conditional directives;
quoted, system, computed, and next-header includes; forced inputs; line
control; feature predicates; common GCC pragmas; reproducible predefined
macros; and Make dependency output. Source files are UTF-8 and may begin with a
byte-order mark. The default language mode is `gnu11`; `-std=c11` selects the
strict preprocessing rules.

The hosted GNU compatibility profile includes selected declaration attributes,
assembly labels, statement expressions, computed goto, 128-bit integers,
builtins, and exact inventory-derived inline-assembly forms. Unsupported GNU
forms remain represented long enough to produce a precise diagnostic and fail
before object emission. Installed system headers use the discovered target
include search; the compiler-owned resource directory supplies headers such as
`<stddef.h>`, `<stdarg.h>`, and the supported scalar subset of `<stdatomic.h>`.

Unavailable behavior is diagnosed rather than approximated. Current hard
boundaries include complex arithmetic and ABI transport, Linux binary128
`long double` value operations, non-native atomic representations, general
unclassified inline assembly, and GNU attributes whose observable semantics
are not implemented. X86-64 f80 arithmetic and ABI transport use localized x87
support and generated System V bridges; Darwin `long double` uses its native
binary64 representation.

```sh
cargo run -p ccc-driver -- -c program.c
cargo run -p ccc-driver -- -E -P -I include program.c
cargo run -p ccc-driver -- -MMD -MF program.d -c program.c
cargo run -p ccc-driver -- --dump-pp-tokens program.c
cargo run -p ccc-driver -- --dump-ast program.c
cargo run -p ccc-driver -- --dump-typed-ast program.c
cargo run -p ccc-driver -- --dump-ir program.c
cargo run -p ccc-driver -- --dump-abi program.c
cargo run -p ccc-driver -- --emit=clif program.c
```

The driver discovers and fingerprints the matching target compiler, include
tree, sysroot or SDK, assembler, linker, and object tools. It accepts ordered C,
assembly, object, archive, and library inputs; emits default PIE executables or
explicit shared objects; and exposes replayable phase plans and effective
configuration queries for build systems. Native and cross-target execution
coverage is recorded in the testing design and corpus applicability matrix.
