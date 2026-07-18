long double unsupported_long_double_va_arg(int marker, ...) {
    __builtin_va_list arguments;
    long double value;
    __builtin_va_start(arguments, marker);
    value = __builtin_va_arg(arguments, long double);
    __builtin_va_end(arguments);
    return value;
}
