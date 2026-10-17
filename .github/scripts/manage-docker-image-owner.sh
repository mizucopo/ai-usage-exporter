#!/usr/bin/env bash
set -euo pipefail

operation="${1:-}"
tag="${2:-}"
if [[ ! "$RELEASE_SHA" =~ ^[0-9a-f]{40}$ ]] ||
   [[ ! "$tag" =~ ^[a-zA-Z0-9_][a-zA-Z0-9_.-]{0,127}$ ]] ||
   [ "$tag" = latest ] ||
   ! git check-ref-format "refs/heads/automation/docker-images/$tag"; then
  echo "Invalid release SHA or image-owner tag." >&2
  exit 2
fi
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
    if [ -z "$marker_commit" ]; then
      python3 -I "$(dirname "$0")/release.py" image-owner-verify --image-tag "$tag"
      exit 0
    fi
    if [ "$marker_commit" != "$RELEASE_SHA" ]; then
      echo "Image tag $tag exists without ownership by $RELEASE_SHA." >&2
      exit 1
    fi
    ;;
  record)
    if [ "$marker_commit" = "$RELEASE_SHA" ]; then
      exit 0
    fi
    if [ -n "$marker_commit" ]; then
      echo "Image tag $tag is already owned by $marker_commit." >&2
      exit 1
    fi
    git push \
      --force-with-lease="$marker_ref:$marker_commit" \
      origin "$RELEASE_SHA:$marker_ref"
    ;;
  remove)
    if [ -z "$marker_commit" ]; then
      exit 0
    fi
    if [ "$marker_commit" != "$RELEASE_SHA" ]; then
      echo "Refusing to remove image ownership by $marker_commit." >&2
      exit 1
    fi
    git push --force-with-lease="$marker_ref:$RELEASE_SHA" origin ":$marker_ref"
    ;;
  *)
    echo "Usage: $0 {verify|record|remove} TAG" >&2
    exit 2
    ;;
esac
