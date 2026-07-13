#ifndef CCC_FIXTURE_FEATURES_H
#define CCC_FIXTURE_FEATURES_H 1

#define __GLIBC__ 2
#define __GLIBC_MINOR__ 39
#define __GLIBC_PREREQ(major, minor) \
    ((__GLIBC__ << 16) + __GLIBC_MINOR__ >= ((major) << 16) + (minor))

#ifdef __GNUC__
#define __GNUC_PREREQ(major, minor) \
    ((__GNUC__ << 16) + __GNUC_MINOR__ >= ((major) << 16) + (minor))
#else
#define __GNUC_PREREQ(major, minor) 0
#endif

#include <sys/cdefs.h>

#endif
