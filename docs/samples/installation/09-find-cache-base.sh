# Where the webfang cache base resolves on THIS machine — run the line
# that matches your platform, and note the printed path.
echo "${XDG_CACHE_HOME:-$HOME/.cache}"           # Linux
# echo "${XDG_CACHE_HOME:-$HOME/Library/Caches}"   # macOS
# echo "$env:LOCALAPPDATA"                         # Windows (PowerShell)
