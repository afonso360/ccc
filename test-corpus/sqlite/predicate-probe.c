#if !defined(__GNUC__) || !defined(__GNUC_MINOR__) ||                         \
    !defined(__GNUC_PATCHLEVEL__)
#error "CCC's GNU compatibility identity is incomplete"
#endif

gnu_compatibility = __GNUC__.__GNUC_MINOR__.__GNUC_PATCHLEVEL__

#if __has_builtin(__sync_synchronize)
selected_builtin=__sync_synchronize
#endif

#if __has_builtin(__builtin_nanf)
available_hosted_builtin=__builtin_nanf
#endif

#if __has_builtin(__builtin_inff)
available_hosted_builtin=__builtin_inff
#endif

#if __GNUC__ * 1000000 + __GNUC_MINOR__ * 1000 + __GNUC_PATCHLEVEL__ >= 4007000 || \
    __has_extension(c_atomic)
selected_builtin=__atomic_load_n
selected_builtin=__atomic_store_n
#endif

#if __GNUC__ * 1000000 + __GNUC_MINOR__ * 1000 + __GNUC_PATCHLEVEL__ >= 4003000
selected_builtin=__builtin_bswap32
#endif

#if __GNUC__ * 1000000 + __GNUC_MINOR__ * 1000 + __GNUC_PATCHLEVEL__ >= 5004000
selected_builtin=__builtin_add_overflow
selected_builtin=__builtin_clzll
#endif
