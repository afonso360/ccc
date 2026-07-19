#ifndef __CCC_STDATOMIC_H
#define __CCC_STDATOMIC_H

typedef enum {
    memory_order_relaxed = 0,
    memory_order_consume = 1,
    memory_order_acquire = 2,
    memory_order_release = 3,
    memory_order_acq_rel = 4,
    memory_order_seq_cst = 5
} memory_order;

#define __ATOMIC_RELAXED 0
#define __ATOMIC_CONSUME 1
#define __ATOMIC_ACQUIRE 2
#define __ATOMIC_RELEASE 3
#define __ATOMIC_ACQ_REL 4
#define __ATOMIC_SEQ_CST 5

typedef _Atomic(_Bool) atomic_bool;
typedef _Atomic(char) atomic_char;
typedef _Atomic(signed char) atomic_schar;
typedef _Atomic(unsigned char) atomic_uchar;
typedef _Atomic(short) atomic_short;
typedef _Atomic(unsigned short) atomic_ushort;
typedef _Atomic(int) atomic_int;
typedef _Atomic(unsigned int) atomic_uint;
typedef _Atomic(long) atomic_long;
typedef _Atomic(unsigned long) atomic_ulong;
typedef _Atomic(long long) atomic_llong;
typedef _Atomic(unsigned long long) atomic_ullong;
typedef _Atomic(__CHAR16_TYPE__) atomic_char16_t;
typedef _Atomic(__CHAR32_TYPE__) atomic_char32_t;
typedef _Atomic(__WCHAR_TYPE__) atomic_wchar_t;

typedef _Atomic(__INT_LEAST8_TYPE__) atomic_int_least8_t;
typedef _Atomic(__UINT_LEAST8_TYPE__) atomic_uint_least8_t;
typedef _Atomic(__INT_LEAST16_TYPE__) atomic_int_least16_t;
typedef _Atomic(__UINT_LEAST16_TYPE__) atomic_uint_least16_t;
typedef _Atomic(__INT_LEAST32_TYPE__) atomic_int_least32_t;
typedef _Atomic(__UINT_LEAST32_TYPE__) atomic_uint_least32_t;
typedef _Atomic(__INT_LEAST64_TYPE__) atomic_int_least64_t;
typedef _Atomic(__UINT_LEAST64_TYPE__) atomic_uint_least64_t;
typedef _Atomic(__INT_FAST8_TYPE__) atomic_int_fast8_t;
typedef _Atomic(__UINT_FAST8_TYPE__) atomic_uint_fast8_t;
typedef _Atomic(__INT_FAST16_TYPE__) atomic_int_fast16_t;
typedef _Atomic(__UINT_FAST16_TYPE__) atomic_uint_fast16_t;
typedef _Atomic(__INT_FAST32_TYPE__) atomic_int_fast32_t;
typedef _Atomic(__UINT_FAST32_TYPE__) atomic_uint_fast32_t;
typedef _Atomic(__INT_FAST64_TYPE__) atomic_int_fast64_t;
typedef _Atomic(__UINT_FAST64_TYPE__) atomic_uint_fast64_t;
typedef _Atomic(__INTPTR_TYPE__) atomic_intptr_t;
typedef _Atomic(__UINTPTR_TYPE__) atomic_uintptr_t;
typedef _Atomic(__SIZE_TYPE__) atomic_size_t;
typedef _Atomic(__PTRDIFF_TYPE__) atomic_ptrdiff_t;
typedef _Atomic(__INTMAX_TYPE__) atomic_intmax_t;
typedef _Atomic(__UINTMAX_TYPE__) atomic_uintmax_t;

typedef atomic_bool atomic_flag;

#define ATOMIC_BOOL_LOCK_FREE 2
#define ATOMIC_CHAR_LOCK_FREE 2
#define ATOMIC_CHAR16_T_LOCK_FREE 2
#define ATOMIC_CHAR32_T_LOCK_FREE 2
#define ATOMIC_WCHAR_T_LOCK_FREE 2
#define ATOMIC_SHORT_LOCK_FREE 2
#define ATOMIC_INT_LOCK_FREE 2
#define ATOMIC_LONG_LOCK_FREE 2
#define ATOMIC_LLONG_LOCK_FREE 2
#define ATOMIC_POINTER_LOCK_FREE 2

