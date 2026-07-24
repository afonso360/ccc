/* ccc-kernel-benchmark: integer-loop */
/* ccc-kernel-work-unit: integer-iteration */
/* ccc-kernel-work-count: 4000000 */
/* ccc-kernel-expected-result: 0x2b37aed1 */

_Static_assert(sizeof(unsigned) == 4, "integer-loop requires 32-bit unsigned");

enum {
    WORK_COUNT = 4000000
};

static volatile unsigned seed = 0xdeadbeefu;

int main(void) {
    unsigned value = seed;
    unsigned sum = 0x9e3779b9u;
    unsigned iteration;

    for (iteration = 0; iteration < WORK_COUNT; ++iteration) {
        value += iteration ^ (value >> 7);
        value = (value << 5) | (value >> 27);
        sum += value ^ iteration * 0x85ebca6bu;
    }

    return (value ^ sum) != 0x2b37aed1u;
}
