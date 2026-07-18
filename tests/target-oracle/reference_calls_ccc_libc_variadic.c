#include <string.h>

#include "abi_types.h"

int main(void) {
    char buffer[64];
    int length = ccc_format(buffer, sizeof(buffer), "%d:%s:%.1f", 17, "oracle", 2.5);
    if (length != 13) return 81;
    return strcmp(buffer, "17:oracle:2.5") == 0 ? 0 : 82;
}