#define ATOMIC_FLAG_INIT 0
#define ATOMIC_VAR_INIT(value) (value)

#define kill_dependency(value) (value)

#define atomic_init(object, desired) \
    __atomic_store_n((object), (desired), __ATOMIC_SEQ_CST)
#define atomic_is_lock_free(object) __ccc_atomic_is_lock_free((object))

#define atomic_store(object, desired) \
    __atomic_store_n((object), (desired), __ATOMIC_SEQ_CST)
#define atomic_store_explicit(object, desired, order) \
    __atomic_store_n((object), (desired), (order))
#define atomic_load(object) __atomic_load_n((object), __ATOMIC_SEQ_CST)
#define atomic_load_explicit(object, order) __atomic_load_n((object), (order))
#define atomic_exchange(object, desired) \
    __atomic_exchange_n((object), (desired), __ATOMIC_SEQ_CST)
#define atomic_exchange_explicit(object, desired, order) \
    __atomic_exchange_n((object), (desired), (order))

#define atomic_compare_exchange_strong(object, expected, desired)       \
    __atomic_compare_exchange_n((object), (expected), (desired), 0,     \
                                __ATOMIC_SEQ_CST, __ATOMIC_SEQ_CST)
#define atomic_compare_exchange_weak(object, expected, desired)         \
    __atomic_compare_exchange_n((object), (expected), (desired), 1,     \
                                __ATOMIC_SEQ_CST, __ATOMIC_SEQ_CST)
#define atomic_compare_exchange_strong_explicit(                        \
    object, expected, desired, success, failure)                        \
    __atomic_compare_exchange_n((object), (expected), (desired), 0,     \
                                (success), (failure))
#define atomic_compare_exchange_weak_explicit(                          \
    object, expected, desired, success, failure)                        \
    __atomic_compare_exchange_n((object), (expected), (desired), 1,     \
                                (success), (failure))

#define atomic_fetch_add(object, operand) \
    __atomic_fetch_add((object), (operand), __ATOMIC_SEQ_CST)
#define atomic_fetch_add_explicit(object, operand, order) \
    __atomic_fetch_add((object), (operand), (order))
#define atomic_fetch_sub(object, operand) \
    __atomic_fetch_sub((object), (operand), __ATOMIC_SEQ_CST)
#define atomic_fetch_sub_explicit(object, operand, order) \
    __atomic_fetch_sub((object), (operand), (order))
#define atomic_fetch_or(object, operand) \
    __atomic_fetch_or((object), (operand), __ATOMIC_SEQ_CST)
#define atomic_fetch_or_explicit(object, operand, order) \
    __atomic_fetch_or((object), (operand), (order))
#define atomic_fetch_xor(object, operand) \
    __atomic_fetch_xor((object), (operand), __ATOMIC_SEQ_CST)
#define atomic_fetch_xor_explicit(object, operand, order) \
    __atomic_fetch_xor((object), (operand), (order))
#define atomic_fetch_and(object, operand) \
    __atomic_fetch_and((object), (operand), __ATOMIC_SEQ_CST)
#define atomic_fetch_and_explicit(object, operand, order) \
    __atomic_fetch_and((object), (operand), (order))

#define atomic_flag_test_and_set(object) \
    __atomic_exchange_n((object), 1, __ATOMIC_SEQ_CST)
#define atomic_flag_test_and_set_explicit(object, order) \
    __atomic_exchange_n((object), 1, (order))
#define atomic_flag_clear(object) \
    __atomic_store_n((object), 0, __ATOMIC_SEQ_CST)
#define atomic_flag_clear_explicit(object, order) \
    __atomic_store_n((object), 0, (order))

#define atomic_thread_fence(order) __atomic_thread_fence((order))
#define atomic_signal_fence(order) __atomic_signal_fence((order))

#endif
