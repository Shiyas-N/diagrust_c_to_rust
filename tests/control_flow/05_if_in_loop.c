int count_positives(int n) {
    int count = 0;
    for (int i = 0; i < n; i = i + 1) {
        if (i > 0) {
            count = count + 1;
        }
    }
    return count;
}
