_Thread_local int reference_tls_value = 39;

int reference_tls_read(void) {
    return reference_tls_value;
}
