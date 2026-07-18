#if __GNUC__ != 4 || __GNUC_MINOR__ != 2 || __GNUC_PATCHLEVEL__ != 1
#error "unexpected GNU compatibility identity"
#endif

#include <features.h>
#include <assert.h>

gnu_compatibility_tuple=4.2.1

#if !defined(__STRICT_ANSI__) && defined(__ASSERT_FUNCTION)
selected_assert=system-gnu-macro
#else
selected_assert=unexpected
#endif

#ifdef __STDC_NO_ATOMICS__
selected_c11_atomic_surface=unavailable
#else
selected_c11_atomic_surface=available
#endif

#if !defined(__STDC_NO_ATOMICS__)
selected_core_atomic_surface=unexpected-c11
#elif defined(__ATOMIC_RELAXED) && defined(__ATOMIC_SEQ_CST)
selected_core_atomic_surface=unexpected-atomic-builtin
#elif (__i386 || __amd64 || __powerpc__) && __GNUC__ && \
    defined(__GLIBC__) && defined(__GLIBC_PREREQ) && \
    ((__GNUC__ * 10000 + __GNUC_MINOR__ * 100 + __GNUC_PATCHLEVEL__) >= 40100) && \
    __GLIBC_PREREQ(2, 6)
selected_core_atomic_surface=sync-builtin
#else
selected_core_atomic_surface=unavailable
#endif

#if defined(__x86_64__) && !defined(__ATOMIC_SEQ_CST)
selected_upstream_hdr_atomic_surface=x86-inline-assembly
#else
selected_upstream_hdr_atomic_surface=unexpected
#endif

#define REPORT_BUILTIN(name) \
    REPORT_BUILTIN_I(name, __has_builtin(name))
#define REPORT_BUILTIN_I(name, available) \
    REPORT_BUILTIN_II(name, available)
#define REPORT_BUILTIN_II(name, available) \
    available_builtin_##available=name

REPORT_BUILTIN(__builtin_bswap64)
REPORT_BUILTIN(__builtin_clz)
REPORT_BUILTIN(__builtin_clzl)
REPORT_BUILTIN(__builtin_clzll)
REPORT_BUILTIN(__builtin_ctzll)
REPORT_BUILTIN(__builtin_expect)
REPORT_BUILTIN(__builtin_popcount)
REPORT_BUILTIN(__builtin_popcountll)
REPORT_BUILTIN(__builtin_prefetch)
REPORT_BUILTIN(__sync_add_and_fetch)
REPORT_BUILTIN(__sync_bool_compare_and_swap)
REPORT_BUILTIN(__sync_fetch_and_add)
REPORT_BUILTIN(__sync_lock_test_and_set)
REPORT_BUILTIN(__sync_sub_and_fetch)
REPORT_BUILTIN(__sync_synchronize)
REPORT_BUILTIN(__sync_val_compare_and_swap)
