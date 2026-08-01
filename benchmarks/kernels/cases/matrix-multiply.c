/* ccc-kernel-benchmark: matrix-multiply */
/* ccc-kernel-work-unit: unsigned-multiply-accumulate */
/* ccc-kernel-work-count: 1048576 */
/* ccc-kernel-expected-result: 0xfe6cbaf0 */

_Static_assert(sizeof(unsigned) == 4,
               "matrix-multiply requires 32-bit unsigned");

enum {
    SIDE = 32,
    CELL_COUNT = SIDE * SIDE,
    PASS_COUNT = 32
};

static unsigned left_matrix[CELL_COUNT];
static unsigned right_matrix[CELL_COUNT];
static unsigned product_matrix[CELL_COUNT];
static volatile unsigned seed = 0x8badf00du;

int main(void) {
    unsigned state = seed;
    unsigned checksum = 0x243f6a88u;
    unsigned pass;
    unsigned index;

    for (index = 0; index < CELL_COUNT; ++index) {
        state = state * 1103515245u + 12345u;
        left_matrix[index] = state ^ index * 0x9e3779b9u;
        state = state * 1103515245u + 12345u;
        right_matrix[index] = state + index * 0x7f4a7c15u;
    }

    for (pass = 0; pass < PASS_COUNT; ++pass) {
        unsigned row;

        for (row = 0; row < SIDE; ++row) {
            unsigned column;

            for (column = 0; column < SIDE; ++column) {
                unsigned inner;
                unsigned sum = pass * 0x85ebca6bu;

                for (inner = 0; inner < SIDE; ++inner) {
                    sum += left_matrix[row * SIDE + inner] *
                           right_matrix[inner * SIDE + column];
                }
                product_matrix[row * SIDE + column] = sum;
            }
        }

        for (index = 0; index < CELL_COUNT; ++index) {
            unsigned product = product_matrix[index];

            left_matrix[index] +=
                product_matrix[(index * 17u) & (CELL_COUNT - 1u)] ^ state;
            right_matrix[index] ^=
                product_matrix[(index * 29u) & (CELL_COUNT - 1u)] + index;
            checksum ^= product + index * 0xc2b2ae35u;
            checksum = (checksum << 7) | (checksum >> 25);
        }
        state = (state << 9) | (state >> 23);
        state += checksum;
    }

    return (checksum ^ state) != 0xfe6cbaf0u;
}
