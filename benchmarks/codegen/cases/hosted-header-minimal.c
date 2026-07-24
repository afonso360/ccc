/* ccc-benchmark-family: hosted-header */
/* ccc-benchmark-variant: minimal */
/* ccc-benchmark-scale: 1 */

struct ccc_benchmark_file;

extern struct ccc_benchmark_file *stdout;
extern int fputs(const char *message, struct ccc_benchmark_file *stream);

int main(void) {
    return fputs("hello from ccc\n", stdout) < 0;
}
