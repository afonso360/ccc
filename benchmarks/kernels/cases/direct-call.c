/* ccc-kernel-benchmark: direct-call */
/* ccc-kernel-work-unit: leaf-call */
/* ccc-kernel-work-count: 4000000 */
/* ccc-kernel-expected-result: 0x99b0c920 */

_Static_assert(sizeof(unsigned) == 4, "direct-call requires 32-bit unsigned");

enum {
    WORK_COUNT = 4000000
};

static volatile unsigned seed = 0x243f6a88u;

static unsigned mix(unsigned value) {
    value ^= value >> 16;
    value *= 0x7feb352du;
    value ^= value >> 15;
    value *= 0x846ca68bu;
    return value ^ (value >> 16);
}

int main(void) {
    unsigned value = seed;
    unsigned iteration;

    for (iteration = 0; iteration < WORK_COUNT; ++iteration) {
        value = mix(value + iteration + 0x9e3779b9u);
    }

    return value != 0x99b0c920u;
}
