# Toolchain, dependencies, and project policy

## Rust and library dependencies

The workspace resolves `cranelift-codegen`, `cranelift-frontend`,
`cranelift-module`, `cranelift-object`, and their internal Cranelift crates from
the Wasmtime repository's `main` branch. The committed `Cargo.lock` is the
authoritative exact-revision record: ordinary builds use `--locked` and never
move with the branch implicitly. A lockfile test requires every package whose
name starts with `cranelift-` to use the audited revision recorded by the ABI
configuration key. Registry dependencies such as `object`, `target-lexicon`,
and `gimli` retain exact workspace constraints.

Cranelift lock refreshes occur in isolated changes. Refresh with
`cargo update -p cranelift-codegen`, copy the resulting Wasmtime commit from
`Cargo.lock` into the single backend-revision constant in `ccc-abi`, and inspect
the complete dependency diff before adapting APIs. Each refresh records
capability changes for variadics, f16/f128, atomic operations, memory flags,
runtime-sized automatic-storage lowering, native dynamic-stack support, TLS,
object relocations, unwind information, and debug information, then runs the
ABI oracle, object inspection, CLIF verification, execution suite, debugger
checks, and backend-specific regression corpus. A newly present API is not
enabled until its emitted behavior passes those tests.

To bisect an upstream regression, set all four workspace Git dependencies to a
candidate Wasmtime revision, refresh only the Cranelift family, update the
backend-revision constant, and run the smallest failing gate with `--locked`.
Repeat until the first bad commit is known, then restore the branch declarations
and the last accepted lockfile. A temporary rollback changes the lockfile and
backend provenance together; it must not mix packages from different commits.

CCC currently keeps Cranelift's object-level unwind feature disabled and emits
verified call-frame information itself. A backend update must not enable a
second unwind emitter accidentally.

## Target toolchain resolution

