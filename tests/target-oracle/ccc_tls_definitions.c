_Thread_local int ccc_tls_value = 17;

int ccc_tls_read(void) {
    return ccc_tls_value;
}

void ccc_tls_write(int value) {
    ccc_tls_value = value;
}
