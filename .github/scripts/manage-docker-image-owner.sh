#!/usr/bin/env bash
set -euo pipefail

operation="$1"
tag="$2"
marker_ref="refs/heads/automation/docker-images/$tag"

set +e
marker_line="$(git ls-remote --exit-code --refs origin "$marker_ref")"
marker_status=$?
set -e
case "$marker_status" in
  0)
    marker_commit="${marker_line%%[[:space:]]*}"
    if [[ ! "$marker_commit" =~ ^[0-9a-f]{40}$ ]]; then
      echo "The image-owner marker returned an invalid commit." >&2
      exit 1
    fi
    ;;
  2)
    marker_commit=""
    ;;
  *)
    echo "Could not inspect the image-owner marker for $tag." >&2
    exit 1
    ;;
esac

case "$operation" in
  verify)
    if [ "$marker_commit" != "$GITHUB_SHA" ]; then
      echo "Image tag $tag exists without ownership by $GITHUB_SHA." >&2
      exit 1
    fi
    ;;
  record)
    if [ "$marker_commit" = "$GITHUB_SHA" ]; then
      exit 0
    fi
    if [ -n "$marker_commit" ]; then
      if ! git merge-base --is-ancestor "$marker_commit" "$GITHUB_SHA"; then
        echo "Image tag $tag is already owned by $marker_commit." >&2
        exit 1
      fi
    fi
    git push \
      --force-with-lease="$marker_ref:$marker_commit" \
      origin "$GITHUB_SHA:$marker_ref"
    ;;
  release)
    if [ -z "$marker_commit" ]; then
      exit 0
    fi
    if [ "$marker_commit" != "$GITHUB_SHA" ]; then
      echo "Refusing to release an image-owner marker held by $marker_commit." >&2
      exit 1
    fi
    git push \
      --force-with-lease="$marker_ref:$marker_commit" \
      origin ":$marker_ref"
    ;;
  *)
    echo "Usage: $0 {verify|record|release} TAG" >&2
    exit 2
    ;;
esac
