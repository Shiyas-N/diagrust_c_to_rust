// Test: do-while loop
#include <stdio.h>

int main() {
  int i = 1;
  int sum = 0;

  do {
    sum = sum + i;
    i = i + 1;
  } while (i <= 10);

  printf("Sum 1..10 = %d\n", sum);

  // do-while with break
  int count = 0;
  do {
    count = count + 1;
    if (count > 5)
      break;
  } while (1);

  printf("Count stopped at: %d\n", count);
  return 0;
}
