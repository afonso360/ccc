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

#if defined(__APPLE__) && defined(__aarch64__)
#define EXPECTED_LONG_DOUBLE_SIZE 8
#define EXPECTED_LONG_DOUBLE_ALIGNMENT 8
#define EXPECTED_MAX_ALIGN_SIZE 16
#define EXPECTED_MAX_ALIGN_ALIGNMENT 8
#else
#define EXPECTED_LONG_DOUBLE_SIZE 16
#define EXPECTED_LONG_DOUBLE_ALIGNMENT 16
#define EXPECTED_MAX_ALIGN_SIZE 32
#define EXPECTED_MAX_ALIGN_ALIGNMENT 16
#endif

int main(void) {
    if (sizeof(char) != 1 || sizeof(short) != 2 || sizeof(int) != 4)
        return 1;
    if (sizeof(long) != 8 || sizeof(long long) != 8 || sizeof(void *) != 8)
        return 2;
    if (sizeof(float) != 4 || sizeof(double) != 8
        || sizeof(long double) != EXPECTED_LONG_DOUBLE_SIZE)
        return 3;
    if (_Alignof(int) != 4 || _Alignof(long) != 8
        || _Alignof(long double) != EXPECTED_LONG_DOUBLE_ALIGNMENT)
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
    if (_Alignof(max_align_t) != EXPECTED_MAX_ALIGN_ALIGNMENT
        || sizeof(max_align_t) != EXPECTED_MAX_ALIGN_SIZE)
        return 9;
    return 49;
}
