#if __GNUC__ == 4 && __GNUC_MINOR__ == 2 && __GNUC_PATCHLEVEL__ == 1
gnu_compatibility_tuple=4.2.1
#else
gnu_compatibility_tuple=unexpected
#endif

#ifdef __GNUC__
selected_attribute=noreturn
selected_keyword=__inline__
selected_integer_type=unsigned-long-long
#else
selected_attribute=none
selected_keyword=macro-empty-inline
selected_integer_type=unsigned-int
#endif
