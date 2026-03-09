#include <stdio.h>

void bubble_sort(int arr[], int n) {
    int i = 0;
    while (i < n - 1) {
        int j = 0;
        while (j < n - i - 1) {
            if (arr[j] > arr[j + 1]) {
                int temp = arr[j];
                arr[j] = arr[j + 1];
                arr[j + 1] = temp;
            }
            j = j + 1;
        }
        i = i + 1;
    }
}

int binary_search(int arr[], int n, int target) {
    int low = 0;
    int high = n - 1;
    while (low <= high) {
        int mid = (low + high) / 2;
        if (arr[mid] == target) {
            return mid;
        }
        if (arr[mid] < target) {
            low = mid + 1;
        } else {
            high = mid - 1;
        }
    }
    return -1;
}

void print_array(int arr[], int n) {
    int i = 0;
    while (i < n) {
        printf("%d ", arr[i]);
        i = i + 1;
    }
    printf("\n");
}

int main() {
    int arr[8];
    arr[0] = 64;
    arr[1] = 34;
    arr[2] = 25;
    arr[3] = 12;
    arr[4] = 22;
    arr[5] = 11;
    arr[6] = 90;
    arr[7] = 45;

    int n = 8;

    printf("Before sorting: ");
    print_array(arr, n);

    bubble_sort(arr, n);

    printf("After sorting:  ");
    print_array(arr, n);

    int target = 25;
    int idx = binary_search(arr, n, target);
    printf("Search %d: index = %d\n", target, idx);

    target = 99;
    idx = binary_search(arr, n, target);
    printf("Search %d: index = %d (not found)\n", target, idx);

    return 0;
}
