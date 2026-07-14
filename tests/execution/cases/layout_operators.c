#include <stddef.h>

struct LayoutRecord {
    char byte;
    int integer;
    short tail;
};

union LayoutUnion {
    char byte;
    long wide;
};

int main(void) {
    if (sizeof(char) != 1 || sizeof(short) != 2 || sizeof(int) != 4)
        return 1;
    if (sizeof(long) != 8 || sizeof(long long) != 8 || sizeof(void *) != 8)
        return 2;
    if (sizeof(float) != 4 || sizeof(double) != 8 || sizeof(long double) != 16)
        return 3;
    if (_Alignof(int) != 4 || _Alignof(long) != 8 || _Alignof(long double) != 16)
        return 4;
    if (sizeof(struct LayoutRecord) != 12 || _Alignof(struct LayoutRecord) != 4)
        return 5;
    if (offsetof(struct LayoutRecord, integer) != 4
        || offsetof(struct LayoutRecord, tail) != 8)
        return 6;
    if (sizeof(union LayoutUnion) != 8 || _Alignof(union LayoutUnion) != 8)
        return 7;
    if (sizeof(int[3]) != 12)
        return 8;
    if (_Alignof(max_align_t) != 16 || sizeof(max_align_t) != 32)
        return 9;
    return 49;
}
