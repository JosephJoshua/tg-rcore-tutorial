#!/bin/bash
set -eo pipefail

cargo build 2>&1

OUTPUT=$(timeout 30 qemu-system-riscv64 \
    -machine virt \
    -smp 4 \
    -nographic \
    -bios none \
    -kernel "target/riscv64gc-unknown-none-elf/debug/jsph-tg-rcore-tutorial-ch2-smp" 2>&1) || true

echo "$OUTPUT"

PASS=true
for i in 0 1 2 3; do
    if ! echo "$OUTPUT" | grep -q "Hart $i"; then
        echo "FAIL: Hart $i not found"
        PASS=false
    fi
done

if echo "$OUTPUT" | grep -q "Hello, world!"; then
    echo "PASS: User programs executed"
else
    echo "FAIL: User program output missing"
    PASS=false
fi

if [ "$PASS" = true ]; then
    echo "Test PASSED"
    exit 0
else
    echo "Test FAILED"
    exit 1
fi
