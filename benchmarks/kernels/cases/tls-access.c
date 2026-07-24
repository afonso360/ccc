/* ccc-kernel-benchmark: tls-access */
/* ccc-kernel-work-unit: thread-local-update */
/* ccc-kernel-work-count: 1000000 */
/* ccc-kernel-expected-result: 0x19138677 */

_Static_assert(sizeof(unsigned) == 4, "tls-access requires 32-bit unsigned");

enum {
    WORK_COUNT = 1000000
};

static _Thread_local volatile unsigned tls_state = 0x517cc1b7u;

int main(void) {
    unsigned checksum = 0x9e3779b9u;
    unsigned iteration;

    for (iteration = 0; iteration < WORK_COUNT; ++iteration) {
        unsigned value;

        tls_state = tls_state * 1664525u + 1013904223u;
        value = tls_state;
        checksum ^= value + iteration * 0x85ebca6bu;
        checksum = (checksum << 7) | (checksum >> 25);
    }

    return (checksum ^ tls_state) != 0x19138677u;
}
