static long double native_long_double_storage;

_Static_assert(sizeof(long double) == 16, "native long double storage size");
_Static_assert(_Alignof(long double) == 16, "native long double storage alignment");

int native_long_double_storage_size(void) {
    return sizeof(native_long_double_storage);
}

long double *native_long_double_storage_address(void) {
    return &native_long_double_storage;
}
