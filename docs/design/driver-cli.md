# Driver: CLI surface, flag policy, and observability

`ccc-driver` parses a GCC/Clang-compatible command line, constructs one [`EffectiveCompilationConfig`](targets.md#effective-compilation-configuration), selects compilation phases, resolves the [target toolchain](toolchain.md#target-toolchain-resolution), and executes the link plan.

## Inputs and outputs

The driver supports multiple C source, preprocessed C (`.i` or `-x c-cpp-output`), assembly (`.s`, and `.S` preprocessed first), object, archive, and library inputs with GCC-compatible `-c`, `-E`, and link-mode `-o` validation. Assembly inputs are driven through the resolved target assembler; real build systems (and corpus targets such as musl) feed `$CC` assembly files directly. It preserves argument order for libraries and linker options, uses collision-free temporary paths, deletes temporaries on success/failure/signal, and prints replayable compile and link commands under `-###` without executing them. `-S` is diagnosed as described below.

Core options include `-x`, `-std=`, `-O`, `-g`, `-D`, `-U`, include/library paths, forced includes, dependency generation, response files, target/sysroot/toolchain selection, threading, PIC/PIE/shared/static selection, rpaths, and pass-through linker arguments.

`-g`, `-g1`, `-g2`, and `-g3` enable the same source-debug profile;
`-g0` disables it, and the last debug-level option wins. Debug objects contain
DWARF compilation units, line rows, subprogram ranges, canonical C types,
a single function-wide lexical block per subprogram, parameter DIEs, automatic
storage DIEs, and compilation-unit-level DIEs for defined data objects. Nested
source blocks and the original scope of block-static objects are not
reconstructed. Fixed frame slots are described relative to the target frame
pointer. Automatic scalars promoted to SSA use version-specific Cranelift value
labels and location lists clipped at verifier-backed source-state boundaries;
an uninitialized, optimized-away, or otherwise unavailable version creates a
gap rather than inheriting a stale location. Runtime-sized and dynamically
realigned objects likewise omit a location rather than publishing a false one.
Defined x86-64 ELF TLS objects use a DTP-relative relocation followed by
`DW_OP_form_tls_address`; Darwin arm64 uses the corresponding TLV descriptor
offset expression. Declaration-only objects have no DIE. The ordinary System
V call-frame section is retained alongside source-level debug sections.
Unsupported debug dialects and levels are rejected instead of being treated as
no-ops. For a Darwin executable or dynamic-library link, the driver resolves
`dsymutil` through the selected compiler driver, publishes the linked binary,
and materializes a staged `.dSYM` while every registered object and Mach-O OSO
debug-map input is still available. The completed bundle then replaces its
destination before temporary objects are released. Relocatable links and `-g0`
do not create a bundle.

`-DNAME`, `-DNAME=value`, and function-like definitions such as
`-D'F(x)=x'` use the same tokenization and definition checks as source
`#define` directives. `-D` and `-U` are applied in command-line order.
`-imacros` inputs are processed, in order, before the ordered `-include`
inputs.

Plain `-E` emits deterministic GCC-style linemarkers. The initial main-file
marker has no entry flag; included-file entry, include return, and system-header
state use flags `1`, `2`, and `3` respectively. Entry and return markers use
the source spelling consistently, and logical locations changed by `#line` are
reflected in subsequent markers. `-P` suppresses every linemarker. Textual
output collapses ordinary whitespace without ever concatenating spellings into
a different preprocessing token.

Dependency options have the following driver contract:

- `-M` and `-MM` select dependency-only output, imply `-E`, and suppress
  warnings. Without `-MF`, the rule is written to stdout unless `-o` selects a
  file. `-MF -` always selects stdout.
- `-MD` and `-MMD` retain the selected compile or preprocess action and emit a
  dependency file as a side effect. Without `-MF`, its name is derived from
  `-o` or the input. With `-E` and no `-MF`, `-o` names the dependency output;
  when `-MF` is present, `-o` remains the preprocessed-output destination.
- `-MM` and `-MMD` exclude headers reached from system include entries or a
  system-header region. `-MT`, `-MQ`, and `-MP` follow Make target, escaping,
  and phony-rule semantics; each `-MP` phony rule is separated by a blank line.
- `-MG` is recognized and rejected as unsupported rather than silently
  changing missing-header errors.

Dependency files are replaced atomically only after successful preprocessing.

Build-system introspection is part of the compatible surface: `--version`, `-v`, `-dumpmachine`, `-dumpversion`, `-dM` with `-E`, `-print-prog-name=`, `-print-file-name=`, `-print-search-dirs`, and compile/link `-###` output are derived from the effective configuration and resolved toolchain. autotools, libtool, and CMake identify and probe compilers with these before compiling anything.

`-S` is reserved for assemblable target output with the same symbols,
relocatable expressions, visibility, and section semantics that assembling it
would produce in object mode. The selected backend cannot currently produce
that faithful representation, so `-S` is a clear unsupported-capability error.
`--emit=asm` likewise reports that annotated disassembly is unavailable; neither
option writes a disassembly file while claiming GCC-compatible assembly output.

Response files use documented GCC-compatible quoting and nesting rules with a recursion limit. Linker response files are produced only for the selected linker flavor; driver response syntax is not assumed to match every linker.

## Flag classification

Every recognized option is classified by its parser entry and tests according to how it affects the effective configuration and one of these behaviors:

- **Implemented:** apply and test the complete semantics.
- **Behavior-compatible no-op:** accept only from an explicit allowlist whose entry explains why it cannot affect language semantics, ABI, predefined macros, object contents, linking, or observable output.
- **Diagnostic-only:** accept and emit the documented warning/remark behavior.
- **Degradable hardening option:** a recognized code-generation option whose absence weakens only hardening or quality of implementation — never language semantics, type layout, or call ABI (e.g. `-fstack-protector-strong`, `-fstack-clash-protection`, `-fcf-protection` while unimplemented). Accepted with a warning; its classification documents the weakened contract and any predefined macros deliberately left undefined. Distro-injected `CFLAGS` and non-probing build systems pass these unconditionally, so a hard error would fail builds CCC can compile correctly.
- **Unsupported semantic option:** hard error before compiling any input.

Unknown options are errors by default. CCC never classifies an option as harmless merely because it begins with `-f` or `-m`; flags such as character signedness, enum size, packing, builtin assumptions, overflow, ISA, TLS, visibility, and calling conventions are semantics- or ABI-changing. A build-system compatibility mode may downgrade a specifically allowlisted unknown diagnostic option, but cannot downgrade code-generation or target options; recognized hardening flags use the degradable-hardening state above instead of a downgrade.

Warning options are also fail-closed. Emitted categories and explicitly audited
diagnostic-only compatibility names share one driver registry; unknown `-W`,
`-Wno-`, `-Werror=`, and `-Wno-error=` names are errors. `-W`, `-Wall`, and
`-Wextra` are positive compatibility selectors because CCC's current warning
set is enabled by default; unsupported negative or promotion forms of those
external compiler groups are not inferred. Per-category enable, suppression,
promotion, and demotion are resolved in command-line order, with specific
promotion state overriding global `-Werror`. Assembler pass-through spellings
such as `-Wa,...` are rejected until the external-assembler phase has a safe,
ordered forwarding contract.

Unsupported `-std=`, target, ABI, `long double`, relocation, or overflow modes
are fatal. Ignored-option diagnostics name the allowlist rule and are
controllable with a dedicated warning category.

## Target and link planning

The target driver, sysroot/SDK, include paths, runtime helpers, CRT objects, default PIE behavior, linker flavor, and archive tools all come from `ToolchainResolver`. Link mode and code-generation relocation mode are selected together. An explicit non-PIE executable passes the resolved driver's disable-PIE option; shared/PIE output cannot consume non-PIC objects produced by the same invocation.

Runtime-sized automatic storage is a named code-generation requirement, not an
implicit libc guess. The hosted arena provider lowers directly into each
affected function and leaves only ordinary `realloc` and `free` references for
a hosted link. Invalid extents, size overflow, and allocation failure use the
backend's explicit trap path rather than importing an abort helper. It does not
add assembler, partial-link, or object-copy steps. An external compatible
compiler driver can therefore link that object using its ordinary hosted libc
because no CCC runtime archive remains to be supplied. `-###` shows the compile
and link commands but does not claim source-specific helper selection without
compiling the source. A runtime layout operation that does not allocate an
automatic object, such as VLA `sizeof`, emits no `realloc` or `free` reference.

CCC validates the architecture and format of primary and generated objects
while packaging them. Direct user objects, archives, dynamic libraries, and
`-l` inputs remain allowed. Before the final link CCC models the resolved
driver's fixed arguments followed by the user arguments using ordinary
left-to-right symbol resolution solely to select unresolved compiler-runtime
helpers: archive groups and extraction, forced undefined symbols,
symbolic entry points,
COMMON/weak/strong precedence, thin members, library-search order, dynamic
visibility, and `--as-needed` state are retained. Linker scripts, plugin inputs,
startup-file suppression, and other selection mechanisms that cannot be
reconstructed select the complete
archive provider, whose ordinary member extraction keeps this fallback safe.
The resolved platform linker still owns architecture, format, and ABI
diagnostics. CCC does not attach or validate private ABI or long-double metadata
on arbitrary user link inputs.

## Observability

`-E`, `--dump-pp-tokens`, `--dump-tokens`, `--dump-ast`,
`--dump-typed-ast`, `--dump-ir`, `--dump-abi`, `--emit=clif`, `--emit=obj`,
`-###`, and `--print-effective-config` expose stable, deterministic
representations suitable for snapshot tests. Dumps separate semantic content
from unstable entity numbering and absolute temporary paths.

`--dump-pp-tokens` shows the expanded preprocessing-token stream, including
stable origin summaries. `--dump-tokens` shows the converted parser-token
stream. `-dM -E` emits the final macro environment, including predefined
macros, as `#define` directives.
