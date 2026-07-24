#!/usr/bin/env bash
set -euo pipefail

CRATES=(
  webfang_core
  webfang_ai
)

TMPFILE=$(mktemp)
trap 'rm -f "$TMPFILE"' EXIT

for crate in "${CRATES[@]}"; do
  cargo check -p "$crate" --all-features --message-format=json 2>/dev/null \
    | jq -r --arg crate "$crate" '
        select(
          .reason == "compiler-message"
          and (.message.code != null)
          and .message.code.code == "missing_docs"
        )
        | [
            $crate,
            (.message.spans[0].file_name // "unknown"),
            (.message.spans[0].line_start // 0),
            (.message.message // "missing documentation")
          ]
        | @tsv
      '
done >> "$TMPFILE"

# Deduplicate by file:line (columns 2 and 3)
# Keep the first crate name seen for each unique file:line
sort -t$'\t' -k2,2 -k3,3n "$TMPFILE" | awk -F'\t' '
  !seen[$2,$3]++ { print }
' > "${TMPFILE}.dedup"

cat "${TMPFILE}.dedup"
echo "---"
echo "# Total unique warnings: $(wc -l < "${TMPFILE}.dedup")"
