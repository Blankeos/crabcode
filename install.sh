#!/bin/sh
# Prebuilt release installer. Also works with: curl .../install.sh | sh -s -- [version]
set -eu

say() { printf '%s\n' "$*" >&2; }
die() { say "Error: $*"; exit 1; }
valid_version() {
    case "$1" in ''|*[!0-9A-Za-z.+-]*) return 1 ;; esac
    printf '%s\n' "$1" | LC_ALL=C grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z]+([.-][0-9A-Za-z]+)*)?(\+[0-9A-Za-z]+([.-][0-9A-Za-z]+)*)?$'
}
[ "$#" -le 1 ] || die 'Usage: install.sh [version|latest]'
version=${1-latest}
case "$version" in
    latest) ;;
    *) version=${version#v}
       # Keep tags out of URLs until validated (no options, paths, or whitespace).
       valid_version "$version" || die 'Invalid version; expected a semantic version or latest' ;;
esac
for tool in curl tar awk od mktemp; do
    command -v "$tool" >/dev/null 2>&1 || die "Required tool not found: $tool"
done
if command -v sha256sum >/dev/null 2>&1; then
    hash_tool=sha256sum
elif command -v shasum >/dev/null 2>&1; then
    hash_tool=shasum
else
    die 'SHA256 verification requires sha256sum or shasum'
fi
os=$(uname -s)
arch=$(uname -m)
case "$arch" in arm64|aarch64) arch=aarch64 ;; x86_64) ;; *) die "Unsupported architecture: $arch" ;; esac
case "$os" in Darwin) target=$arch-apple-darwin ;; Linux) target=$arch-unknown-linux-gnu ;; *) die "Unsupported OS: $os" ;; esac
: "${HOME:?HOME must be set}"
install_dir=${CRABCODE_INSTALL_DIR:-$HOME/.local/bin}
case "$install_dir" in /*) ;; *) install_dir=$PWD/$install_dir ;; esac
work=$(mktemp -d "${TMPDIR:-/tmp}/crabcode-install.XXXXXXXX")
pending=
parser_pid=
probe_pid=
watchdog_pid=
cleanup() {
    [ -z "$probe_pid" ] || kill -KILL "$probe_pid" 2>/dev/null || :
    [ -z "$watchdog_pid" ] || kill "$watchdog_pid" 2>/dev/null || :
    [ -z "$parser_pid" ] || kill "$parser_pid" 2>/dev/null || :
    [ -z "$probe_pid" ] || wait "$probe_pid" 2>/dev/null || :
    [ -z "$watchdog_pid" ] || wait "$watchdog_pid" 2>/dev/null || :
    [ -z "$parser_pid" ] || wait "$parser_pid" 2>/dev/null || :
    [ -z "$pending" ] || rm -f "$pending"
    rm -rf "$work"
}
trap cleanup 0
trap 'exit 130' INT
trap 'exit 143' TERM HUP

repo=https://github.com/blankeos/crabcode
# Follow GitHub's HTTPS latest redirect; no JSON parser or API rate-limit dependency.
if [ "$version" = latest ]; then
    if ! curl --proto '=https' --proto-redir '=https' --fail --location --silent --show-error \
        --connect-timeout 10 --max-time 20 --output "$work/latest" --write-out '%{url_effective}' "$repo/releases/latest" >"$work/url"; then
        die 'Could not resolve latest release'
    fi
    tag=$(cat "$work/url")
    tag=${tag##*/}
    version=${tag#v}
    valid_version "$version" || die 'Latest release returned an invalid version'
fi
current='not installed'
installed=
if [ -x "$install_dir/crabcode" ]; then
    installed=$install_dir/crabcode
elif command -v crabcode >/dev/null 2>&1; then
    installed=$(command -v crabcode)
fi
if [ -n "$installed" ]; then
    current=unknown
    # No timeout utility on stock macOS. Probe only our child, with no access
    # to the installer's stdin (which may itself be a piped shell script).
    "$installed" --version </dev/null >"$work/version" 2>/dev/null &
    probe_pid=$!
    (
        sleep 2 & timer=$!
        trap 'kill "$timer" 2>/dev/null || :; wait "$timer" 2>/dev/null || :; exit 0' TERM INT HUP
        wait "$timer"
        kill -KILL "$probe_pid" 2>/dev/null || :
    ) &
    watchdog_pid=$!
    if wait "$probe_pid" 2>/dev/null; then
        # Accept one compact line only; never echo arbitrary terminal controls.
        candidate=$(LC_ALL=C awk 'NR==1 && length($0)<=128 { line=$0 } END { if(NR==1) print line }' "$work/version")
        candidate=${candidate#crabcode }
        if valid_version "$candidate"; then current=$candidate; fi
    fi
    probe_pid=
    kill "$watchdog_pid" 2>/dev/null || :
    wait "$watchdog_pid" 2>/dev/null || :
    watchdog_pid=
fi
say "Installed version: $current"
say ''
say "Installing crabcode version: $version"

ui=0
color=0
width=30
if [ -t 2 ] && [ "${TERM:-dumb}" != dumb ]; then
    ui=1
    [ -n "${NO_COLOR+x}" ] || color=1
    columns=${COLUMNS-}
    if [ -z "${COLUMNS+x}" ]; then
        size=$(stty size <&2 2>/dev/null || :)
        columns=${size##* }
    fi
    case "$columns" in ''|*[!0-9]*) columns=80 ;; esac
    [ "$columns" -gt 0 ] || columns=80
    available=$((columns - 1))
    width=$((available - 6))
    [ "$width" -le 50 ] || width=50
    [ "$width" -ge 0 ] || width=0
fi

# curl runs directly, not in a pipeline: its exit status and HTTP status survive.
# Parse its real, CR-delimited progress updates concurrently. Unknown-length
# transfers stay textual rather than inventing a percentage.
download() {
    url=$1
    destination=$2
    : >"$work/errors"
    phase=${3-archive}
    max_time=300
    if [ "$phase" = checksum ]; then
        say 'Verifying download...'
        max_time=20
    else
        say 'Downloading...'
    fi
    if [ "$ui" = 1 ] && [ "$phase" = archive ]; then
        rm -f "$work/progress"
        mkfifo "$work/progress"
        LC_ALL=C awk -v width="$width" -v available="$available" -v color="$color" -v errors="$work/errors" '
            BEGIN { RS="\r|\n"; shown=0 }
            /[0-9]+(\.[0-9]+)?%/ {
                match($0, /[0-9]+(\.[0-9]+)?%/)
                percent=substr($0, RSTART, RLENGTH); value=percent+0
                if (value<0 || value>100) next
                filled=int(width*value/100); bar=""; rest=""
                for(i=0;i<filled;i++) bar=bar "\342\226\210"
                for(i=filled;i<width;i++) rest=rest "."
                if(available<1) next
                printf "\r"
                if(color) printf "\033[33m"
                percent=sprintf("%3d%%", int(value))
                if(width>0) printf "%s%s %s", bar, rest, percent
                else printf "%s", substr(percent, 1, available)
                if(color) printf "\033[0m"
                fflush(); shown=1; next
            }
            NF && $0 !~ /^[ #=-]+$/ { print $0 >> errors }
            END { if(shown) printf "\n" }
        ' <"$work/progress" >&2 &
        parser_pid=$!
        if curl --proto '=https' --proto-redir '=https' --fail --location --show-error --progress-bar \
            --connect-timeout 10 --max-time "$max_time" --output "$destination" --write-out '%{http_code}' "$url" >"$work/status" 2>"$work/progress"; then
            result=0
        else result=$?; fi
        wait "$parser_pid" || :
        parser_pid=
    else
        if curl --proto '=https' --proto-redir '=https' --fail --location --silent --show-error \
            --connect-timeout 10 --max-time "$max_time" --output "$destination" --write-out '%{http_code}' "$url" >"$work/status" 2>"$work/errors"; then
            result=0
        else result=$?; fi
    fi
    http_status=$(cat "$work/status")
    [ "$result" = 0 ] || cat "$work/errors" >&2
    return "$result"
}

archive=crabcode-$target.tar.gz
base=$repo/releases/download/v$version
if ! download "$base/$archive" "$work/archive"; then
    # Older releases used xz. Only missing assets justify switching formats.
    [ "$http_status" = 404 ] || die 'Archive download failed; existing installation was not changed'
    archive=crabcode-$target.tar.xz
    download "$base/$archive" "$work/archive" || die 'Legacy archive download failed'
fi
download "$base/$archive.sha256" "$work/checksum" checksum || die 'Required checksum download failed'
expected=$(awk 'NR==1 { print $1 }' "$work/checksum")
[ "${#expected}" = 64 ] || die 'Invalid SHA256 checksum'
case "$expected" in *[!0-9a-fA-F]*) die 'Invalid SHA256 checksum' ;; esac
if [ "$hash_tool" = sha256sum ]; then
    actual=$(sha256sum "$work/archive" | awk '{print $1}')
else
    actual=$(shasum -a 256 "$work/archive" | awk '{print $1}')
fi
expected=$(printf '%s' "$expected" | tr 'A-F' 'a-f')
[ "$actual" = "$expected" ] || die 'SHA256 checksum mismatch; existing installation was not changed'

# Never unpack untrusted paths into HOME. Validate the inventory, then stream
# exactly one regular binary into staging (symlinks and hardlinks are refused).
tar -tf "$work/archive" >"$work/members" || die 'Invalid release archive'
member=$(awk '
    /^\// || /^-/ || /(^|\/)\.\.(\/|$)/ { bad=1 }
    /(^|\/)crabcode$/ { candidate=$0; count++ }
    END { if(bad || count!=1) exit 1; print candidate }
' "$work/members") || die 'Unsafe archive or missing/duplicate crabcode binary'
tar -tvf "$work/archive" "$member" >"$work/type" || die 'Could not inspect binary'
case "$(head -c 1 "$work/type")" in -) ;; *) die 'Archive binary is not a regular file' ;; esac
tar -xOf "$work/archive" "$member" >"$work/crabcode" || die 'Could not extract binary'
[ -s "$work/crabcode" ] || die 'Archive binary is empty'
chmod 755 "$work/crabcode"

# JSON-escape arbitrary prefix bytes, including quotes, backslashes and control
# characters, without relying on Python, Node, or unsafe shell interpolation.
json_string() {
    printf '%s' "$1" | od -An -v -tu1 | LC_ALL=C awk '
        BEGIN { printf "\"" }
        { for(i=1;i<=NF;i++) {
            n=$i+0
            if(n==34 || n==92) printf "\\%c", n
            else if(n<32) printf "\\u%04x", n
            else printf "%c", n
        } }
        END { printf "\"" }
    '
}
mkdir -p "$install_dir"
install_dir=$(cd "$install_dir" && pwd -P)
receipt_dir=${XDG_CONFIG_HOME:-$HOME/.config}/crabcode
mkdir -p "$receipt_dir"
{
    printf '{"binaries":["crabcode"],"binary_aliases":{},"install_prefix":'
    json_string "$install_dir"
    printf ',"install_layout":"flat","version":'
    json_string "$version"
    printf '}\n'
} >"$work/receipt"
# Copy beside the destination and rename atomically, even across filesystems.
pending=$(mktemp "$install_dir/.crabcode.XXXXXXXX")
cp "$work/crabcode" "$pending"
chmod 755 "$pending"
mv -f "$pending" "$install_dir/crabcode"
pending=
pending=$(mktemp "$receipt_dir/.receipt.XXXXXXXX")
cp "$work/receipt" "$pending"
mv -f "$pending" "$receipt_dir/crabcode-receipt.json"
pending=
say "Installed crabcode $version to $install_dir/crabcode"
case ":$PATH:" in *":$install_dir:"*) ;; *) say "Add $install_dir to your PATH to run crabcode." ;; esac
