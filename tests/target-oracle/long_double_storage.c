static long double binary128_storage;

_Static_assert(sizeof(long double) == 16, "binary128 storage size");
_Static_assert(_Alignof(long double) == 16, "binary128 storage alignment");

int binary128_storage_size(void) {
    return sizeof(binary128_storage);
}
