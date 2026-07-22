_Thread_local int ccc_debug_tls = 22;

static int ccc_debug_add(int left, int right) {
    return left + right + ccc_debug_tls - 22;
}

int main(void) {
    volatile int ccc_debug_local = ccc_debug_add(20, 22);
    int observed = ccc_debug_local;
    return observed == 42 ? 0 : 1;
}
