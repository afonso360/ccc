int variadic(int final, ...) {
    __builtin_va_list list;
    float value;
    __builtin_va_start(list, final);
    value = __builtin_va_arg(list, float);
    __builtin_va_end(list);
    return (int)value;
}
