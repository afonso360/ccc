/* ccc-benchmark-family: hosted-header */
/* ccc-benchmark-variant: stdio */
/* ccc-benchmark-scale: 1 */

#include <stdio.h>

int main(void) {
    return fputs("hello from ccc\n", stdout) < 0;
}
