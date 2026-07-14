#include <stddef.h>

struct Flags {
    unsigned int low : 3;
    unsigned int middle : 5;
    unsigned int high : 1;
    int signed_plain : 3;
};

#pragma pack(push, 1)
struct PackedValue {
    char tag;
    int value;
};
#pragma pack(pop)

int main(void) {
    struct Flags flags = {0, 0, 0, 0};
    struct PackedValue packed = {'A', 0x01020304};

    flags.low = 5;
    flags.middle = 19;
    flags.high = 1;
    flags.signed_plain = -2;
    if (flags.low != 5 || flags.middle != 19 || flags.high != 1
        || flags.signed_plain != -2)
        return 1;
    if (sizeof(struct PackedValue) != 5
        || _Alignof(struct PackedValue) != 1
        || offsetof(struct PackedValue, value) != 1)
        return 2;
    if (packed.tag != 'A' || packed.value != 0x01020304)
        return 3;
    return 44;
}
