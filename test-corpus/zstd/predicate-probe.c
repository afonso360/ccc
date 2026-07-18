#ifndef _GNU_SOURCE
#define _GNU_SOURCE
#endif

#include <features.h>
#include <assert.h>

#if __GNUC__ == 4 && __GNUC_MINOR__ == 2 && __GNUC_PATCHLEVEL__ == 1
gnu_compatibility_tuple=4.2.1
#else
gnu_compatibility_tuple=unexpected
#endif

#if defined(__x86_64__) && defined(__LP64__)
selected_data_model=x86_64-lp64
#else
selected_data_model=unexpected
#endif

#if __has_builtin(__builtin_expect)
selected_builtin=__builtin_expect
#else
selected_builtin=none
#endif

#if __has_builtin(__builtin_clz) && __has_builtin(__builtin_clzll) \
    && __has_builtin(__builtin_ctz) && __has_builtin(__builtin_ctzll)
count_bit_builtin_registry=clz-clzll-ctz-ctzll
#else
count_bit_builtin_registry=unexpected
#endif

#if !__has_builtin(__builtin_bswap32) && __has_builtin(__builtin_bswap64) \
    && __has_builtin(__builtin_prefetch) \
    && !__has_builtin(__builtin_unreachable) \
    && !__has_builtin(__builtin_assume) \
    && !__has_builtin(__builtin_rotateleft32) \
    && !__has_builtin(__builtin_rotateleft64) \
    && !__has_builtin(__builtin_altivec_vmuleuw) \
    && !__has_builtin(__builtin_altivec_vmulouw)
additional_builtin_registry=bswap64-prefetch-only
#else
additional_builtin_registry=unexpected
#endif

#if defined(ZSTD_DISABLE_ASM)
selected_assembly=disabled
#else
selected_assembly=enabled
#endif
selected_count_bits=gnu-builtins

selected_prefetch=compiler-builtin

#if MEM_FORCE_MEMORY_ACCESS == 0
selected_zstd_unaligned_access=memcpy
#else
selected_zstd_unaligned_access=unexpected
#endif

#if XXH_FORCE_MEMORY_ACCESS == 0
selected_xxhash_unaligned_access=memcpy
#else
selected_xxhash_unaligned_access=unexpected
#endif

#if __has_builtin(__builtin_memcpy) && __has_builtin(__builtin_memmove) \
    && __has_builtin(__builtin_memset)
selected_memory_dependencies=compiler-builtins
#else
selected_memory_dependencies=unexpected
#endif

#if !defined(__STRICT_ANSI__) && defined(__ASSERT_FUNCTION)
selected_assert=system-gnu-macro
#else
selected_assert=unexpected
#endif

#if defined(_GNU_SOURCE) && defined(__USE_GNU) && defined(__USE_MISC) \
    && defined(__USE_XOPEN2K8) && !defined(__STRICT_ANSI__)
selected_host_features=glibc-gnu
#else
selected_host_features=unexpected
#endif
