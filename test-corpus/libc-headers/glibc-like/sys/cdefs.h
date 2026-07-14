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

#if __GNUC_PREREQ(2, 96) || __glibc_has_attribute(__pure__)
#define __attribute_pure__ __attribute((__pure__))
#else
#define __attribute_pure__
#endif

#if __GNUC_PREREQ(3, 3) || __glibc_has_attribute(__nonnull__)
#define __nonnull(parameters) __attribute__((__nonnull__ parameters))
#else
#define __nonnull(parameters)
#endif

#if __GNUC_PREREQ(3, 4) || __glibc_has_attribute(__warn_unused_result__)
#define __wur __attribute__((__warn_unused_result__))
#else
#define __wur
#endif

#define __ASMNAME(name) __ASMNAME_INNER(name)
#define __ASMNAME_INNER(name) #name
#define __REDIRECT(name, prototype, alias) \
    name prototype __asm__(__ASMNAME(alias))

#define __glibc_pragma(text) _Pragma(#text)
#define __glibc_diagnostic_push() __glibc_pragma(GCC diagnostic push)
#define __glibc_diagnostic_pop() __glibc_pragma(GCC diagnostic pop)

#endif
