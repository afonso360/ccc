#include "abi_types.h"

int main(void) {
    int result = ccc_unwind_entry(2);
    if (result != 0) return result;
    return ccc_unwind_variadic(2, 9);
}
