#ifndef CCC_ZSTD_PORTABLE_ASSERT_H
#define CCC_ZSTD_PORTABLE_ASSERT_H

#include <features.h>

#if defined(__USE_GNU) && defined(__USE_MISC) && defined(__USE_XOPEN2K8)
#define CCC_ZSTD_GNU_FEATURES_PRIMED 1
#else
#define CCC_ZSTD_GNU_FEATURES_PRIMED 0
#endif

#ifndef __STRICT_ANSI__
#define __STRICT_ANSI__ 1
#define CCC_ZSTD_RESTORE_GNU_HEADER_MODE 1
#endif

#include_next <assert.h>

#ifdef CCC_ZSTD_RESTORE_GNU_HEADER_MODE
#undef CCC_ZSTD_RESTORE_GNU_HEADER_MODE
#undef __STRICT_ANSI__
#endif

#undef __ASSERT_FUNCTION
#define __ASSERT_FUNCTION __func__

#define CCC_ZSTD_PORTABLE_ASSERT 1

#endif
