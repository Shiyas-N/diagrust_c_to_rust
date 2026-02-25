int calculate(int x, int y, int z) {
    int sum = x + y;
    int product = sum * z;
    int final_result = product - x;
    return final_result;
}