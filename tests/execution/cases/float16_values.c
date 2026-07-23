typedef __builtin_va_list va_list;

union HalfBits {
    _Float16 value;
    unsigned short bits;
};

static unsigned short bits(_Float16 value) {
    union HalfBits representation;
    representation.value = value;
    return representation.bits;
}

static _Float16 pass(_Float16 value) {
    return value;
}

static _Float16 narrow_double(double value) {
    return value;
}

static unsigned short variadic_bits(int count, ...) {
    va_list arguments;
    union HalfBits representation;
    __builtin_va_start(arguments, count);
    representation.value = __builtin_va_arg(arguments, _Float16);
    return representation.bits;
}

int main(void) {
    volatile double tie_down_source = 1.00048828125;
    volatile double tie_up_source = 1.00146484375;
    volatile double subnormal_source = 0x1p-24;
    volatile _Float16 left = 1.5;
    volatile _Float16 right = 2.0;
    if (bits((_Float16)1.5) != 0x3e00) return 1;
    if (bits((_Float16)1.00048828125) != 0x3c00) return 2;
    if (bits((_Float16)1.00146484375) != 0x3c02) return 3;
    if (bits((_Float16)0x1p-24) != 1) return 4;
    if (bits((_Float16)0x1p-25) != 0) return 5;
    if (bits((_Float16)1.0 + (_Float16)0x1p-11) != 0x3c00) return 6;
    if (bits(left * right) != 0x4200) return 7;
    if (bits(pass((_Float16)-2.0)) != 0xc000) return 8;
    if (variadic_bits(1, (_Float16)1.5) != 0x3e00) return 9;
    if ((double)(_Float16)1.5 != 1.5) return 10;
    if ((int)(_Float16)-2.0 != -2) return 11;
    if (bits(narrow_double(tie_down_source)) != 0x3c00) return 12;
    if (bits(narrow_double(tie_up_source)) != 0x3c02) return 13;
    if (bits(narrow_double(subnormal_source)) != 1) return 14;
#if defined(__x86_64__) && defined(__linux__)
    {
        volatile long double above_halfway = 0x1.0020000000000002p0L;
        if (bits((_Float16)above_halfway) != 0x3c01) return 15;
    }
#endif
    return 0;
}
