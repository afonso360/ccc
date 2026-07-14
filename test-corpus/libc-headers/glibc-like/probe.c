#include <sys/types.h>

#if !defined(__GLIBC__) || !__GNUC_PREREQ(4, 2)
#error the hosted-header compatibility path was not selected
#endif

#if !defined(__THROW)
#error the declaration compatibility macro is missing
#endif

fixture_record_t hosted_header_record;
int hosted_header_preprocessing_sentinel;
