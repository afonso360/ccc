#ifndef CCC_REDIS_PORTABLE_ASSERT_H
#define CCC_REDIS_PORTABLE_ASSERT_H

#include <features.h>
#define CCC_REDIS_HOSTED_FEATURES_PRIMED 1

#ifndef __STRICT_ANSI__
#define __STRICT_ANSI__ 1
#define CCC_REDIS_RESTORE_GNU_HEADER_MODE 1
#endif

#include_next <assert.h>

#ifdef CCC_REDIS_RESTORE_GNU_HEADER_MODE
#undef CCC_REDIS_RESTORE_GNU_HEADER_MODE
#undef __STRICT_ANSI__
#endif

#undef __ASSERT_FUNCTION
#define __ASSERT_FUNCTION __func__

#define CCC_REDIS_PORTABLE_ASSERT 1

#endif
