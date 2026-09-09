#!/usr/bin/env bash
set -euo pipefail

repo_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
binary="$repo_dir/target/release/gekitai-solver"

reserve_gib=${GEKITAI_RESERVE_GIB:-32}
ceiling_gib=${GEKITAI_MEMORY_CEILING_GIB:-192}

if [[ ! $reserve_gib =~ ^[0-9]+$ || ! $ceiling_gib =~ ^[0-9]+$ ]]; then
    echo "GEKITAI_RESERVE_GIB and GEKITAI_MEMORY_CEILING_GIB must be integers" >&2
    exit 2
fi

available_kib=$(awk '/^MemAvailable:/ { print $2; exit }' /proc/meminfo)
available_gib=$((available_kib / 1024 / 1024))
if (( available_gib <= reserve_gib + 1 )); then
    echo "Not enough available RAM: ${available_gib} GiB available, ${reserve_gib} GiB reserved" >&2
    exit 1
fi

safe_gib=$((available_gib - reserve_gib))
if (( ceiling_gib < safe_gib )); then
    cap_gib=$ceiling_gib
else
    cap_gib=$safe_gib
fi
high_gib=$((cap_gib * 9 / 10))
if (( high_gib < 1 )); then
    high_gib=1
fi

if [[ ! -x $binary ]]; then
    rustup run stable cargo build --release --locked --manifest-path "$repo_dir/Cargo.toml"
fi

echo "Launching proof in a cgroup: MemoryHigh=${high_gib}G MemoryMax=${cap_gib}G reserve=${reserve_gib}G" >&2
exec systemd-run --user --scope --collect --quiet \
    -p "MemoryHigh=${high_gib}G" \
    -p "MemoryMax=${cap_gib}G" \
    -p MemorySwapMax=0 \
    "$binary" \
    --proof \
    --proof-max-ram-gb "$cap_gib" \
    --proof-system-reserve-gb "$reserve_gib" \
    "$@"
