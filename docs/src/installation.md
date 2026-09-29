# Installation & Uninstall

WebFang is a **binaries-only** distribution. There is no `cargo install
webfang` and nothing is published to crates.io — the release automation sets
`git_only = true` and `publish = false` in `release-plz.toml`, so a
`cargo install webfang` cannot work. Get the binary from
[GitHub Releases](https://github.com/XaviCode1000/webfang/releases/latest).

> [Releases](https://github.com/XaviCode1000/webfang/releases) ·
> [Troubleshooting](troubleshooting.md) ·
> [CLI Reference](cli-reference.md)

---

## Quick path

1. Pick your platform in the [asset table](#1-pick-your-asset) and copy its
   download command.
2. [Verify the checksum](#2-verify-the-checksum) with the command for your
   shell.
3. Put the binary on your `PATH` using the per-platform steps
   ([Linux](#linux), [macOS](#macos-apple-silicon), [Windows](#windows)).
4. Confirm it runs: `webfang --version`.

If step 4 fails, go straight to
[install-time failures](troubleshooting.md#the-downloaded-binary-wont-run).

---

## 1. Pick your asset

Every release publishes exactly four archives plus one checksum file. The
asset name carries the **full Rust target triple** — there is no friendly OS
alias and **no version in the name** (the version is in the tag/URL, not the
file). The archive contains a **bare binary**: no install script, no LICENSE,
no README inside.

| Platform | Asset (inside the release) | Format |
| :--- | :--- | :--- |
| Linux x86_64 | `webfang-x86_64-unknown-linux-gnu.tar.gz` | `.tar.gz` |
| Linux ARM64 | `webfang-aarch64-unknown-linux-gnu.tar.gz` | `.tar.gz` |
| macOS Apple Silicon | `webfang-aarch64-apple-darwin.tar.gz` | `.tar.gz` |
| Windows x86_64 | `webfang-x86_64-pc-windows-msvc.zip` | `.zip` |
| Any platform | `SHA256SUMS.txt` | checksum manifest |

**There is no Intel macOS artifact.** `x86_64-apple-darwin` is deliberately
not built: ONNX Runtime dropped x64 macOS as of 1.24.1, so `ort-sys` has no
prebuilt to link against and the `ai` feature cannot compile there. If you are
on an Intel Mac, there is nothing to mis-download — see
[Intel macOS](#intel-macos-unsupported) for what building from source costs
you.

Set the version once and reuse it. Copy the tag from
[the latest release](https://github.com/XaviCode1000/webfang/releases/latest):

```bash
VERSION=v2.4.0   # ← the tag you just looked up
```

<details>
<summary>Verify these names yourself (any release, any version)</summary>

```bash
gh release view v2.4.0 --json assets --jq '.assets[].name'
```

Observed output for `v2.4.0`:

```text
SHA256SUMS.txt
webfang-aarch64-apple-darwin.tar.gz
webfang-aarch64-unknown-linux-gnu.tar.gz
webfang-x86_64-pc-windows-msvc.zip
webfang-x86_64-unknown-linux-gnu.tar.gz
```

</details>

---

## 2. Verify the checksum

Each release ships a `SHA256SUMS.txt` listing the SHA-256 of every archive.
Verify **the archive you downloaded, before extracting it**. The command
differs per platform: `sha256sum` is GNU coreutils and does **not** exist on
stock macOS, and PowerShell has neither it nor `shasum`.

**Linux (GNU coreutils)** — checks the archive against the manifest:

```bash
sha256sum -c SHA256SUMS.txt --ignore-missing
```

**macOS (`shasum`, ships with the system)** — BSD coreutils has no
`-c`/`--ignore-missing`; check the one file you downloaded:

```bash
shasum -a 256 webfang-aarch64-apple-darwin.tar.gz
grep 'aarch64-apple-darwin' SHA256SUMS.txt
```

The two hashes must be identical.

**Windows (PowerShell)** — `Get-FileHash`, compared against the manifest line:

```powershell
(Get-FileHash webfang-x86_64-pc-windows-msvc.zip -Algorithm SHA256).Hash.ToLower()
Select-String -Path SHA256SUMS.txt -Pattern 'pc-windows-msvc'
```

If you have [GitHub CLI](https://cli.github.com) installed, it fetches both
files for you (it does **not** verify them — do step 2 anyway):

```bash
gh release download "${VERSION}" --pattern 'SHA256SUMS.txt'
gh release download "${VERSION}" --pattern 'webfang-x86_64-unknown-linux-gnu.tar.gz'
```

> The releases publish **checksums, not GitHub attestations**.
> `gh release verify-asset v2.4.0 <file>` currently fails with
> `no attestations found for tag v2.4.0` — the provenance block in the release
> body is WebFang's own L1 gate, not a Sigstore attestation. `SHA256SUMS.txt`
> is the authoritative check.

---

## 3. Install

Every recipe below is copy-pasteable as a unit. The three unix steps —
**download, verify, extract** — are identical apart from the asset name, so
only the differing part is repeated per platform.

### Linux

```bash
# 1. Download (x86_64; use aarch64-unknown-linux-gnu on ARM64)
VERSION=v2.4.0
curl -fLO "https://github.com/XaviCode1000/webfang/releases/download/${VERSION}/webfang-x86_64-unknown-linux-gnu.tar.gz"
curl -fLO "https://github.com/XaviCode1000/webfang/releases/download/${VERSION}/SHA256SUMS.txt"

# 2. Verify BEFORE extracting
sha256sum -c SHA256SUMS.txt --ignore-missing

# 3. Extract; the archive holds a single file named `webfang`
tar xzf webfang-x86_64-unknown-linux-gnu.tar.gz

# 4. Put it on PATH (system-wide)
sudo install -m 0755 webfang /usr/local/bin/webfang

#    …or per-user, no sudo:
# mkdir -p ~/.local/bin && install -m 0755 webfang ~/.local/bin/webfang
#   then add to PATH if needed: export PATH="$HOME/.local/bin:$PATH"

# 5. Confirm
webfang --version
```

Notes:

- The archive holds a **bare binary** and preserves its executable bit, so
  `tar xzf` alone is usually enough. Use `install -m 0755` (as above) to
  guarantee the bit regardless of how the archive was extracted — a
  file-manager double-click can drop it.
- The binary is `strip`ped, but it is **not** fully static: it links
  `libc`/`libm` (glibc), plus **`libstdc++` and `libgcc_s`** (BoringSSL is
  C++). All are standard on glibc distros, but a minimal or musl-based image
  will fail to load it until they are present. There is no `.deb`/`.rpm` and
  no bundled runtime to install.

### macOS (Apple Silicon)

```bash
# 1. Download
VERSION=v2.4.0
curl -fLO "https://github.com/XaviCode1000/webfang/releases/download/${VERSION}/webfang-aarch64-apple-darwin.tar.gz"
curl -fLO "https://github.com/XaviCode1000/webfang/releases/download/${VERSION}/SHA256SUMS.txt"

# 2. Verify BEFORE extracting (no sha256sum on stock macOS)
shasum -a 256 webfang-aarch64-apple-darwin.tar.gz
grep 'aarch64-apple-darwin' SHA256SUMS.txt

# 3. Extract; the archive holds a single file named `webfang`
tar xzf webfang-aarch64-apple-darwin.tar.gz

# 4. Put it on PATH (system-wide)
sudo install -m 0755 webfang /usr/local/bin/webfang

#    …or per-user:
# mkdir -p ~/.local/bin && install -m 0755 webfang ~/.local/bin/webfang

# 5. Clear the quarantine flag (see note), then confirm
xattr -d com.apple.quarantine webfang 2>/dev/null || true
webfang --version
```

> **Gatekeeper.** A binary downloaded with `curl`/`Safari` and then extracted
> carries the `com.apple.quarantine` extended attribute, and macOS kills it on
> first run with *"cannot be opened because the developer cannot be
> verified"*. It is not corrupt and the checksum will match. Remove the
> attribute with `xattr -d com.apple.quarantine webfang` (or
> `xattr -dr com.apple.quarantine <dir>`), then run it. Right-click → Open
> works too, once.

#### Intel macOS (unsupported)

**No artifact exists for Intel macOS, and that is deliberate** — it is a
decision, not a release gap, so do not go looking for a fourth `.tar.gz`.

Building from source is possible, but the `ai` feature cannot link there
(ONNX Runtime ships no x64 macOS prebuilt), so **everything semantic is
absent**: `--clean-ai`, `--offline`, `--ai-model`, `--threshold`,
`--max-tokens`, and Obsidian vault search. Scraping, crawling, and every
non-AI export format work normally. Build without `ai`
(see [from source](#from-source-developers)).

### Windows

```powershell
# 1. Download (PowerShell 5.1+)
$Version = "v2.4.0"
$Asset   = "webfang-x86_64-pc-windows-msvc.zip"
Invoke-WebRequest "https://github.com/XaviCode1000/webfang/releases/download/$Version/$Asset" -OutFile $Asset
Invoke-WebRequest "https://github.com/XaviCode1000/webfang/releases/download/$Version/SHA256SUMS.txt" -OutFile SHA256SUMS.txt

# 2. Verify BEFORE extracting
(Get-FileHash $Asset -Algorithm SHA256).Hash.ToLower()
Select-String -Path SHA256SUMS.txt -Pattern 'pc-windows-msvc'

# 3. Extract; the archive holds a single file named `webfang.exe`
Expand-Archive -Path $Asset -DestinationPath .

# 4. Put it on PATH (per-user, no admin)
$Bin = "$env:USERPROFILE\bin"
New-Item -ItemType Directory -Force -Path $Bin | Out-Null
Move-Item -Force webfang.exe "$Bin\webfang.exe"
[Environment]::SetEnvironmentVariable(
    "Path",
    [Environment]::GetEnvironmentVariable("Path", "User") + ";$Bin",
    "User")

# 5. Open a NEW terminal, then confirm
webfang --version
```

Notes:

- The binary is `webfang.exe` on Windows, `webfang` everywhere else. The
  command is `webfang` either way.
- `%USERPROFILE%\bin` avoids needing Administrator rights. The PATH edit only
  reaches **new** terminals — re-open yours before step 5.
- If the system-wide `%ProgramFiles%\webfang` route is preferred, copy
  `webfang.exe` there instead.

> **Visual C++ runtime.** The Windows artifact is built with MSVC, which links
> the **dynamic** CRT by default. A machine without the Visual C++
> Redistributable fails at startup with *"The code execution cannot proceed
> because `VCRUNTIME140.dll` was not found"*. That is the redistributable
> being absent, not a broken download — install the **Microsoft Visual C++
> Redistributable 2015-2022 (x64)** from Microsoft and re-run. Verify your
> checksum first either way, so the two causes stay distinguishable.

---

## Platform requirements

| Platform | Requirement | Why |
| :--- | :--- | :--- |
| Linux x86_64 / ARM64 | **glibc 2.39 or newer**; no musl build exists | The artifacts are built on GitHub's `ubuntu-latest` runner (Ubuntu 24.04, glibc 2.39) and link glibc directly. The floor is **inherited from the builder**, not a chosen target — so a distro older than Ubuntu 24.04 has no compatible artifact. |
| macOS | Apple Silicon only | Intel macOS has no artifact; `ai` cannot link there. |
| Windows | Visual C++ Redistributable 2015-2022 (x64) | MSVC's default dynamic CRT. |
| Any | ~17 MB download, no installer, no bundled runtime | `strip`ped binary; needs only standard system libraries (glibc + libstdc++/libgcc_s). |

If a Linux binary dies immediately with
`/lib/x86_64-linux-gnu/libc.so.6: version 'GLIBC_2.xx' not found`, the distro
is older than the floor above. There is no musl/static artifact to fall back
to; build from source instead.

---

## What is *not* in the release binary

The release build is `cargo build --release --locked --features "ai mcp" -p webfang_cli`.
Two consequences worth knowing before you file a bug:

| Missing from the release binary | Consequence |
| :--- | :--- |
| `persistence` feature | The SQLite checkpoint store is absent. File-based `--resume` state is unaffected. |
| `chromium` feature | `--js-strategy full` is rejected at preflight (it needs a Chromium downloader). The default `static` strategy and the 3-layer `hybrid` strategy are unaffected. |
| `console` feature | `tokio-console` is not available. Tracing via `--trace-file` is. |
| `adaptive-selectors` feature | Adaptive selector learning is off. |
| MCP server | The `webfang` CLI exposes no MCP flag. The MCP server lives in the separate `webfang_mcp` crate (`webfang-mcp`, `webfang-mcp-stdio`), which the release build never compiles — it is reachable from source only. See the [MCP section](https://github.com/XaviCode1000/webfang#-mcp-server) in the README. |

The `ai` feature **is** compiled into the release binary, alongside the default
`images` + `documents`. The `mcp` feature in that build command is inert: it is
an empty feature in `webfang_core` that gates no shipped code — the feature
that matters is the `webfang_mcp` *crate*, which is not in the release.

---

## From source (developers)

> Not an installation route for end users — this is the contributor path. It
> downloads and compiles every dependency, BoringSSL included.

**Prerequisites: Rust 1.88 (`rust-toolchain.toml` pins it), a C/C++ compiler,
`git`, and `cmake`.** `cmake` is mandatory: the HTTP client `wreq` → `btls`
→ `btls-sys` (formerly `boring2`/`boring-sys2`) compiles BoringSSL from C++ source on first build, and
without `cmake` the failure surfaces deep inside a build script with an error
that says nothing about the real cause.

```bash
git clone https://github.com/XaviCode1000/webfang.git
cd webfang
cmake --version        # must print a version; install cmake if empty
cargo build --release --locked --features "ai mcp" -p webfang_cli
./target/release/webfang --version
```

- `-p webfang_cli` builds only the CLI. Omitting it builds every workspace
  member.
- On Intel macOS drop `"ai mcp"` — `ai` cannot link there.
- The first build compiles BoringSSL from source and takes minutes, not
  seconds. This is normal.

---

## Uninstall

WebFang never removes anything for you. The binary itself is one file; the
**cache and output trees it creates are entirely your responsibility**.

### 1. Remove the binary

| Install location | Command |
| :--- | :--- |
| Linux, `/usr/local/bin` | `sudo rm /usr/local/bin/webfang` |
| Linux, `~/.local/bin` | `rm ~/.local/bin/webfang` |
| macOS, `/usr/local/bin` | `sudo rm /usr/local/bin/webfang` |
| Windows, `%USERPROFILE%\bin` | `Remove-Item "$env:USERPROFILE\bin\webfang.exe"` |

### 2. Remove the data

**Check where things actually live first** — the ONNX model cache is the big
one (~372 MB for the default `granite-97m`, ~1.2 GB for `granite-311m`) and
**no `WEBFANG_*` variable relocates it**. It is controlled by HuggingFace's
own `HF_HOME`. If you set `HF_HOME` to relocate the model, the default
location below is empty and deleting it recovers nothing.

```bash
# Where the webfang cache base resolves on THIS machine — run the line
# that matches your platform, and note the printed path.
echo "${XDG_CACHE_HOME:-$HOME/.cache}"          # Linux
# echo "${XDG_CACHE_HOME:-$HOME/Library/Caches}"   # macOS
# echo "%LOCALAPPDATA%\webfang"                     # Windows
```

The config base is the sibling you will also want when cleaning up:
`~/.config` (Linux), `~/Library/Application Support` (macOS), `%APPDATA%`
(Windows).

| Path (relative to the base above) | Created by | Size | Auto-cleaned? |
| :--- | :--- | :--- | :--- |
| `webfang/state/<domain>.json` | `--resume` | KBs, grows with processed-URL count | No — relocatable via `--state-dir` / `WEBFANG_STATE_DIR` |
| `webfang/state/<domain>.json.lock` | every `RecordStore` write | 0 B | **No — permanent by design.** See the note below. |
| `webfang/state/<domain>.json.bak` | migrating a stale state version | same as state | No — an existing backup is kept as-is |
| `webfang/user_agents.json` | any fetch that resolves a user-agent list | KBs | No — relocatable via `XDG_CACHE_HOME` |
| `huggingface/hub/` (under the **HF** cache base) | any `--clean-ai` run, via `hf_hub` | **372 MB** default / ~1.2 GB | No — relocatable via **`HF_HOME`**, a HuggingFace variable, *not* `WEBFANG_*` |
| `<output_dir>/` — Markdown, `export.jsonl`, `rag_dataset/`, `_inbox/` | every run | unbounded — **your data** | No — relocatable via `-o` / `WEBFANG_OUTPUT`; defaults to `output/` in the current directory |
| `webfang/config.toml` (under the **config** base: `~/.config`, `~/Library/Application Support`, `%APPDATA%`) | you, by editing it | KBs | No — relocatable via `WEBFANG_CONFIG` or `XDG_CONFIG_HOME` |

> **The `.lock` sentinel is supposed to survive.** `<domain>.json.lock` is
> created on demand and *intentionally never unlinked*; the real mutual
> exclusion is an advisory `flock` the kernel releases when the process exits,
> including after a `kill -9`. Its presence after an uninstall is correct
> behaviour, **not** a failed removal. See
> [troubleshooting.md](troubleshooting.md#leftover-lock-files-in-the-state-directory--is-my-state-corrupted).

To remove the webfang-owned cache roots and leave everything else alone:

```bash
# Linux: cache + state + UA cache, but NOT the model cache
rm -rf "${XDG_CACHE_HOME:-$HOME/.cache}/webfang"

# …and the ONNX model cache, if you want the ~372 MB back.
# Confirm you did not relocate it first, or this removes nothing:
echo "${HF_HOME:-$HOME/.cache/huggingface}"
rm -rf "${HF_HOME:-$HOME/.cache/huggingface}"
```

**Never** delete a `webfang` directory without reading the path you built
above — a relative or empty base would resolve to somewhere unintended, and
`<output_dir>` is user data with no backup.

---

## Checklist

A fresh install is done when all of these are true:

- [ ] I picked the asset whose target triple matches my machine
      (Apple Silicon, not "Intel").
- [ ] The SHA-256 of my archive matches its line in `SHA256SUMS.txt`.
- [ ] The binary is executable (`chmod +x` / `install -m 0755`) and on `PATH`.
- [ ] `webfang --version` prints a version.
- [ ] On macOS, `com.apple.quarantine` is cleared if the first run was killed.
- [ ] I know where my `output/` directory is, and that uninstalling will not
      remove it.

---

## Next step

- [Troubleshooting](troubleshooting.md) — install-time failures, slow crawls,
  WAF blocks, SSRF refusals.
- [CLI Reference](cli-reference.md) — the complete flag reference.
- [Debugging & Observability](debugging.md) — tracing, correlation IDs, and
  the `jq` query cookbook.
