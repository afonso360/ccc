int wrong_parameter(int first, int final, ...) {
    __builtin_va_list list;
    __builtin_va_start(list, first);
    return 0;
}
