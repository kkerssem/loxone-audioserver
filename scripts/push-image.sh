#!/bin/sh
# Builds the arm64 image and pushes it to a registry, for a MikroTik App (which pulls images
# from a registry; it cannot import a tar file). Normally CI does this on every push to main
# (.github/workflows/ci.yml); use this script for a manual push or another registry.
#
#   scripts/push-image.sh            # pushes ghcr.io/kkerssem/loxone-audioserver:<commit>,
#                                    # and :latest when run on main (the App pulls :latest)
#   IMAGE_REPO=ghcr.io/you/loxone-audioserver scripts/push-image.sh
#
# Log in first (a GitHub token with the write:packages scope):
#   container registry login ghcr.io
# A new package on ghcr.io is private. RouterOS Apps document no registry login, so make the
# package public on GitHub (Package settings -> Change visibility); the image contains only the
# binary and CA certificates, no configuration or keys.
set -eu

IMAGE_REPO=${IMAGE_REPO:-ghcr.io/kkerssem/loxone-audioserver}
cd "$(dirname "$0")/.."

# Untracked files count too: the build copies all of src/, so an untracked source file would
# end up in an image tagged with a commit that does not contain it. Ignored files are fine.
if [ -n "$(git status --porcelain)" ]; then
    echo "the working tree has uncommitted or untracked files; commit first so the tag matches the code" >&2
    exit 1
fi
# same 7-character tag as CI
TAG=$(git rev-parse --short=7 HEAD)
TAGS=$TAG
if [ "$(git rev-parse --abbrev-ref HEAD)" = main ]; then
    TAGS="$TAG latest"
else
    echo "not on main: pushing only :$TAG, not :latest" >&2
fi

container build --platform linux/arm64 -t "loxone-audioserver:$TAG" .
for t in $TAGS; do
    container image tag "loxone-audioserver:$TAG" "$IMAGE_REPO:$t"
    container image push --platform linux/arm64 "$IMAGE_REPO:$t"
done
echo "pushed $IMAGE_REPO: $TAGS"
