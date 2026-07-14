int unaddressable(int final, ...) {
    register __builtin_va_list list;
    __builtin_va_start(list, final);
    return 0;
}
