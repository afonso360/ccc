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
    && !__has_builtin(__builtin_ctz) && __has_builtin(__builtin_ctzll)
count_bit_builtin_registry=clz-clzll-ctzll-without-ctz
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
selected_count_bits=generic-c
#else
selected_assembly=enabled
selected_count_bits=gnu-builtins
#endif

#if defined(NO_PREFETCH)
selected_prefetch=disabled
#else
selected_prefetch=compiler-specific
#endif

#if MEM_FORCE_MEMORY_ACCESS == 0
selected_zstd_unaligned_access=memcpy
#else
selected_zstd_unaligned_access=compiler-specific
#endif

#if XXH_FORCE_MEMORY_ACCESS == 0
selected_xxhash_unaligned_access=memcpy
#else
selected_xxhash_unaligned_access=compiler-specific
#endif

#if CCC_ZSTD_LIBC_MEMORY_DEPS == 1
selected_memory_dependencies=libc
#else
selected_memory_dependencies=unexpected
#endif

#if CCC_ZSTD_PORTABLE_ASSERT == 1 && !defined(__STRICT_ANSI__)
selected_assert=standard-c-macro-gnu-mode-restored
#else
selected_assert=unexpected
#endif

#if CCC_ZSTD_GNU_FEATURES_PRIMED == 1 && defined(_GNU_SOURCE) \
    && defined(__USE_GNU) && defined(__USE_MISC) \
    && defined(__USE_XOPEN2K8) && !defined(__STRICT_ANSI__)
selected_host_features=glibc-gnu
#else
selected_host_features=unexpected
#endif
