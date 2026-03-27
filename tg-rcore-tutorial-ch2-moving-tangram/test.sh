#!/bin/bash
# ch2-moving-tangram test script
# Runs kernel with VirtIO-GPU in headless mode (no display) with timeout.

set -e

GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[0;33m'
NC='\033[0m'

echo "Running ch2-moving-tangram test..."
echo -e "${YELLOW}────────── cargo run output ──────────${NC}"

# Run with timeout (60s) — GPU init + 14 pieces × 300ms + overhead
set -o pipefail
if timeout 60 cargo run 2>&1 | tee /dev/stderr | grep -q "\[tangram\] rendering piece 13"; then
    echo ""
    echo -e "${YELLOW}────────── test result ──────────${NC}"
    echo -e "${GREEN}✓ ch2-moving-tangram: all 14 pieces rendered${NC}"
    exit 0
else
    echo ""
    echo -e "${YELLOW}────────── test result ──────────${NC}"
    echo -e "${RED}✗ ch2-moving-tangram: failed to render all pieces${NC}"
    exit 1
fi
