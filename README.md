# OctoNode

A dark-themed desktop Git client built for using several accounts side by side:
personal GitHub, work GitLab, self-hosted GitLab, and plain local repositories.
Each operation runs with the right SSH key, token, and author identity, chosen
per repository at the Rust level.

**Stack:** Tauri 2 (stable) · React 18 · TypeScript · Tailwind CSS 3 · `git2`
(vendored libgit2) · `keyring` with an encrypted-file fallback · `directories`.

See **[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)** for the design, diagrams,
and cross-platform strategy.

## Features

- **Multi-account workspaces.** Each workspace is bound to one account, so its
  repositories use that account's SSH key (`IdentitiesOnly`), its
  host-scoped token, and its `user.name`/`user.email`. The isolation applies to
  libgit2 calls and to the `git` CLI fallback.
- **Secret storage** in Keychain, Credential Manager, or Secret Service. When
  none of those is reachable (headless Linux, a window manager without a keyring
  daemon), tokens go into an Argon2id + XChaCha20-Poly1305 vault unlocked with a
  master password.
- **Commit graph.** Lanes are laid out in Rust and cached until a ref moves. The
  list is virtualized and the graph is drawn on a Canvas. On rust-lang/cargo
  (24k commits), layout takes about 0.4 s in a release build, and scrolling
  stays smooth.
- **Diff viewer** with unified and split modes, context folding, and a full-file
  view. You can stage or unstage individual hunks and lines; checks detect a
  stale view, so an outdated selection is never written to the index.

## Prerequisites

| OS | Requirements |
|---|---|
| All | Rust stable (≥ 1.77), Node.js 22 LTS, `git` on `PATH` (used for the CLI fallback) |
| Linux | `libwebkit2gtk-4.1-dev libsoup-3.0-dev libjavascriptcoregtk-4.1-dev libgtk-3-dev librsvg2-dev libssl-dev libdbus-1-dev pkg-config` |
| Windows | MSVC build tools; WebView2 (preinstalled on Windows 10 and 11) |
| macOS | Xcode Command Line Tools |

## Develop

```bash
npm install
npm run tauri dev        # Vite dev server + Rust backend with hot reload
```

## Test and lint

```bash
npm run typecheck && npm test                  # TS strict + vitest
cd src-tauri && cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
```

Graph layout benchmark on any repository:

```bash
cd src-tauri
OCTONODE_BENCH_REPO=/path/to/repo cargo test --release bench_graph -- --ignored --nocapture
```

## Build installers

```bash
npm run tauri build      # .deb/.rpm/.AppImage · .msi/.exe · .app/.dmg
```

## Configuration

| Variable | Purpose |
|---|---|
| `OCTONODE_LOG` | Log filter, e.g. `debug` or `octonode_lib=trace` |
| `OCTONODE_GIT` | Path to the `git` executable, e.g. a portable Git for Windows |

Config is stored in the platform config directory (`config.json`). The fallback
vault is stored in the platform local-data directory (`secrets.vault`).
