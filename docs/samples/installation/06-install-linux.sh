# 1. Download (x86_64; use aarch64-unknown-linux-gnu on ARM64)
VERSION=v2.12.0
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
