void alias_test(int *a, int *b) {
    int x = *a;
    *b = x + 1;
    *a = *b * 2;
}
