#ifndef __CCC_SYS_CDEFS_WRAPPER_H
#define __CCC_SYS_CDEFS_WRAPPER_H

#include_next <sys/cdefs.h>

#if defined(__CCC__) && defined(__APPLE__) && defined(__aarch64__)
/*
 * Apple's headers use these spellings for small implementation functions.
 * CCC does not promise to honor always_inline, so the SDK's GCC-compatible
 * extern-inline branch would otherwise publish one definition per translation
 * unit.  Use the SDK's documented static-inline fallback: it preserves each
 * header implementation without claiming optimizer-driven inlining.
 */
#undef __header_inline
#define __header_inline static __inline
#undef __header_always_inline
#define __header_always_inline static __inline
#endif

#endif
