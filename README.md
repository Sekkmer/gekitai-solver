# Gekitai Solver

[Play in your browser](https://sekkmer.github.io/gekitai-solver/) · [Verified results](docs/RESULTS.md) · [Rules](docs/RULES.md)

A browser player and Rust solver for **Gekitai**, the push-away board game designed
by [Scott Brady](https://boardgamegeek.com/boardgamedesigner/117420/scott-brady).
Place a piece, push its neighbours, and win with three adjacent pieces in a line
or all eight pieces on the board.

- **Verified strategy:** play second against a strategy that forces a clean win
  within 25 total placements under the [documented rules](docs/RULES.md).
- **Practice:** play first against a three-second search on your device.
  Small forced-win continuations are checked before being labeled verified;
  ordinary moves can miss mistakes.
- **Demo:** watch a checked 25-placement game, with animated pushes, pause,
  and single-step controls.

The website runs entirely in JavaScript. The verified strategy downloads about
12 MB once and is cached when browser storage is available. Practice and the
demo can run without that download. No account, gameplay API, or server-side
search is required.

## Run locally

```sh
python3 -m http.server 8769 --bind 127.0.0.1 --directory web
```

Open [localhost:8769](http://127.0.0.1:8769). The site needs HTTP on localhost or
HTTPS on a public host; opening the HTML directly from the filesystem will not
load its workers. There is no frontend build step.

## Rust solver

```sh
cargo build --release --locked
cargo run --release -- --help
```

The Rust library implements the rules, symmetry reduction, bounded forcing
search, loopy retrograde search, checkpoints, and independent certificate
checking. The CLI also retains a reference browser server.

To check a downloaded full certificate without rerunning the search:

```sh
cargo run --release -- --verify-forcing-certificate clean-first.certificate
```

Full certificates are distributed as [release assets](https://github.com/Sekkmer/gekitai-solver/releases/tag/v0.1.0).
The 25-placement bound is a verified upper bound; this project does not claim
an independently certified shortest possible win. See [results and provenance](docs/RESULTS.md).

## Develop and publish

```sh
npm ci
cargo test --locked
cargo test --locked --test browser_reference -- --ignored
npm test
```

See [development](docs/DEVELOPMENT.md) for browser tests, formatting, architecture,
and proof-format compatibility. [Solver usage](docs/SOLVER.md) covers resource
budgets and checkpoints. [Deployment](docs/DEPLOYMENT.md) explains GitHub Pages
and packaging for any static HTTPS host.

CI checks Rust, formatting, the compressed strategy, cross-language rule cases,
and real browser play before deploying `web/` to GitHub Pages on `main`.

## Attribution

This is an independent software implementation and analysis project, not an
official Gekitai product. Gekitai's game design is by Scott Brady;
see the [game listing](https://boardgamegeek.com/boardgame/295449/gekitai).
The source code is available under the [MIT license](LICENSE).
