extern _Thread_local int reference_tls_value;
extern int reference_tls_read(void);

int main(void) {
    if (reference_tls_value != 39 || reference_tls_read() != 39) {
        return 1;
    }
    reference_tls_value = 71;
    return reference_tls_read() == 71 ? 0 : 2;
}
