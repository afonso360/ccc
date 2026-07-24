/* ccc-kernel-benchmark: floating-loop */
/* ccc-kernel-work-unit: floating-addition */
/* ccc-kernel-work-count: 4000000 */
/* ccc-kernel-expected-result: float=1250001.0f,double=1250001.0 */

_Static_assert(sizeof(float) == 4, "floating-loop requires binary32 float");
_Static_assert(sizeof(double) == 8, "floating-loop requires binary64 double");

enum {
    ITERATION_COUNT = 2000000
};

static volatile float float_seed = 1.0f;
static volatile double double_seed = 1.0;
static const float float_steps[4] = {0.25f, 0.5f, 0.75f, 1.0f};
static const double double_steps[4] = {0.25, 0.5, 0.75, 1.0};

int main(void) {
    float float_value = float_seed;
    double double_value = double_seed;
    unsigned iteration;

    for (iteration = 0; iteration < ITERATION_COUNT; ++iteration) {
        float_value += float_steps[iteration & 3u];
        double_value += double_steps[iteration & 3u];
    }

    return float_value != 1250001.0f || double_value != 1250001.0;
}
