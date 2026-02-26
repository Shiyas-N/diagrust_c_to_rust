#!/bin/bash
# Control Flow Testing Script for DIAGRUST

echo "🔄 DIAGRUST Control Flow Testing"
echo "================================="
echo ""

# Create test directory
mkdir -p tests/control_flow

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Counters
total=0
passed=0
compiled=0
failed=0

echo "📝 Creating test files..."

# Test 1: If-Else
cat > tests/control_flow/01_if_else.c << 'EOF'
int max(int a, int b) {
    if (a > b) {
        return a;
    } else {
        return b;
    }
}
EOF

# Test 2: While loop
cat > tests/control_flow/02_while_simple.c << 'EOF'
int sum_to_n(int n) {
    int sum = 0;
    int i = 0;
    while (i < n) {
        sum = sum + i;
        i = i + 1;
    }
    return sum;
}
EOF

# Test 3: For loop
cat > tests/control_flow/03_for_simple.c << 'EOF'
int sum_range(int n) {
    int sum = 0;
    for (int i = 0; i < n; i = i + 1) {
        sum = sum + i;
    }
    return sum;
}
EOF

# Test 4: Nested if
cat > tests/control_flow/04_nested_if.c << 'EOF'
int classify(int x) {
    if (x > 0) {
        if (x > 10) {
            return 2;
        } else {
            return 1;
        }
    } else {
        return 0;
    }
}
EOF

# Test 5: If in loop
cat > tests/control_flow/05_if_in_loop.c << 'EOF'
int count_positives(int n) {
    int count = 0;
    for (int i = 0; i < n; i = i + 1) {
        if (i > 0) {
            count = count + 1;
        }
    }
    return count;
}
EOF

# Test 6: Factorial
cat > tests/control_flow/06_factorial.c << 'EOF'
int factorial(int n) {
    int result = 1;
    for (int i = 1; i <= n; i = i + 1) {
        result = result * i;
    }
    return result;
}
EOF

# Test 7: GCD
cat > tests/control_flow/07_gcd.c << 'EOF'
int gcd(int a, int b) {
    while (b != 0) {
        int temp = b;
        b = a % b;
        a = temp;
    }
    return a;
}
EOF

echo "✅ Test files created"
echo ""

echo "🧪 Running translation tests..."
echo "================================"
echo ""

for file in tests/control_flow/*.c; do
    if [ ! -f "$file" ]; then
        continue
    fi
    
    total=$((total + 1))
    filename=$(basename "$file")
    echo -n "Testing $filename ... "
    
    # Run diagrust
    output=$(./target/release/diagrust "$file" --no-compile 2>&1)
    
    # Check if translation succeeded
    rs_file="${file%.c}.rs"
    if [ -f "$rs_file" ]; then
        echo -ne "${GREEN}✓ Translated${NC}"
        passed=$((passed + 1))
        
        # Try to compile the Rust file
        compile_output=$(rustc --crate-type lib "$rs_file" 2>&1)
        if [ $? -eq 0 ]; then
            echo -e " ${GREEN}✓ Compiles${NC}"
            compiled=$((compiled + 1))
        else
            echo -e " ${RED}✗ Compile Failed${NC}"
            if [ ! -z "$1" ] && [ "$1" == "-v" ]; then
                echo "$compile_output" | head -5 | sed 's/^/    /'
            fi
        fi
    else
        echo -e "${RED}✗ Translation Failed${NC}"
        failed=$((failed + 1))
        if [ ! -z "$1" ] && [ "$1" == "-v" ]; then
            echo "$output" | grep "Error" | head -3 | sed 's/^/    /'
        fi
    fi
done

echo ""
echo "📊 Test Results Summary"
echo "======================="
echo ""
echo "Total Tests:      $total"
echo -e "${GREEN}Translated:       $passed ($((passed * 100 / total))%)${NC}"
echo -e "${GREEN}Compiled:         $compiled ($((compiled * 100 / total))%)${NC}"
echo -e "${RED}Failed:           $failed${NC}"
echo ""

if [ $compiled -eq $total ]; then
    echo -e "${GREEN}🎉 Perfect! All tests passed!${NC}"
elif [ $passed -eq $total ]; then
    echo -e "${YELLOW}⚠️  All translated, but some don't compile yet${NC}"
elif [ $passed -gt $((total / 2)) ]; then
    echo -e "${YELLOW}✓ More than half working - good progress!${NC}"
else
    echo -e "${RED}⚠️  More work needed on control flow parsing${NC}"
fi

echo ""
echo "💡 Tip: Run with -v flag for verbose output"
echo "   Example: $0 -v"
echo ""

# Show sample generated code
if [ $compiled -gt 0 ]; then
    echo "📄 Sample Generated Code:"
    echo "========================="
    first_rs=$(ls tests/control_flow/*.rs 2>/dev/null | head -1)
    if [ ! -z "$first_rs" ]; then
        echo "File: $(basename $first_rs)"
        echo "---"
        cat "$first_rs"
    fi
fi