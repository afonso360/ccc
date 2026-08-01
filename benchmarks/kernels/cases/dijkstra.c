/* ccc-kernel-benchmark: dijkstra */
/* ccc-kernel-work-unit: 64-node-shortest-path */
/* ccc-kernel-work-count: 512 */
/* ccc-kernel-expected-result: 0xced1bd1d */

_Static_assert(sizeof(unsigned) == 4, "dijkstra requires 32-bit unsigned");

enum {
    NODE_COUNT = 64,
    EDGE_COUNT = NODE_COUNT * NODE_COUNT,
    PATH_COUNT = 512
};

static unsigned weights[EDGE_COUNT];
static unsigned distances[NODE_COUNT];
static unsigned settled[NODE_COUNT];
static volatile unsigned seed = 0xa54ff53au;

int main(void) {
    unsigned state = seed;
    unsigned checksum = 0x510e527fu;
    unsigned from;
    unsigned to;
    unsigned path;

    for (from = 0; from < NODE_COUNT; ++from) {
        for (to = 0; to < NODE_COUNT; ++to) {
            state = state * 1103515245u + 12345u;
            weights[from * NODE_COUNT + to] =
                from == to ? 0u : 1u + (state & 31u);
        }
    }

    for (path = 0; path < PATH_COUNT; ++path) {
        unsigned start;
        unsigned step;

        state = state * 1664525u + 1013904223u;
        start = state & (NODE_COUNT - 1u);
        for (to = 0; to < NODE_COUNT; ++to) {
            distances[to] = weights[start * NODE_COUNT + to];
            settled[to] = 0u;
        }
        distances[start] = 0u;

        for (step = 0; step < NODE_COUNT; ++step) {
            unsigned candidate_node = NODE_COUNT;
            unsigned candidate_distance = 0xffffffffu;

            for (to = 0; to < NODE_COUNT; ++to) {
                if (!settled[to] && distances[to] < candidate_distance) {
                    candidate_node = to;
                    candidate_distance = distances[to];
                }
            }
            if (candidate_node == NODE_COUNT) {
                return 2;
            }
            settled[candidate_node] = 1u;
            for (to = 0; to < NODE_COUNT; ++to) {
                unsigned distance = candidate_distance +
                    weights[candidate_node * NODE_COUNT + to];

                if (!settled[to] && distance < distances[to]) {
                    distances[to] = distance;
                }
            }
        }

        for (to = 0; to < NODE_COUNT; ++to) {
            checksum ^= distances[to] + to * 0x9e3779b9u + path;
            checksum = (checksum << 5) | (checksum >> 27);
        }
    }

    return (checksum ^ state) != 0xced1bd1du;
}
