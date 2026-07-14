#if !defined(__need_ptrdiff_t) && !defined(__need_size_t) \
    && !defined(__need_wchar_t) && !defined(__need_NULL) \
    && !defined(__need_max_align_t) && !defined(__need_offsetof)
#define __CCC_STDDEF_ALL 1
#endif

#if (defined(__CCC_STDDEF_ALL) || defined(__need_ptrdiff_t)) \
    && !defined(__CCC_PTRDIFF_T_DEFINED)
#define __CCC_PTRDIFF_T_DEFINED 1
typedef __PTRDIFF_TYPE__ ptrdiff_t;
#endif

#if (defined(__CCC_STDDEF_ALL) || defined(__need_size_t)) \
    && !defined(__CCC_SIZE_T_DEFINED)
#define __CCC_SIZE_T_DEFINED 1
typedef __SIZE_TYPE__ size_t;
#endif

#if (defined(__CCC_STDDEF_ALL) || defined(__need_wchar_t)) \
    && !defined(__CCC_WCHAR_T_DEFINED)
#define __CCC_WCHAR_T_DEFINED 1
typedef __WCHAR_TYPE__ wchar_t;
#endif

#if (defined(__CCC_STDDEF_ALL) || defined(__need_NULL)) \
    && !defined(NULL)
#define NULL ((void *)0)
#endif

#if (defined(__CCC_STDDEF_ALL) || defined(__need_max_align_t)) \
    && !defined(__CCC_MAX_ALIGN_T_DEFINED)
#define __CCC_MAX_ALIGN_T_DEFINED 1
typedef struct {
    long long __ccc_max_align_ll;
    long double __ccc_max_align_ld;
} max_align_t;
#endif

#if defined(__CCC_STDDEF_ALL) || defined(__need_offsetof)
#define offsetof(type, member) __builtin_offsetof(type, member)
#endif

#undef __need_ptrdiff_t
#undef __need_size_t
#undef __need_wchar_t
#undef __need_NULL
#undef __need_max_align_t
#undef __need_offsetof
#undef __CCC_STDDEF_ALL
