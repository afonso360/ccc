#ifndef CCC_FIXTURE_SYS_TYPES_H
#define CCC_FIXTURE_SYS_TYPES_H 1

#include <features.h>

typedef unsigned long int size_t;
typedef long int ssize_t;

extern ssize_t fixture_read(int, void *, size_t) __THROW;

#endif
