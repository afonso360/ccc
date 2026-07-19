int main(void) {
    volatile int ccc_debug_local = 42;
    int observed = ccc_debug_local;
    return observed == 42 ? 0 : 1;
}
