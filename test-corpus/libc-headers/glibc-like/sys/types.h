#ifndef CCC_FIXTURE_SYS_TYPES_H
#define CCC_FIXTURE_SYS_TYPES_H 1

#include <features.h>

typedef unsigned long int size_t;
typedef long int ssize_t;
__extension__ typedef __signed__ long long int fixture_int64_t;

typedef struct {
    int tag;
    unsigned long payload;
} fixture_record_t
    __attribute__((__aligned__(__alignof(long int))));

extern ssize_t fixture_read(int, void *__restrict, size_t)
    __THROW __nonnull((2)) __wur;
extern int fixture_compare(__const void *__restrict__,
                           __const__ void *__restrict)
    __attribute_pure__;
extern __volatile__ int fixture_generation;
extern __typeof(fixture_read) fixture_read_type_alias;
extern __typeof__(fixture_read) fixture_read_spelled_alias
    __asm("fixture_read_impl") __THROW;
extern ssize_t __REDIRECT(fixture_read_redirect,
                          (int, void *__restrict, size_t),
                          fixture_read_impl) __THROW;

extern __inline__ __attribute__((__gnu_inline__)) int
fixture_identity(int value)
{
    return value;
}

#endif
