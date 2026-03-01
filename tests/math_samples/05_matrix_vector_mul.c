// Test: Matrix-vector multiplication (3x3 matrix)
// Matrix stored as flat 1D array: A[i][j] = arr[i*3 + j]
void mat_vec_mul() {
    int A[9];
    A[0] = 1; A[1] = 2; A[2] = 3;
    A[3] = 4; A[4] = 5; A[5] = 6;
    A[6] = 7; A[7] = 8; A[8] = 9;

    int x[3];
    x[0] = 1;
    x[1] = 0;
    x[2] = 2;

    int result[3];
    result[0] = 0;
    result[1] = 0;
    result[2] = 0;

    int i = 0;
    while (i < 3) {
        int j = 0;
        while (j < 3) {
            result[i] = result[i] + A[i * 3 + j] * x[j];
            j = j + 1;
        }
        i = i + 1;
    }
}
