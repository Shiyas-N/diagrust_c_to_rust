int linear_search(int target) {
    int arr[6];
    arr[0] = 3;
    arr[1] = 7;
    arr[2] = 1;
    arr[3] = 9;
    arr[4] = 4;
    arr[5] = 6;

    int i = 0;
    int found = -1;
    while (i < 6) {
        if (arr[i] == target) {
            found = i;
            break;
        }
        i = i + 1;
    }
    return found;
}
