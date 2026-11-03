#!/usr/bin/env bash
# Builds the release archive dist/anvil-<version>-linux-x86_64.tar.gz (ADR-0015 §8).
#
#   scripts/package.sh            build and package
#   scripts/package.sh --verify   build and package twice from scratch, fail unless the two
#                                 archives are byte-identical
#
# SOURCE_DATE_EPOCH (default: the HEAD commit time) fixes every timestamp. Needs the system
# packages the build needs (libshaderc), a clean `cargo build --release` environment, GNU tar.
set -euo pipefail

cd "$(dirname "$0")/.."
root=$PWD

verify=0
case "${1:-}" in
    "") ;;
    --verify) verify=1 ;;
    *) echo "usage: scripts/package.sh [--verify]" >&2; exit 2 ;;
esac

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)
name=anvil-$version-linux-x86_64
export SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH:-$(git log -1 --format=%ct)}
commit=$(git rev-parse HEAD)
[ -z "$(git status --porcelain)" ] || commit=$commit-dirty
target=$root/target/package
dist=$root/dist

# Builds into a fresh target dir (same path both times: the path is remapped, but a build
# script may still see it) and writes the archive to $1.
build_and_pack() {
    local out=$1 stage
    rm -rf "$target"
    # Local paths must not leak into the binaries. Replaces RUSTFLAGS on purpose.
    RUSTFLAGS="--remap-path-prefix=$root=/build --remap-path-prefix=${CARGO_HOME:-$HOME/.cargo}=/cargo --remap-path-prefix=$(rustc --print sysroot)=/rust" \
        CARGO_TARGET_DIR=$target \
        cargo build --release --locked -p player -p editor

    stage=$(mktemp -d)
    trap 'rm -rf "$stage"' RETURN
    local dir=$stage/$name
    mkdir -p "$dir/bin" "$dir/samples" "$dir/docs"
    cp "$target/release/anvil-player" "$target/release/anvil-editor" "$dir/bin/"
    # The sample project without its import cache.
    (cd samples && tar -c --exclude=.anvil sample-project) | tar -x -C "$dir/samples"
    cp docs/user/USER-GUIDE.md docs/user/LIMITATIONS.md CHANGELOG.md "$dir/docs/"
    [ ! -f LICENSE ] || cp LICENSE "$dir/"
    cat > "$dir/VERSION" <<EOF
version: $version
commit: $commit
rustc: $(rustc --version)
date: $(date -u -d "@$SOURCE_DATE_EPOCH" +%Y-%m-%dT%H:%M:%SZ)
EOF
    (cd "$dir" && find . -type f ! -name SHA256SUMS | LC_ALL=C sort | xargs sha256sum > SHA256SUMS)
    chmod -R u=rwX,go=rX "$dir"

    mkdir -p "$(dirname "$out")"
    tar --sort=name --mtime="@$SOURCE_DATE_EPOCH" --owner=0 --group=0 --numeric-owner \
        --format=gnu -C "$stage" -cf - "$name" | gzip -n -9 > "$out"
}

mkdir -p "$dist"
archive=$dist/$name.tar.gz
if [ "$verify" = 1 ]; then
    build_and_pack "$dist/verify-1.tar.gz"
    build_and_pack "$dist/verify-2.tar.gz"
    a=$(sha256sum < "$dist/verify-1.tar.gz" | cut -d' ' -f1)
    b=$(sha256sum < "$dist/verify-2.tar.gz" | cut -d' ' -f1)
    echo "build 1: $a"
    echo "build 2: $b"
    if [ "$a" != "$b" ]; then
        echo "NOT reproducible; compare with: mkdir x y; tar -xzf dist/verify-1.tar.gz -C x; tar -xzf dist/verify-2.tar.gz -C y; diff -r x y" >&2
        exit 1
    fi
    mv "$dist/verify-1.tar.gz" "$archive"
    rm "$dist/verify-2.tar.gz"
    echo "reproducible"
else
    build_and_pack "$archive"
fi
(cd "$dist" && sha256sum "$name.tar.gz" | tee "$name.tar.gz.sha256")
