# 1. Download
VERSION=v2.12.0
curl -fLO "https://github.com/XaviCodeDev1000/webfang/releases/download/${VERSION}/webfang-aarch64-apple-darwin.tar.gz"
curl -fLO "https://github.com/XaviCodeDev1000/webfang/releases/download/${VERSION}/SHA256SUMS.txt"

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
