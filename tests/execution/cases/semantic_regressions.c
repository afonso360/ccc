struct Pair {
    int left;
    int right;
};

static const struct Pair source = {20, 22};

extern int values[];
extern int values[2];

enum Large {
    LARGE = 3000000000
};

int adjust();
int adjust(int value);

int adjust(int value) { return value; }

int values[] = {3, 4};

int main(void) {
    struct Pair copy = source;
    int unsigned_enum = LARGE > 2147483647;
    int signed_character = '\xff' == -1;

    return copy.left + copy.right + values[0] + values[1] + adjust(3)
           + unsigned_enum + signed_character;
}
