#if __GNUC__ == 4 && __GNUC_MINOR__ == 2 && __GNUC_PATCHLEVEL__ == 1
gnu_compatibility_tuple=4.2.1
#else
gnu_compatibility_tuple=unexpected
#endif

#if defined(__GNUC__) && !defined(LUA_NOBUILTIN)
selected_builtin=__builtin_expect
#else
selected_builtin=none
#endif

#if defined(__GNUC__)
selected_computed_goto=luaV_execute-jump-table
selected_attribute=noreturn
selected_operator=__extension__
#else
selected_computed_goto=none
#endif

#if defined(__GNUC__) && ((__GNUC__ * 100 + __GNUC_MINOR__) >= 302) && \
    (defined(__ELF__) || defined(__MACH__))
selected_attribute=visibility-internal
#else
selected_attribute_visibility=none
#endif
