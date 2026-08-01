/* ccc-kernel-benchmark: crc32 */
/* ccc-kernel-work-unit: input-byte */
/* ccc-kernel-work-count: 1048576 */
/* ccc-kernel-expected-result: 0xaf3f6d98 */

_Static_assert(sizeof(unsigned) == 4, "crc32 requires 32-bit unsigned");

enum {
    BYTE_COUNT = 1048576
};

static volatile unsigned seed = 0x6d2b79f5u;

int main(void) {
    unsigned state = seed;
    unsigned crc = 0xffffffffu;
    unsigned index;

    for (index = 0; index < BYTE_COUNT; ++index) {
        unsigned bit;
        unsigned byte;

        state = state * 1664525u + 1013904223u;
        byte = (state >> 24) & 0xffu;
        crc ^= byte;
        for (bit = 0; bit < 8; ++bit) {
            unsigned mask = 0u - (crc & 1u);

            crc = (crc >> 1) ^ (0xedb88320u & mask);
        }
    }

    return (crc ^ state) != 0xaf3f6d98u;
}
