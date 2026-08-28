#!/usr/bin/env bash
set -euo pipefail

repo="origincreativegroup/crush"
tag="models-v1"
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
manifest="$root/crates/core/model-manifest-v1.json"

credential="$(printf 'protocol=https\nhost=github.com\n\n' | git credential fill)"
token="$(sed -n 's/^password=//p' <<<"$credential")"
if [[ -z "$token" ]]; then
  echo "GitHub credential helper did not return a password/token" >&2
  exit 1
fi

api() {
  curl --fail-with-body --silent --show-error \
    -H "Authorization: Bearer $token" \
    -H "Accept: application/vnd.github+json" \
    -H "X-GitHub-Api-Version: 2022-11-28" \
    "$@"
}

assets=(
  "clip-image.onnx:$root/models/clip-image.onnx"
  "clip-text.onnx:$root/models/clip-text.onnx"
  "bpe_simple_vocab_16e6.txt.gz:$root/models/bpe_simple_vocab_16e6.txt.gz"
  "ggml-base.bin:$root/models/ggml-base.bin"
  "ggml-small.bin:$root/models/ggml-small.bin"
  "manifest.json:$manifest"
)

for asset in "${assets[@]}"; do
  name="${asset%%:*}"
  path="${asset#*:}"
  [[ -f "$path" ]] || { echo "missing release asset: $path" >&2; exit 1; }
  if [[ "$name" != "manifest.json" ]]; then
    expected_bytes="$(jq -r --arg name "$name" '.files[$name].bytes' "$manifest")"
    expected_sha="$(jq -r --arg name "$name" '.files[$name].sha256' "$manifest")"
    actual_bytes="$(stat -f '%z' "$path")"
    actual_sha="$(shasum -a 256 "$path" | awk '{print $1}')"
    [[ "$actual_bytes" == "$expected_bytes" ]] || {
      echo "$name byte count does not match manifest" >&2
      exit 1
    }
    [[ "$actual_sha" == "$expected_sha" ]] || {
      echo "$name sha256 does not match manifest" >&2
      exit 1
    }
  fi
done

release="$(
  api "https://api.github.com/repos/$repo/releases?per_page=100" |
    jq -c --arg tag "$tag" '.[] | select(.tag_name == $tag)' |
    head -1
)"
if [[ -z "$release" ]]; then
  payload="$(jq -nc \
    --arg tag "$tag" \
    --arg body 'Pinned CLIP and Whisper model assets for Crush. Checksums and reference verification are recorded in manifest.json.' \
    '{tag_name:$tag,target_commitish:"main",name:$tag,body:$body,draft:true,prerelease:false}')"
  release="$(api -X POST "https://api.github.com/repos/$repo/releases" --data-binary "$payload")"
fi

release_id="$(jq -r '.id' <<<"$release")"
release_assets="$(api "https://api.github.com/repos/$repo/releases/$release_id/assets?per_page=100")"
for asset in "${assets[@]}"; do
  name="${asset%%:*}"
  path="${asset#*:}"
  bytes="$(stat -f '%z' "$path")"
  existing_bytes="$(jq -r --arg name "$name" '.[] | select(.name == $name) | .size' <<<"$release_assets")"
  if [[ -n "$existing_bytes" ]]; then
    [[ "$existing_bytes" == "$bytes" ]] || {
      echo "release asset $name exists with $existing_bytes bytes, expected $bytes" >&2
      exit 1
    }
    echo "release asset already present: $name"
    continue
  fi
  content_type="application/octet-stream"
  [[ "$name" == "manifest.json" ]] && content_type="application/json"
  echo "uploading $name ($bytes bytes)"
  api -X POST \
    -H "Content-Type: $content_type" \
    "https://uploads.github.com/repos/$repo/releases/$release_id/assets?name=$name" \
    --data-binary "@$path" >/dev/null
done

payload='{"draft":false}'
published="$(api -X PATCH "https://api.github.com/repos/$repo/releases/$release_id" --data-binary "$payload")"
echo "published $(jq -r '.html_url' <<<"$published")"
