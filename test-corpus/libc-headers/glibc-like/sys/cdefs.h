#ifndef CCC_FIXTURE_SYS_CDEFS_H
#define CCC_FIXTURE_SYS_CDEFS_H 1

#if defined __has_attribute
#define __glibc_has_attribute(name) __has_attribute(name)
#else
#define __glibc_has_attribute(name) 0
#endif

#if __GNUC_PREREQ(3, 3) || __glibc_has_attribute(__nothrow__)
#define __THROW __attribute__((__nothrow__))
#else
#define __THROW
#endif

#define __glibc_pragma(text) _Pragma(#text)
#define __glibc_diagnostic_push() __glibc_pragma(GCC diagnostic push)
#define __glibc_diagnostic_pop() __glibc_pragma(GCC diagnostic pop)

#endif
