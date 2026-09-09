# Rust solver usage

```sh
cargo build --release --locked
./target/release/gekitai-solver --help
```

## Three kinds of search

- **Scouting** is iterative-deepening alpha-beta. It is useful for exploration;
  its history-independent transposition cache is not a proof of the loopy game.
- **Forcing search** asks whether a designated player can force a particular
  objective. Definitive answers require independently checked certificates.
- **Retrograde proof** builds the reachable graph and solves it by fixed-point
  propagation. The graph may be extremely large.

## Scouting

```sh
./target/release/gekitai-solver --max-depth 12 --tt-mb 1024 --checkpoint checkpoint.zst
./target/release/gekitai-solver --resume --max-depth 14 --checkpoint checkpoint.zst
```

Depth is measured in plies (individual placements). `--tt-mb` is an approximate
budget for the scouting map; its memory can grow. Checkpoints are streamed and
optionally compressed. Ctrl+C requests a checkpoint and clean exit.

## Forcing objectives

```sh
./target/release/gekitai-solver --forcing both --forcing-side both \
  --max-depth 25 --tt-mb 4096 --forcing-dir forcing-data
```

This starts four adversarial reachability questions, which can be expensive even
with the depth capped. The published solution used a much larger server.

| Objective         | Success for the designated player                                                 |
| ----------------- | --------------------------------------------------------------------------------- |
| `clean`           | Their line of three or all eight pieces, without an opponent win condition        |
| `clean-or-double` | A clean win or both players simultaneously having lines of three                  |
| `double-only`     | Simultaneous actual lines; an ordinary win ends play without reaching this target |

`both` selects clean and clean-or-double; `all` also includes double-only.
`--forcing-side` accepts `first`, `second`, or `both`.

`FORCED within N plies` requires a ranked strategy checked against every opposing
reply. `CANNOT FORCE` requires a closed defensive strategy, including along
cycles. `UNRESOLVED; cannot force within N plies` is only a finite-horizon lower
bound and does not establish impossibility or a draw.

The forcing `--tt-mb` budget is a fixed allocation shared across the selected
objectives and roles. Full keys are collision-checked; eviction only causes
recomputation. Search caches are not certificates. Checkpoint files are bound
to rules, objective, and player. Resume with the same selection:

```sh
./target/release/gekitai-solver --forcing both --forcing-side both \
  --max-depth 25 --tt-mb 4096 --forcing-dir forcing-data --forcing-resume
```

Successful certificates are saved as `NAME.certificate`. Unfinished construction
is saved as `NAME.certificate-work` and is not a proof. The checker supports up
to 100,000,000 explicit states; a certificate at that cap is about 1.1 GB on disk
and needs several GiB to check. Construction pauses at the cap and can resume.

## Resource-capped server run

On Linux with user systemd, `scripts/run-forcing-server.sh` provides a cgroup
boundary and persistent execution. Its defaults target a large-memory machine:
256 GiB table, 384 GiB process ceiling, 128 GiB system reserve, 36 CPU threads,
and NUMA interleaving. It checks available RAM and disk before starting.

Override `GEKITAI_TABLE_MIB`, `GEKITAI_MEMORY_CEILING_GIB`,
`GEKITAI_RESERVE_GIB`, `GEKITAI_THREADS`, `GEKITAI_RUN_DIR`, or `GEKITAI_UNIT`.
`GEKITAI_NUMA_POLICY=default` disables the optional `numactl` placement.

```sh
./scripts/run-forcing-server.sh
journalctl --user -u gekitai-forcing -f
systemctl --user stop gekitai-forcing
# Rerun the script to resume available checkpoints.
```

## Retrograde graph proof

```sh
./scripts/run-proof-capped.sh --proof-disk-dir proof-data
```

This constructs disk-backed edges and a state index, then solves the graph.
Defaults preserve a 32 GiB system reserve within a 192 GiB ceiling, clamped to
live memory availability. Override the wrapper's `GEKITAI_MEMORY_CEILING_GIB`
and `GEKITAI_RESERVE_GIB`, or the CLI's `--proof-max-ram-gb`,
`--proof-ram-ceiling-gb`, and `--proof-system-reserve-gb`.

`--proof-max-states` adds a state-count guard. `--proof-fast-ram` uses an in-memory
index reconstructed from durable keys. `--proof-resume` resumes the printed
run directory; rules and index backend must match. Durable checkpoints flush
edges and record exact file lengths before publication. Resuming truncates
uncommitted tails. Checkpoints without rule metadata cannot be resumed.

## Original Rust reference player

The public website uses JavaScript, but a Rust server remains useful for
cross-checks and certificate experiments:

```sh
./target/release/gekitai-solver --serve-certificate clean-first.certificate --port 8765
```

It binds to loopback and checks the full certificate before verified play.
Practice is available while it loads. Its human game, checked demo, and
bounded practice search are covered by Rust tests. `scripts/play-browser.sh`
is a convenience wrapper; it takes the certificate path as its first argument.
