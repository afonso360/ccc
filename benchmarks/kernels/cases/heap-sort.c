/* ccc-kernel-benchmark: heap-sort */
/* ccc-kernel-work-unit: 1024-element-sort */
/* ccc-kernel-work-count: 128 */
/* ccc-kernel-expected-result: 0x6236dd0a */

_Static_assert(sizeof(unsigned) == 4, "heap-sort requires 32-bit unsigned");

enum {
    ELEMENT_COUNT = 1024,
    SORT_COUNT = 128
};

static unsigned values[ELEMENT_COUNT];
static volatile unsigned seed = 0x3c6ef372u;

int main(void) {
    unsigned state = seed;
    unsigned checksum = 0xbb67ae85u;
    unsigned sort;

    for (sort = 0; sort < SORT_COUNT; ++sort) {
        unsigned index;
        unsigned root;
        unsigned end;

        for (index = 0; index < ELEMENT_COUNT; ++index) {
            state = state * 1664525u + 1013904223u;
            values[index] = state ^ index * 0x9e3779b9u;
        }

        for (root = ELEMENT_COUNT / 2u; root != 0u; --root) {
            unsigned parent = root - 1u;
            unsigned value = values[parent];
            unsigned child = parent * 2u + 1u;

            while (child < ELEMENT_COUNT) {
                if (child + 1u < ELEMENT_COUNT &&
                    values[child] < values[child + 1u]) {
                    ++child;
                }
                if (value >= values[child]) {
                    break;
                }
                values[parent] = values[child];
                parent = child;
                child = parent * 2u + 1u;
            }
            values[parent] = value;
        }

        for (end = ELEMENT_COUNT; end > 1u; --end) {
            unsigned parent = 0;
            unsigned value = values[end - 1u];
            unsigned child = 1;

            values[end - 1u] = values[0];
            while (child < end - 1u) {
                if (child + 1u < end - 1u &&
                    values[child] < values[child + 1u]) {
                    ++child;
                }
                if (value >= values[child]) {
                    break;
                }
                values[parent] = values[child];
                parent = child;
                child = parent * 2u + 1u;
            }
            values[parent] = value;
        }

        for (index = 1; index < ELEMENT_COUNT; ++index) {
            if (values[index - 1u] > values[index]) {
                return 2;
            }
            checksum ^= values[index] + index * 0x85ebca6bu;
            checksum = (checksum << 11) | (checksum >> 21);
        }
        checksum ^= values[0] + sort;
    }

    return (checksum ^ state) != 0x6236dd0au;
}
