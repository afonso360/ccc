_Thread_local long double unsupported_long_double_tls;

long double *unsupported_long_double_tls_address(void) {
    return &unsupported_long_double_tls;
}
