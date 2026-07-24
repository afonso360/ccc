/* ccc-kernel-benchmark: atomic-rmw */
/* ccc-kernel-work-unit: atomic-operation */
/* ccc-kernel-work-count: 4000000 */
/* ccc-kernel-expected-result: 0xf133366f */

#include <stdatomic.h>

_Static_assert(sizeof(unsigned) == 4, "atomic-rmw requires 32-bit unsigned");
_Static_assert(ATOMIC_INT_LOCK_FREE == 2,
               "atomic-rmw requires lock-free unsigned operations");

enum {
    ITERATION_COUNT = 1000000
};

static atomic_uint shared_state = ATOMIC_VAR_INIT(0x243f6a88u);

int main(void) {
    unsigned checksum = 0x85a308d3u;
    unsigned final_state = 0x243f6a88u;
    unsigned iteration;

    for (iteration = 0; iteration < ITERATION_COUNT; ++iteration) {
        unsigned increment = iteration * 0x9e3779b9u | 1u;
        unsigned old_add = atomic_fetch_add_explicit(
            &shared_state, increment, memory_order_relaxed);
        unsigned xor_operand = old_add ^ 0x7f4a7c15u;
        unsigned old_xor = atomic_fetch_xor_explicit(
            &shared_state, xor_operand, memory_order_acq_rel);
        unsigned expected = old_xor ^ xor_operand;
        unsigned desired =
            ((expected << 5) | (expected >> 27)) + iteration;
        unsigned observed;

        if (!atomic_compare_exchange_strong_explicit(
                &shared_state,
                &expected,
                desired,
                memory_order_release,
                memory_order_relaxed)) {
            return 2;
        }
        observed = atomic_load_explicit(&shared_state, memory_order_acquire);
        final_state = observed;
        checksum ^= old_add + ((old_xor << 11) | (old_xor >> 21)) + observed;
        checksum = (checksum << 3) | (checksum >> 29);
    }

    return (checksum ^ final_state) != 0xf133366fu;
}
