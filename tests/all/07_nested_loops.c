int sum_matrix_diagonal(int n) {
    int i = 0;
    int j = 0;
    int total = 0;
    for (i = 0; i < n; i++) {
        for (j = 0; j < n; j++) {
            if (i == j) {
                total = total + i;
            }
        }
    }
    return total;
}
