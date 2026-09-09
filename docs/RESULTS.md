# Verified results

Four certificates were completed and independently checked on 2026-09-07.
They apply from the empty board under the [documented rule model](RULES.md).

| Objective                       | First player                         | Second player |
| ------------------------------- | ------------------------------------ | ------------- |
| Clean win                       | Can force within 25 total placements | Cannot force  |
| Clean win or simultaneous lines | Can force within 25 total placements | Cannot force  |

The negative results have closed defensive strategies, not merely failed
finite-depth searches. The positive results have ranked strategies covering
every legal opposing reply. The clean-first certificate contains 10,162,722
explicit strategy states, plus immediate leaves reconstructed by the checker.
The bound counts both players' moves. It is not an independently certified
minimum winning distance or a strong solution of every reachable position.

## Full proof artifacts

Download the `.certificate` files from the [v0.1.0 release](https://github.com/Sekkmer/gekitai-solver/releases/tag/v0.1.0).
They are release assets rather than large blobs in Git history. Verify one with:

```sh
cargo run --release -- --verify-forcing-certificate clean-first.certificate
```

The checker regenerates moves and verifies terminal conditions and all required
replies. It does not use the search cache. Checking needs far less memory than
finding the strategy.

| File                                 | SHA-256                                                            |
| ------------------------------------ | ------------------------------------------------------------------ |
| `clean-first.certificate`            | `c01f23955ed3c7bc797e0c3665688fd6ab5a8ae0807ef40747e79c45b70d1010` |
| `clean-or-double-first.certificate`  | `f3c82948eaab454803f707e888d36f71426313991added6fcf93d5705e2121d9` |
| `clean-or-double-second.certificate` | `b73cd1d61cca8f9276a6aecbc38d44652033ce057df25459267bf721d85ea3a5` |
| `clean-second.certificate`           | `5c2b3271e8d13f0ce3615eb4666d8a09a9cba2b6457e86489be58669cfe5e29f` |

The final resumed search peaked at 258.4 GiB of memory; an earlier attempt
peaked at 265.3 GiB. Those are observations, not minimum resource requirements.
The large search table is unnecessary for playing the resulting strategy.

## Browser policy

The browser needs only the 5,312,888 computer-turn decisions from clean-first,
plus local rule checks for immediate winning leaves. Sorted state-code deltas
and move bytes occupy 15,075,697 bytes; gzip reduces this to 11,963,857 bytes.

`web/assets/strategy.json` records the source certificate SHA, asset SHA, format,
entry count, and size. The browser checks the asset digest and decodes it into
exact typed-array keys. The exporter checks every decision survives its roundtrip.
Tests separately check every decoded entry and 500 complete games against varied
replies.

The compact policy omits proof-only opponent coverage records and ranks. It
plays an already verified strategy; it is not a standalone proof certificate.
The full artifacts above remain available for independent checking.
