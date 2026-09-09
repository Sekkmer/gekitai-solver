# Static deployment

The permanent project site is [sekkmer.github.io/gekitai-solver](https://sekkmer.github.io/gekitai-solver/).

## GitHub Pages

`.github/workflows/ci-pages.yml` validates the Rust and JavaScript implementations,
then uploads **only `web/`** and deploys it to GitHub Pages on pushes to `main`.
Pull requests run the same checks without deploying. The workflow can also be
started manually.

For a fork, enable Pages in **Settings → Pages → Build and deployment → GitHub
Actions**. The static site uses relative asset and worker URLs and can run under
a repository subpath. Update the project links in the README and site footer if
publishing a fork under another account.

GitHub's [custom Pages workflow documentation](https://docs.github.com/en/pages/getting-started-with-github-pages/using-custom-workflows-with-github-pages)
describes the Pages permissions and deployment environment.

## Any static HTTPS host

Upload the contents of `web/`, or make an upload-ready archive:

```sh
npm run package
# dist/gekitai-static.zip
```

No Rust server, database, npm runtime, or frontend build step is needed. Modern
browsers need module workers, Web Crypto, and Compression Streams. The site
works over HTTPS or localhost HTTP.

Serve the strategy `.bin.gz` as an ordinary gzip file without setting
`Content-Encoding: gzip`: the worker performs decompression itself. Ordinary
transport compression of HTML, CSS, and JavaScript is fine. Only the compact
12 MB policy belongs in the served directory; full certificates are release
assets for separate verification.

The strategy is cached when browser storage permits it. A refresh starts a new
game; this is not an installable offline app. Each tab owns its own game, and
practice strength depends on the visitor's device speed.

## Re-export the strategy

```sh
python3 scripts/export-static-strategy.py clean-first.certificate web/assets
```

The exporter accepts the known independently verified source certificate by
SHA-256, writes a content-addressed gzip asset and manifest, and checks every
record survives decoding. See [RESULTS.md](RESULTS.md) for source provenance.
