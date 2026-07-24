/* ccc-kernel-benchmark: aggregate-copy */
/* ccc-kernel-work-unit: 32-byte-struct-copy */
/* ccc-kernel-work-count: 1000000 */
/* ccc-kernel-expected-result: 0x294ffa8f */

_Static_assert(sizeof(unsigned) == 4, "aggregate-copy requires 32-bit unsigned");

enum {
    WORD_COUNT = 8,
    BLOCK_COUNT = 64,
    WORK_COUNT = 1000000
};

struct Block {
    unsigned words[WORD_COUNT];
};

_Static_assert(sizeof(struct Block) == 32,
               "aggregate-copy requires a 32-byte Block");

static struct Block sources[BLOCK_COUNT];
static struct Block destinations[BLOCK_COUNT];
static volatile unsigned seed = 0x3c6ef372u;

int main(void) {
    unsigned state = seed;
    unsigned checksum = 0xa54ff53au;
    unsigned block;
    unsigned word;
    unsigned iteration;

    for (block = 0; block < BLOCK_COUNT; ++block) {
        for (word = 0; word < WORD_COUNT; ++word) {
            sources[block].words[word] =
                state + block * 0x9e3779b9u + word * 0x7f4a7c15u;
        }
    }

    for (iteration = 0; iteration < WORK_COUNT; ++iteration) {
        unsigned source_index;
        unsigned destination_index;
        unsigned word_index;

        state = state * 1103515245u + 12345u;
        source_index = state & (BLOCK_COUNT - 1u);
        destination_index = (state >> 10) & (BLOCK_COUNT - 1u);
        word_index = (state >> 22) & (WORD_COUNT - 1u);
        sources[source_index].words[word_index] += state ^ iteration;
        destinations[destination_index] = sources[source_index];
        checksum += destinations[destination_index].words[
            (word_index + 3u) & (WORD_COUNT - 1u)];
        checksum = (checksum << 9) | (checksum >> 23);
    }

    for (block = 0; block < BLOCK_COUNT; ++block) {
        for (word = 0; word < WORD_COUNT; ++word) {
            checksum ^= destinations[block].words[word] +
                        block * 0x85ebca6bu + word;
            checksum = (checksum << 3) | (checksum >> 29);
        }
    }

    return (checksum ^ state) != 0x294ffa8fu;
}
