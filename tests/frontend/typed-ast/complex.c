#pragma pack(push, wire, 1)
struct {
    const int *volatile pointer;
    struct {
        unsigned bits : 3;
        int value;
    } nested;
} packed_global = {
    .pointer = 0,
    .nested = { .value = 7 },
};
#pragma pack(pop, wire)

volatile unsigned counter = 2;

int convert(short value, int *restrict out) {
    *out = value + counter;
    return packed_global.nested.value + sizeof(packed_global);
}