`ToolchainResolver` constructs the [`ToolchainSpec`](targets.md#effective-compilation-configuration) used for preprocessing, assembling, linking, runtime selection, and execution. Resolution order is deterministic:

1. explicit command-line paths/options such as `--gcc-toolchain`, `--sysroot`, `-isysroot`, linker selection, SDK, and deployment target;
2. target-specific CCC configuration or environment entries;
3. a native host driver only when its reported target matches the requested target;
4. otherwise, a hard error explaining which target tool is missing.

There is no cross-target fallback to an unverified host `cc`.

Resolution is phase-scoped: preprocessing and compilation require the sysroot/SDK and system include tree; assembling requires the assembler; linking requires the linker driver, CRT objects, and runtime libraries. A missing component is an error only when a selected phase needs it — `ccc -E` or `ccc -c` never fails for want of a linker.

Resolution is requirements-scoped in the implementation: `-nostdinc` with no
other system-dependent action does not probe for a system include tree, and a
preprocess-only action never resolves a linker.

The resolver probes and fingerprints the selected tools using machine-readable or stable driver queries where available: reported target, sysroot, search directories, startup objects, runtime-library paths, assembler/linker identity, PIE default, multilib selection, and builtin/system include directories. Probe results are cached by executable identity, version, target options, sysroot/SDK, and relevant environment. Missing or contradictory results are errors, not guessed paths.

For GCC- and Clang-compatible drivers, preprocessing resolution separately
probes the reported target, sysroot, and the delimited include-search listing
from a no-code preprocessing invocation. The include-list parser is tested
against recorded GCC and Clang output. A fingerprint covers the canonical
driver path, version output, target options, sysroot, relevant environment,
and normalized probe results. A Darwin development host cannot supply the
Linux GNU system-header gate by fallback; local deterministic tests use
`-nostdinc`, recorded probes, or a fake sysroot.

Darwin requires a compatible Apple SDK, deployment target, and Apple-capable linker. Linux GNU and musl configurations resolve distinct CRTs, dynamic loaders, libraries, and header trees. `ccc-link` invokes the resolved target compiler driver for executable/shared linking, the resolved assembler for generated bridge/assembly files, and the resolved `ar`/`ranlib` pair (or a verified in-process archive writer) for static archives.

Generated-symbol localization is object-format-specific. ELF packaging probes
the target `objcopy` for exact allowlist localization. Mach-O packaging uses
Apple's `nmedit -R`; using a generic object copier for this step is outside the
Darwin toolchain contract. `CCC_OBJCOPY` and `CCC_NMEDIT` select the respective
tools explicitly when reproducible environments need to pin their paths.
Darwin debug links query the selected compiler driver with
`-print-prog-name=dsymutil` and invoke the reported tool before releasing link
objects. This keeps the debug-artifact producer in the same selected developer
toolchain instead of resolving an unrelated `dsymutil` from `PATH`.

## Runtime helper manifest

Every compiler-emitted helper has a manifest entry containing symbol, exact C/ABI signature, provider preference, target availability, and a conformance test. Providers may be compiler-rt, libgcc, libatomic, libc, or a versioned CCC runtime shim. The link plan names the selected provider; it never assumes the target driver happens to supply a helper with the desired ABI.

The System V AMD64 wide-integer contract reserves direct manifest entries for
`__divti3`, `__udivti3`, `__modti3`, `__umodti3`, the signed and unsigned
`ti`-to-`sf`/`df`/`xf` conversion helpers, and the inverse
`sf`/`df`/`xf`-to-`ti` helpers. The x87-specific entries are `__floattixf`,
`__floatuntixf`, `__fixxfti`, and `__fixunsxfti`; their ABI signatures carry
f80 through the platform x87 convention even though CCC keeps source f80
values address-backed. Their manifest signatures are respectively
`long double (__int128)`, `long double (unsigned __int128)`,
`__int128 (long double)`, and `unsigned __int128 (long double)`. Cranelift's
default libcall table does not contain those symbols in the pinned backend.
Codegen therefore selects them per operation and carries their requirements
through object emission and the executable link plan.

The final link scans the resolved driver's fixed arguments and then the ordered
relocatable objects, archives, resolved libraries, and supported linker state
to select manifest entries that remain undefined
after normal archive extraction. The model covers groups, forced undefined
and symbolic-entry symbols, COMMON/weak/strong precedence, thin archives,
directory-major `-L` and
default search, dynamic visibility, `--as-needed`, and state push/pop. An
unmodeled linker script, plugin member, response input, or suppressed startup
set conservatively selects the complete provider; normal archive extraction
still loads only genuinely
needed members. CCC asks the already resolved target driver for its exact
compiler-builtins archive, verifies that archive's symbol index contains every
selected helper, and passes that canonical archive path after user inputs under
an isolated `--no-whole-archive` state. The historical driver query may resolve
GCC's libgcc or Clang's compiler-rt; CCC treats the reported archive as the
provider only after the same verification. It never verifies one archive and
then uses a generic `-lgcc` search that could select another archive through
user `-L` ordering.

CCC runtime shims use a versioned symbol namespace except where an external
ABI mandates a standard helper name. Runtime objects are selected by target and
the implemented effective ABI options. ABI-changing `long double` modes are
rejected by the driver rather than producing objects with a private variant.

The hosted automatic-storage provider selected by
[ADR-0011](../adr/0011-arena-backed-runtime-sized-automatic-storage.md) lowers
directly into each affected function. Its external ABI is exactly
`realloc(void *, size_t)` plus `free(void *)`; those ordinary libc imports are
separate from the compiler-builtins helper manifest above. Invalid extents,
size overflow, and allocation failure take an explicit backend trap. A
compile-only object containing an automatic runtime-sized object retains the
two libc references, but enabling the provider requires a hosted link profile
that resolves them. Runtime layout operations without such an object, including
VLA `sizeof`, do not import the allocation provider. Runtime-sized automatic
storage alone does not trigger generated assembly, a relocatable partial link,
or object-copy tooling. A freestanding profile must select and test another
allocator or leave the capability unavailable. Each enabled hosted target is
compiled, externally linked, and executed at `-O0` and `-O2` by the target
oracle so provider resolution is not inferred from object emission alone.

The object and executable link plan expose these provider requirements. An external
GCC- or Clang-compatible driver can link a CCC object directly because the
arena lowering is already present in that object and its remaining references
are normal hosted-libc symbols. Mixed-link tests verify this path;
provider availability is never inferred merely from a successful native link.

## Rust project policy

- The MSRV is identical in `rust-toolchain.toml` and workspace `rust-version` fields.
- `rustfmt` and `clippy` run in CI; lint policy is pinned with the toolchain so a compiler update cannot break unrelated changes silently.
- Dependency updates are batched, security updates are expedited, and generated lockfile changes are reviewed with license and advisory checks.
- Unsafe code is isolated at FFI/object/backend boundaries and documents its invariants.
