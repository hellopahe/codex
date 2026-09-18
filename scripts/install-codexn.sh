#!/bin/sh
set -eu

repo_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
if [ "$#" -eq 2 ] && [ "$1" = "--from-binary" ]; then
    custom_binary=$2
elif [ "$#" -eq 0 ]; then
    (cd "$repo_dir/codex-rs" && cargo build --release -p codex-cli --bin codex)
    custom_binary="$repo_dir/codex-rs/target/release/codex"
else
    printf '%s\n' 'Usage: install-codexn.sh [--from-binary /path/to/codex]' >&2
    exit 2
fi

test -x "$custom_binary"
custom_install_dir="$HOME/.local/lib/codexn"
custom_bin_dir="$HOME/.local/bin"
mkdir -p "$custom_install_dir" "$custom_bin_dir"
cp "$custom_binary" "$custom_install_dir/codex.new"
chmod 755 "$custom_install_dir/codex.new"
mv "$custom_install_dir/codex.new" "$custom_install_dir/codex"
cat > "$custom_bin_dir/codexn.new" <<'LAUNCHER'
#!/bin/sh
export CODEX_NETWORK_MONITOR="${CODEX_NETWORK_MONITOR:-1}"
exec "$HOME/.local/lib/codexn/codex" "$@"
LAUNCHER
chmod 755 "$custom_bin_dir/codexn.new"
mv "$custom_bin_dir/codexn.new" "$custom_bin_dir/codexn"
printf 'Installed %s\n' "$custom_bin_dir/codexn"
"$custom_bin_dir/codexn" --version
