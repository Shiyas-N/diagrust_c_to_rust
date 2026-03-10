// Test: Complex program with structs, arrays, loops, and function calls
#include <stdio.h>

struct Point {
  int x;
  int y;
};

int distance_sq(int x1, int y1, int x2, int y2) {
  int dx = x2 - x1;
  int dy = y2 - y1;
  return dx * dx + dy * dy;
}

void swap(int arr[], int i, int j) {
  int temp = arr[i];
  arr[i] = arr[j];
  arr[j] = temp;
}

void bubble_sort(int arr[], int n) {
  int i, j;
  for (i = 0; i < n - 1; i++) {
    for (j = 0; j < n - i - 1; j++) {
      if (arr[j] > arr[j + 1]) {
        swap(arr, j, j + 1);
      }
    }
  }
}

int main() {
  struct Point p1;
  p1.x = 3;
  p1.y = 4;

  struct Point p2;
  p2.x = 6;
  p2.y = 8;

  int dist = distance_sq(p1.x, p1.y, p2.x, p2.y);
  printf("Distance squared: %d\n", dist);

  int arr[5] = {5, 3, 1, 4, 2};
  bubble_sort(arr, 5);

  printf("Sorted: ");
  int i;
  for (i = 0; i < 5; i++) {
    printf("%d ", arr[i]);
  }
  printf("\n");

  return 0;
}
