/* ccc-kernel-benchmark: memory-traffic */
/* ccc-kernel-work-unit: indexed-memory-update */
/* ccc-kernel-work-count: 4000000 */
/* ccc-kernel-expected-result: 0xf8599ec7 */

_Static_assert(sizeof(unsigned) == 4, "memory-traffic requires 32-bit unsigned");

enum {
    SLOT_COUNT = 256,
    WORK_COUNT = 4000000
};

static unsigned values[SLOT_COUNT];
static volatile unsigned seed = 0x6a09e667u;

int main(void) {
    unsigned state = seed;
    unsigned checksum = 0xbb67ae85u;
    unsigned slot;
    unsigned iteration;

    for (slot = 0; slot < SLOT_COUNT; ++slot) {
        values[slot] = state + slot * 0x9e3779b9u;
    }

    for (iteration = 0; iteration < WORK_COUNT; ++iteration) {
        unsigned index;
        unsigned neighbor;
        unsigned loaded;
        unsigned neighbor_value;
        unsigned mixed;

        state = state * 1664525u + 1013904223u;
        index = (state >> 16) & (SLOT_COUNT - 1u);
        neighbor = (index + (state >> 25) + 1u) & (SLOT_COUNT - 1u);
        loaded = values[index];
        neighbor_value = values[neighbor];
        mixed = ((loaded << 5) | (loaded >> 27)) ^ neighbor_value ^ state;
        values[index] = mixed;
        checksum = ((checksum << 7) | (checksum >> 25)) + mixed + index;
    }

    for (slot = 0; slot < SLOT_COUNT; ++slot) {
        checksum ^= values[slot] + slot * 0x85ebca6bu;
        checksum = (checksum << 11) | (checksum >> 21);
    }

    return (checksum ^ state) != 0xf8599ec7u;
}
