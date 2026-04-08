#!/bin/bash
set -e

cargo build 2>&1
BINARY="target/riscv64gc-unknown-none-elf/debug/jsph-tg-rcore-tutorial-ch1-smp"

OUTPUT=$(timeout 60 qemu-system-riscv64 \
    -machine virt \
    -smp 4 \
    -display none \
    -serial stdio \
    -bios none \
    -device virtio-gpu-device \
    -kernel "$BINARY" 2>&1) || true

echo "$OUTPUT"

PASS=true
for i in 0 1 2 3; do
    if ! echo "$OUTPUT" | grep -q "Hart $i"; then
        echo "FAIL: Hart $i output not found"
        PASS=false
    fi
done

if echo "$OUTPUT" | grep -q "Demo 2"; then
    echo "PASS: Demo 2 section found"
else
    echo "FAIL: Demo 2 section not found"
    PASS=false
fi

if [ "$PASS" = true ]; then
    echo "Test PASSED: All 4 harts produced output"
    exit 0
else
    echo "Test FAILED"
    exit 1
fi
