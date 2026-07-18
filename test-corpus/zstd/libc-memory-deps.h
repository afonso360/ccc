#ifndef CCC_ZSTD_LIBC_MEMORY_DEPS_H
#define CCC_ZSTD_LIBC_MEMORY_DEPS_H

#ifndef _GNU_SOURCE
#define _GNU_SOURCE
#endif

#include <limits.h>
#include <stddef.h>
#include <string.h>
#include <assert.h>

#define ZSTD_DEPS_COMMON
#define ZSTD_memcpy(destination, source, length) \
    memcpy((destination), (source), (length))
#define ZSTD_memmove(destination, source, length) \
    memmove((destination), (source), (length))
#define ZSTD_memset(pointer, value, length) \
    memset((pointer), (value), (length))

#define CCC_ZSTD_LIBC_MEMORY_DEPS 1

#endif
