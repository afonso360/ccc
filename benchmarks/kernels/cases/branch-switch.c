/* ccc-kernel-benchmark: branch-switch */
/* ccc-kernel-work-unit: switch-branch-iteration */
/* ccc-kernel-work-count: 4000000 */
/* ccc-kernel-expected-result: 0x2f58cc08 */

_Static_assert(sizeof(unsigned) == 4, "branch-switch requires 32-bit unsigned");

enum {
    WORK_COUNT = 4000000
};

static volatile unsigned seed = 0x31415926u;

int main(void) {
    unsigned state = seed;
    unsigned accumulator = 0;
    unsigned iteration;

    for (iteration = 0; iteration < WORK_COUNT; ++iteration) {
        state = state * 1664525u + 1013904223u;
        switch (state >> 29) {
            case 0:
                accumulator += state;
                break;
            case 1:
                accumulator ^= state;
                break;
            case 2:
                accumulator += state >> 3;
                break;
            case 3:
                accumulator -= state;
                break;
            case 4:
                accumulator ^= (state << 7) | (state >> 25);
                break;
            case 5:
                accumulator += state | 1u;
                break;
            case 6:
                accumulator -= state >> 11;
                break;
            default:
                accumulator ^= ~state;
                break;
        }
        if (state & 1u) {
            accumulator = (accumulator << 3) | (accumulator >> 29);
        } else {
            accumulator ^= state >> 5;
        }
    }

    return accumulator != 0x2f58cc08u;
}
