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
- **Clone remote repositories** into a workspace, either by picking from the
  account's GitHub/GitLab repositories (needs a token) or by pasting any
  `https://`, `ssh://` or `git@host:path` URL. The clone uses the workspace
  account's SSH key or token, shows progress, can be cancelled (the partial
  folder is removed), and can write the account identity into the clone's
  `.git/config`. The sidebar's ⤓ button opens it.
- **Open a project's folder** in the system file manager (Files/Explorer/Finder)
  from the folder icon on its sidebar row, or the **Open folder** button in the
  header.
- **Editable accounts.** Open the editor with the pencil in the sidebar header,
  by right-clicking an account avatar, from Settings → Accounts, or from the
  **Edit account** button on an authentication error. You can change the label,
  host, API URL, username, author identity and SSH key, replace or remove the
  access token and SSH passphrase, test the token, or delete the account.
- **Actionable, copyable errors.** Authentication failures name the account and
  the fix: "… has no access token, add one or clone over SSH". Every error
  message can be selected and has a **Copy** button, and error toasts stay until
  dismissed.
- **Settings** (`Ctrl+,` / `⌘,`): change the vault's master password (it
  re-encrypts in place, so there's no restart and no lost tokens), lock the
  vault, and set the default clone folder (`~/Projects`).
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

## Releases

Push a version tag (it must match `version` in `package.json` and
`src-tauri/tauri.conf.json`):

```bash
git tag v0.1.0 && git push origin v0.1.0
```

`.github/workflows/release.yml` then builds a **draft** GitHub Release with:
- Windows: `OctoNode_<ver>_x64-setup.exe` (NSIS, per-user install, no admin
  rights needed), an `.msi`, and `OctoNode_<ver>_x64-portable.exe`
- Linux: `.AppImage`, `.deb`, `.rpm`
- macOS: a `.dmg` each for Apple Silicon and Intel

You can also start the workflow manually from the Actions tab. Every CI run
uploads the same installers as run artifacts.

## Linux troubleshooting

| Symptom | Cause / fix |
|---|---|
| `Gdk-Message: Error 71 (Protocol error) dispatching to Wayland display` | WebKitGTK's DMA-BUF renderer fails on some Wayland compositor and GPU combinations (NVIDIA especially). OctoNode sets `WEBKIT_DISABLE_DMABUF_RENDERER=1`, and on NVIDIA `__NV_DISABLE_EXPLICIT_SYNC=1`, unless you set them yourself. If the window still dies, run with `OCTONODE_FORCE_X11=1` to use XWayland. |
| `Secret Service: no result found` | Your Secret Service has no `default` collection (common with KeePassXC, some KWallet setups, WSL). OctoNode then uses an existing persistent collection, preferring `login`. To choose one yourself, set `OCTONODE_KEYRING_COLLECTION=<exact label>` (labels are case-sensitive). If no collection exists, the encrypted-file vault is used instead. |
| `DBus error … dbus-launch` | There's no D-Bus session (headless or minimal window manager). The encrypted-file vault is used instead. |

## Configuration

| Variable | Purpose |
|---|---|
| `OCTONODE_LOG` | Log filter, e.g. `debug` or `octonode_lib=trace` |
| `OCTONODE_GIT` | Path to the `git` executable, e.g. a portable Git for Windows |
| `OCTONODE_KEYRING_COLLECTION` | Linux: Secret Service collection label to store tokens in |
| `OCTONODE_FORCE_X11` | Linux: `1` runs through XWayland (`GDK_BACKEND=x11`) |

Config is stored in the platform config directory (`config.json`). The fallback
vault is stored in the platform local-data directory (`secrets.vault`).
