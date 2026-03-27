Extend `tg-rcore-tutorial-ch1` to display a tangram "OS" pattern on screen using a VirtIO-GPU framebuffer.

Demo reference: https://github.com/rcore-os/tg-rcore-tutorial-game-demo/blob/main/ch1-tangram.png

## Goal

Add VirtIO-GPU framebuffer rendering to ch1. On `cargo run`, the QEMU graphical window should show an "OS" pattern composed of tangram pieces (7 pieces, each a distinct color). Serial "Hello, world!" output should still work. Shutdown after displaying.

## Key Constraints

- Only modify files under `tg-rcore-tutorial-ch1/`
- ch1 has no page tables (identity mapping), no heap allocator, and a 4KiB stack — all memory must be statically allocated and the stack will likely need enlarging
- Do not modify `tg-rcore-tutorial-sbi` or other shared crates
- QEMU runner config (`.cargo/config.toml`) needs `-device virtio-gpu-device` and must replace `-nographic` with graphical output (keep `-serial stdio` for console)
- Reference ch6's `virtio_block.rs` for the `virtio-drivers` crate usage pattern (Hal trait, MMIO transport), but adapt for the no-alloc ch1 environment
- `cargo check` and `cargo publish --dry-run` must still pass

## Acceptance Criteria

- QEMU window displays a tangram "OS" pattern matching the demo screenshot (distinct colors, recognizable letters)
- Serial still prints "Hello, world!"
- Clean shutdown after display
