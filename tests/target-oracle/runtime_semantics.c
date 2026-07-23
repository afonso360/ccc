#include <setjmp.h>
#include <stdatomic.h>

static jmp_buf saved_environment;
static atomic_schar narrow = ATOMIC_VAR_INIT(3);
static atomic_short medium = ATOMIC_VAR_INIT(10);
static atomic_int ordinary = ATOMIC_VAR_INIT(20);
static atomic_llong wide = ATOMIC_VAR_INIT(100);
static int first;
static int second;
static _Atomic(int *) pointer = &first;

static void resume_through_another_frame(int value) {
    longjmp(saved_environment, value);
}

static int check_native_atomics(void) {
    int expected;

    if (atomic_fetch_add_explicit(&narrow, 2, memory_order_relaxed) != 3)
        return 1;
    if (atomic_fetch_sub_explicit(&medium, 3, memory_order_release) != 10)
        return 2;
    if (__atomic_add_fetch(&ordinary, 7, __ATOMIC_ACQUIRE) != 27)
        return 3;
    if (atomic_exchange_explicit(&wide, 123, memory_order_seq_cst) != 100)
        return 4;

    expected = 27;
    if (!atomic_compare_exchange_strong_explicit(
            &ordinary, &expected, 41, memory_order_acq_rel,
            memory_order_acquire))
        return 5;
    if (expected != 27 || atomic_load(&ordinary) != 41)
        return 6;
    if (atomic_exchange(&pointer, &second) != &first)
        return 7;
    if (atomic_load(&pointer) != &second)
        return 8;
    atomic_thread_fence(memory_order_seq_cst);
    atomic_signal_fence(memory_order_acq_rel);
    return 0;
}

int main(void) {
    int stable = 7;
    volatile int changed = 1;
    int resumed = setjmp(saved_environment);

    if (resumed != 0) {
        if (resumed != 23 || stable != 7 || changed != 9)
            return 20;
        return check_native_atomics();
    }
    changed = 9;
    resume_through_another_frame(23);
    return 21;
}
