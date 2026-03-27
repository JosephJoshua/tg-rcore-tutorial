#!/bin/bash

# Build the kernel binary
cargo build 2>&1
BINARY="target/riscv64gc-unknown-none-elf/debug/tg-rcore-tutorial-ch1"

# Run QEMU with GPU device in headless mode, with a timeout to catch hangs.
# The kernel waits up to 10s after rendering; give it 20s total.
OUTPUT=$(timeout 20 qemu-system-riscv64 \
    -machine virt \
    -display none \
    -serial stdio \
    -bios none \
    -device virtio-gpu-device \
    -kernel "$BINARY" 2>&1)

if echo "$OUTPUT" | grep -q "Hello, world!"; then
    echo "Test PASSED: Found 'Hello, world!' in output"
    exit 0
else
    echo "Test FAILED: 'Hello, world!' not found in output"
    echo "Actual output:"
    echo "$OUTPUT"
    exit 1
fi
