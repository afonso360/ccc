_Thread_local int tls_global_dynamic
    __attribute__((tls_model("global-dynamic"))) = 3;
_Thread_local int tls_local_dynamic
    __attribute__((tls_model("local-dynamic"))) = 5;
_Thread_local int tls_initial_exec
    __attribute__((tls_model("initial-exec"))) = 7;
_Thread_local int tls_local_exec
    __attribute__((tls_model("local-exec"))) = 11;
_Thread_local int tls_zero;

#if defined(__APPLE__)
static _Thread_local int tls_exact_internal asm("physical_tls") = 13;

int *address_tls_exact_internal(void) {
    return &tls_exact_internal;
}
#endif

int read_tls_models(void) {
    return tls_global_dynamic + tls_local_dynamic + tls_initial_exec
        + tls_local_exec + tls_zero;
}
