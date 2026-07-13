# Execution tests

The harness invokes the built `ccc` binary against every fixture in this
directory. It checks the emitted object structurally; linking an empty
translation unit is intentionally not tested because a normal hosted C runtime
requires `main`.
