#!/bin/bash
# Build solar2d-mcp.exe for Windows from Linux (cargo-zigbuild + Zig as the linker).
set -e
. "/media/jeremy/1TB SSD/solar2d-mcp-tools/env.sh"
cd "$(dirname "$0")/.."
cargo zigbuild --release --target x86_64-pc-windows-gnu
ls -la target/x86_64-pc-windows-gnu/release/solar2d-mcp.exe
