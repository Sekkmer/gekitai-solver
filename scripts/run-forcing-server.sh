#!/usr/bin/env bash
# Run on the server. User service survives SSH disconnects; SIGINT checkpoints.
set -euo pipefail
repo_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
binary="$repo_dir/target/release/gekitai-solver"
run_dir=${GEKITAI_RUN_DIR:-"$repo_dir/forcing-data"}
table_mib=${GEKITAI_TABLE_MIB:-262144}
cap_gib=${GEKITAI_MEMORY_CEILING_GIB:-384}
reserve_gib=${GEKITAI_RESERVE_GIB:-128}
threads=${GEKITAI_THREADS:-36}
unit=${GEKITAI_UNIT:-gekitai-forcing}
numa_policy=${GEKITAI_NUMA_POLICY:-interleave}
launch=()
case "$numa_policy" in
    interleave)
        command -v numactl >/dev/null || { echo 'Install numactl or set GEKITAI_NUMA_POLICY=default' >&2; exit 1; }
        # Shared objective tables are accessed from both sockets. Stripe their
        # pages across allowed nodes rather than depending on allocator placement.
        launch=(numactl --interleave=all)
        ;;
    default) ;;
    *) echo 'GEKITAI_NUMA_POLICY must be interleave or default' >&2; exit 2 ;;
esac
for value in "$table_mib" "$cap_gib" "$reserve_gib" "$threads"; do
    [[ $value =~ ^[0-9]+$ ]] || { echo 'Resource settings must be positive integers' >&2; exit 2; }
done
(( threads > 0 && cap_gib > 0 && table_mib >= 4 && table_mib + 8192 < cap_gib * 1024 )) || {
    echo 'Allow at least 8 GiB above the table for process overhead' >&2; exit 2;
}
[[ -x $binary ]] || { echo "Build first: cargo build --release --locked" >&2; exit 1; }
available_kib=$(awk '/^MemAvailable:/ {print $2; exit}' /proc/meminfo)
(( available_kib / 1024 / 1024 >= cap_gib + reserve_gib )) || {
    echo 'Insufficient available memory for the process ceiling plus live-system reserve' >&2; exit 1;
}
mkdir -p -- "$run_dir"
available_disk_kib=$(df -Pk -- "$run_dir" | awk 'END {print $4}')
(( available_disk_kib > 32 * 1024 * 1024 )) || { echo 'Need 32 GiB free for certificate work and a filesystem reserve' >&2; exit 1; }
resume=()
if [[ -f $run_dir/clean-first.ckpt ]]; then resume=(--forcing-resume); fi
exec systemd-run --user --unit="$unit" --collect \
    -p "WorkingDirectory=$repo_dir" \
    -p "MemoryHigh=$((cap_gib * 9 / 10))G" \
    -p "MemoryMax=${cap_gib}G" \
    -p MemorySwapMax=0 \
    -p "CPUQuota=$((threads * 100))%" \
    -p Nice=10 \
    -p KillSignal=SIGINT \
    -p TimeoutStopSec=600 \
    -p UMask=0077 \
    "${launch[@]}" "$binary" --forcing both --forcing-side both --max-depth 512 \
    --threads "$threads" --tt-mb "$table_mib" \
    --forcing-dir "$run_dir" --forcing-checkpoint-mb 256 \
    --proof-checkpoint-seconds 300 "${resume[@]}"
