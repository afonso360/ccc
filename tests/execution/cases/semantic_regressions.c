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
    const struct Pair qualified_copy = copy;
    int unsigned_enum = LARGE > 2147483647;
    int signed_character = '\xff' == -1;

    return qualified_copy.left + qualified_copy.right + values[0] + values[1] + adjust(3)
           + unsigned_enum + signed_character;
}
