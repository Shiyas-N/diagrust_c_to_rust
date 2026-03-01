void selection_sort(int n) {
    int arr[6];
    arr[0] = 29;
    arr[1] = 10;
    arr[2] = 14;
    arr[3] = 37;
    arr[4] = 13;
    arr[5] = 1;

    int i = 0;
    while (i < n - 1) {
        int min_idx = i;
        int j = i + 1;
        while (j < n) {
            if (arr[j] < arr[min_idx]) {
                min_idx = j;
            }
            j = j + 1;
        }
        int temp = arr[min_idx];
        arr[min_idx] = arr[i];
        arr[i] = temp;
        i = i + 1;
    }
}
