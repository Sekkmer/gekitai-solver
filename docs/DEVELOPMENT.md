# Development

Use stable Rust, Node.js 22 or newer, and Python 3. The frontend has no runtime
package dependencies or build step. Development dependencies are pinned in
`package-lock.json`.

```sh
npm ci
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo test --locked --test browser_reference -- --ignored
npm run format:check
npm test
```

The ignored integration test generates `test/evidence/static-rules.json` from
10,000 Rust move cases. It is generated locally and in CI, not checked into Git.
The JavaScript tests compare board transitions, terminal results, push-animation
traces, and canonical keys against those cases. They also exhaust all 7,140
three-piece placements, independently decode the complete policy, replay the
demo, play 500 complete strategy games, and check tactical proof rejection.

## Browser tests

```sh
npx playwright install chromium
npm run serve
# In another terminal:
npm run test:browser
npm run test:motion
```

Tests use an installed `/usr/bin/chromium` when available, otherwise Playwright's
Chromium. Set `PLAYWRIGHT_CHROMIUM_EXECUTABLE` to override it. Screenshots and
other generated evidence go into ignored `test/evidence/`.

A test URL may be passed explicitly, including a GitHub Pages project subpath:

```sh
node test/browser/static.cjs https://sekkmer.github.io/gekitai-solver/
node test/browser/motion.cjs https://sekkmer.github.io/gekitai-solver/
```

Browser checks cover separate tabs, verified replies, practice and cancellation,
cached strategy reuse, failed-download isolation, animation timing and actual
movement, reduced motion, mobile layout, and the complete 25-placement demo.
They also assert that the static app makes no gameplay API requests.

`smoke.cjs`, `demo.cjs`, and `public.cjs` are additional reference-server tests;
they require a running Rust server and the relevant fixtures. They are not needed
to develop or host the static app.

## Code map

| Area                                                                   | Responsibility                                                                |
| ---------------------------------------------------------------------- | ----------------------------------------------------------------------------- |
| `src/lib.rs`                                                           | Rust library entry point                                                      |
| `src/main.rs`, `src/cli.rs`                                            | CLI orchestration and argument definitions                                    |
| `src/board.rs`, `movegen.rs`, `position.rs`, `rules.rs`, `symmetry.rs` | Rules, board representation, exact symmetry keys                              |
| `src/forcing.rs`, `certificate.rs`, `certificate_work.rs`              | Reachability search, independent checking, resumable certificate construction |
| `src/proof.rs`                                                         | Disk-backed retrograde graph solver                                           |
| `src/search.rs`, `tt.rs`, `checkpoint.rs`                              | Depth-limited scouting and checkpoints                                        |
| `src/player.rs`, `trainer.rs`, `player_server.rs`                      | Original Rust reference player                                                |
| `web/engine.js`, `policy.js`                                           | Browser rules and exact strategy lookup                                       |
| `web/practice.js`                                                      | Bounded search and independent tactical-tree checking                         |
| `web/app.js`                                                           | Turn sequencing, cancellation, and game state                                 |
| `web/view.js`, `theme.js`, `styles.css`                                | DOM rendering and appearance                                                  |
| `web/motion.js`                                                        | Cancellable landing/push animation sequence                                   |
| `web/strategy-client.js`, `*-worker.js`                                | Background worker boundaries                                                  |

## Changes that need particular care

Serialized enum order, rule field order, ternary board encoding, symmetry maps,
and certificate state codes are part of the persisted format. A source rename
must not alter them. A rules change invalidates proof claims until the strategy
has been checked under those rules. Never treat a search timeout or missing
cache entry as a proof of impossibility.

Keep the renderer separate from board mutation: moves are validated before
animation, then committed after it settles. Reset and mode changes invalidate
both queued worker answers and pending animations. The practice search must
never label heuristic scores or unverified partial trees as forced wins.

Run `npm run format` and `cargo fmt --all` after edits. CI applies the same gates
before Pages deployment.
