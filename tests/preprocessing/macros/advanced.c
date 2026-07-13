#define STRINGIZE_INNER(value) #value
#define STRINGIZE(value) STRINGIZE_INNER(value)
#define PASTE_INNER(left, right) left ## right
#define PASTE(left, right) PASTE_INNER(left, right)
#define SUM(first, ...) ((first) + (__VA_ARGS__))
#define OPTIONAL(prefix, ...) prefix, ## __VA_ARGS__

int PASTE(ma, in)(void) {
    const char *name = STRINGIZE(PASTE(an, swer));
    return SUM(40, 2) + (name[0] == 'a') - 1;
}
