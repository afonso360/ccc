static int ccc_debug_add(int left, int right) {
    return left + right;
}

int main(void) {
    volatile int ccc_debug_local = ccc_debug_add(20, 22);
    int observed = ccc_debug_local;
    return observed == 42 ? 0 : 1;
}
