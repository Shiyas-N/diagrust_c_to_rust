// Test: Fibonacci via dynamic programming (array memoization)
int fib_dp(int n) {
    int memo[30];
    memo[0] = 0;
    memo[1] = 1;

    int i = 2;
    while (i <= n) {
        memo[i] = memo[i - 1] + memo[i - 2];
        i = i + 1;
    }
    return memo[n];
}
