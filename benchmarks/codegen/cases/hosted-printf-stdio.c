/* ccc-benchmark-family: hosted-printf */
/* ccc-benchmark-variant: stdio */
/* ccc-benchmark-scale: 1 */

#include <stdio.h>

int main(void) {
    return printf("%s: %d\n", "hello from ccc", 42) < 0;
}
