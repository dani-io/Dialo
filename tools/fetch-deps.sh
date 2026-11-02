#!/usr/bin/env bash
# Puts libre and baresip, at the release Dialo is built against, in vendor/.
# build.rs compiles them into the binary, so nothing of baresip is needed at run time.
set -euo pipefail
cd "$(dirname "$0")/.."

RE_TAG=v4.12.0
BARESIP_TAG=v4.12.0

fetch() { # name tag
    local name=$1 tag=$2
    if [ -f "vendor/$name/.dialo-tag" ] && [ "$(cat "vendor/$name/.dialo-tag")" = "$tag" ]; then
        return
    fi
    rm -rf "vendor/$name"
    git -c advice.detachedHead=false clone -q --depth 1 --branch "$tag" "https://github.com/baresip/$name.git" "vendor/$name"
    rm -rf "vendor/$name/.git"
    echo "$tag" > "vendor/$name/.dialo-tag"
    echo "vendor/$name: $tag"
}

mkdir -p vendor
fetch re "$RE_TAG"
fetch baresip "$BARESIP_TAG"
