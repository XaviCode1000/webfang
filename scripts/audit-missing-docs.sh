#!/usr/bin/env bash
set -euo pipefail

CRATES=(
  webfang_core
  webfang_ai
)

for crate in "${CRATES[@]}"; do
  echo "# Missing docs: ${crate}"

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
done
