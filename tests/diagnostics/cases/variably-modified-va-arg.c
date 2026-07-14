typedef __builtin_va_list va_list;

int read_pointer(int count, ...) {
    va_list list;
    return __builtin_va_arg(list, int (*)[count]) != 0;
}
