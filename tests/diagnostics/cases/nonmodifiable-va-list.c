int nonmodifiable(int final, ...) {
    const __builtin_va_list list;
    __builtin_va_start(list, final);
    return 0;
}
