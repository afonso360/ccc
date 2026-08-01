/* ccc-kernel-benchmark: image-stencil */
/* ccc-kernel-work-unit: five-point-stencil */
/* ccc-kernel-work-count: 1016064 */
/* ccc-kernel-expected-result: 0x2c970468 */

_Static_assert(sizeof(unsigned) == 4,
               "image-stencil requires 32-bit unsigned");

enum {
    WIDTH = 128,
    HEIGHT = 128,
    PIXEL_COUNT = WIDTH * HEIGHT,
    PASS_COUNT = 64
};

static unsigned image_a[PIXEL_COUNT];
static unsigned image_b[PIXEL_COUNT];
static volatile unsigned seed = 0x1f123bb5u;

int main(void) {
    unsigned state = seed;
    unsigned checksum = 0x9e3779b9u;
    unsigned index;
    unsigned pass;
    unsigned *final_image;

    for (index = 0; index < PIXEL_COUNT; ++index) {
        state = state * 1664525u + 1013904223u;
        image_a[index] = state & 0xffu;
        image_b[index] = image_a[index];
    }

    for (pass = 0; pass < PASS_COUNT; ++pass) {
        unsigned *source = pass & 1u ? image_b : image_a;
        unsigned *destination = pass & 1u ? image_a : image_b;
        unsigned row;

        for (row = 1; row + 1u < HEIGHT; ++row) {
            unsigned column;

            for (column = 1; column + 1u < WIDTH; ++column) {
                index = row * WIDTH + column;
                destination[index] =
                    (source[index] * 4u + source[index - WIDTH] +
                     source[index + WIDTH] + source[index - 1u] +
                     source[index + 1u]) >> 3;
            }
        }
    }

    final_image = PASS_COUNT & 1u ? image_a : image_b;
    for (index = 0; index < PIXEL_COUNT; ++index) {
        checksum ^= final_image[index] + index * 0x85ebca6bu;
        checksum = (checksum << 3) | (checksum >> 29);
    }

    return (checksum ^ state) != 0x2c970468u;
}
