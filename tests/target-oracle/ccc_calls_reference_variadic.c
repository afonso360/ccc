#include "abi_types.h"

int main(void) {
    struct Pair pair = {8, 9};
    long result = ref_collect(
        3, (signed char)-3, (unsigned short)60000, (float)1.5,
        1L, 2L, 3L, 4L, 5L, 6L, 7L, pair, 10.0);
    return result == 60056 ? 0 : 41;
}
