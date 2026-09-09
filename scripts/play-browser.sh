#!/usr/bin/env bash
set -euo pipefail
repo_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
certificate=${1:-"$repo_dir/forcing-data/clean-first.certificate"}
port=${2:-8765}
exec "$repo_dir/target/release/gekitai-solver" --serve-certificate "$certificate" --port "$port"
