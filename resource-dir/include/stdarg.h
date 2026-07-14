#if !defined(__need___va_list) && !defined(__need_va_list) && \
    !defined(__need_va_arg) && !defined(__need___va_copy) && \
    !defined(__need_va_copy)
#define __need___va_list
#define __need_va_list
#define __need_va_arg
#define __need___va_copy
#define __need_va_copy
#endif

#if defined(__need___va_list) && !defined(__GNUC_VA_LIST)
#define __GNUC_VA_LIST
typedef __builtin_va_list __gnuc_va_list;
#endif

#if defined(__need_va_list) && !defined(_VA_LIST_DEFINED) && \
    !defined(_VA_LIST) && !defined(_VA_LIST_) && \
    !defined(_VA_LIST_T_H) && !defined(__va_list__)
#define _VA_LIST_DEFINED
#define _VA_LIST
#define _VA_LIST_
#define _VA_LIST_T_H
#define __va_list__
typedef __builtin_va_list va_list;
#endif

#ifdef __need_va_arg
#define va_start(list, last) __builtin_va_start(list, last)
#define va_arg(list, type) __builtin_va_arg(list, type)
#define va_end(list) __builtin_va_end(list)
#endif

#ifdef __need___va_copy
#define __va_copy(destination, source) __builtin_va_copy(destination, source)
#endif

#ifdef __need_va_copy
#define va_copy(destination, source) __builtin_va_copy(destination, source)
#endif

#undef __need___va_list
#undef __need_va_list
#undef __need_va_arg
#undef __need___va_copy
#undef __need_va_copy
