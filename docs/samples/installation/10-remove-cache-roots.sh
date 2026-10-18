# Linux / macOS: cache + state, but NOT the model cache
rm -rf "${XDG_CACHE_HOME:-$HOME/.cache}/webfang"     # Linux
rm -rf "${XDG_CACHE_HOME:-$HOME/Library/Caches}/webfang"  # macOS
