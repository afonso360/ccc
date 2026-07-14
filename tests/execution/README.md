# Execution tests

The harness invokes the built `ccc` binary against every fixture in this
directory. Every host checks the emitted x86-64 ELF objects structurally.
x86-64 Linux additionally links and executes the programs and verifies their
exit status. Fixtures use behavior defined by C11 or by the effective x86-64
GNU target configuration; they do not compare padding bytes, depend on
unspecified evaluation order, or access inactive union members. The empty
translation unit is object-only because a normal hosted C runtime requires
`main` when linking an executable.
