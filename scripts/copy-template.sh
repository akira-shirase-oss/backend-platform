#!/usr/bin/env bash
# Copies the starter template into a new directory and renames it for a product
# (template/docs/COPYING.md). The copy is the product's from then on: nothing in it refers
# back to this repository except TEMPLATE_ORIGIN, which records the commit it came from so
# later template changes can be pulled as a reviewed diff.
#
#   scripts/copy-template.sh DEST NAME BAND [--verify]
#
# NAME is 2-16 lower-case letters and digits, starting with a letter (`durable`, `smon`).
# BAND is the product's own port band, <low>-<high> above 60999, written to the copy's
# scripts/port-band and reserved in this repository's port-bands registry: a band that
# overlaps a registered one — the platform's own included — is refused, so two copies
# can never bind the same ports on one machine. It renames the crates (starter-http ->
# NAME-http, ...), the operator tool (NAME-ctl -> NAMEctl), the variables (STARTER_ ->
# NAME_), the database roles, the unit, the paths and the compose project. --verify then
# builds and tests the copy (its own target directory under the copy; needs
# STARTER_TEST_DATABASE_URL, an administrator).
set -euo pipefail
case "${1:-}" in -h|--help) sed -n '2,/^set -euo/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'; exit 0 ;; esac
case "${1:-}" in -*) echo "copy-template.sh: unknown option $1 (see --help)" >&2; exit 2 ;; esac
cd "$(dirname "$0")/.."

dest="${1:?usage: copy-template.sh DEST NAME BAND [--verify] (see --help)}"
name="${2:?usage: copy-template.sh DEST NAME BAND [--verify] (see --help)}"
band="${3:?usage: copy-template.sh DEST NAME BAND [--verify] (see --help)}"
verify="${4:-}"
case "$verify" in ""|--verify) ;; *) echo "copy-template.sh: unknown argument $verify (see --help)" >&2; exit 2 ;; esac
[[ "$name" =~ ^[a-z][a-z0-9]{1,15}$ ]] || { echo "error: NAME must be 2-16 of [a-z0-9], starting with a letter" >&2; exit 2; }
[ "$name" != starter ] || { echo "error: pick the product's own name" >&2; exit 2; }
[[ "$band" =~ ^[0-9]{2,5}-[0-9]{2,5}$ ]] || { echo "error: BAND must be <low>-<high>, got '$band'" >&2; exit 2; }
band_low="${band%%-*}" band_high="${band##*-}"
if (( 10#$band_low <= 60999 || 10#$band_low > 10#$band_high || 10#$band_high > 65535 )); then
  echo "error: BAND must be <low>-<high> above the ephemeral range (60999), got '$band'" >&2
  exit 2
fi
# The band registry reserves one contiguous band per product. PORT_BANDS_FILE
# overrides the registry path (tests point it at a scratch file).
registry="${PORT_BANDS_FILE:-port-bands}"
[ -f "$registry" ] || { echo "error: no port-band registry at $registry" >&2; exit 2; }
if [ "$band" = "61400-61499" ]; then
  echo "error: 61400-61499 is the platform's own band; a copy needs its own" >&2
  exit 2
fi
while IFS=' ' read -r owner reserved _; do
  case "$owner" in ''|'#'*) continue ;; esac
  if [ "$owner" = "$name" ]; then
    echo "error: $registry already reserves a band for $name" >&2
    exit 2
  fi
  rlow="${reserved%%-*}" rhigh="${reserved##*-}"
  if (( 10#$band_low <= 10#$rhigh && 10#$band_high >= 10#$rlow )); then
    echo "error: band $band overlaps $owner's reserved band $reserved in $registry" >&2
    exit 2
  fi
done < "$registry"
if [ -e "$dest" ] && [ -n "$(ls -A "$dest" 2>/dev/null)" ]; then
  echo "error: $dest exists and is not empty" >&2
  exit 2
fi
[ -e "$dest" ] || mkdir -p "$dest"
(cd template && tar --exclude ./target -cf - .) | tar -xf - -C "$dest"

python3 -I - "$dest" "$name" <<'PY'
import os, re, sys
dest, name = sys.argv[1], sys.argv[2]
upper = name.upper()
rules = [
    # (?<![A-Za-z0-9]) rather than \b: an underscore before the name (CARGO_BIN_EXE_...)
    # is a word character, so \b would miss it.
    (re.compile(r"(?<![A-Za-z0-9])starter(?=[-_](http|db|accounts|server|attacks)\b)"), name),
    (re.compile(r"(?<![A-Za-z0-9])starterctl\b"), f"{name}ctl"),
    (re.compile(r"(?<![A-Z0-9])STARTER_"), f"{upper}_"),
    (re.compile(r"\bstarter_(owner|app|test_|probe_|drill_|rehearsal_|control_|session|digest_scratch|install|load_|dev)"), name + r"_\1"),
    # The unit and the user named in commands (the operator guide, the install test).
    (re.compile(r"(^|(?<=[\s(/]))(id|nologin|--service|is-active|-u|--value|Environment|stop|restart|enable) starter\b", re.M), r"\2 " + name),
    (re.compile(r"= starter \]"), f"= {name} ]"),
    (re.compile(r"__Host-starter_session"), f"__Host-{name}_session"),
    (re.compile(r"\bstarter\.service\b"), f"{name}.service"),
    (re.compile(r"/starter\.conf\b"), f"/{name}.conf"),
    (re.compile(r"\bjournald-starter\.conf\b"), f"journald-{name}.conf"),
    (re.compile(r"/(opt|etc|var/lib)/starter\b"), r"/\1/" + name),
    (re.compile(r"^(User|Group|StateDirectory)=starter$", re.M), r"\1=" + name),
    (re.compile(r'application_name\("starter"\)'), f'application_name("{name}")'),
    (re.compile(r"@starter>"), f"@{name}>"),
    (re.compile(r"\bstarter-template\b"), name),
    (re.compile(r"\bstarter-(owner|app|admin)-dev"), name + r"-\1-dev"),
    (re.compile(r"POSTGRES_(USER|DB): starter$", re.M), r"POSTGRES_\1: " + name),
    (re.compile(r"--username=starter --dbname=starter"), f"--username={name} --dbname={name}"),
    (re.compile(r"postgres://starter:"), f"postgres://{name}:"),
    (re.compile(r"(127\.0\.0\.1:\$\{port\})/starter\b"), r"\1/" + name),
    (re.compile(r"\bstarter-(prove|drill|drill-controls|config|load|copy)\b"), name + r"-\1"),
]
changed = 0
for root, dirs, files in os.walk(dest):
    dirs[:] = [d for d in dirs if d not in ("target", ".git")]
    for f in files:
        path = os.path.join(root, f)
        try:
            text = open(path, encoding="utf-8", newline="").read()
        except UnicodeDecodeError:
            # Only the fuzz corpus may hold non-UTF-8 bytes carrying a template
            # name; anywhere else a non-text fixture is left alone and the
            # leftover-name check below reports it rather than risk a rewrite
            # in an encoding surrogateescape cannot represent.
            if "/tests/attacks/fuzz/corpus/" not in path:
                continue
            raw = open(path, "rb").read()
            text = raw.decode("utf-8", errors="surrogateescape")
            new = text
            for pattern, replacement in rules:
                new = pattern.sub(replacement, new)
            if new != text:
                open(path, "wb").write(new.encode("utf-8", errors="surrogateescape"))
                changed += 1
            continue
        new = text
        for pattern, replacement in rules:
            new = pattern.sub(replacement, new)
        if new != text:
            open(path, "w", encoding="utf-8", newline="").write(new)
            changed += 1
for crate in ("http", "db", "accounts", "server"):
    os.rename(os.path.join(dest, "crates", f"starter-{crate}"), os.path.join(dest, "crates", f"{name}-{crate}"))
for old, new in (
    ("deploy/starter.service", f"deploy/{name}.service"),
    ("deploy/journald-starter.conf", f"deploy/journald-{name}.conf"),
    (f"crates/{name}-server/src/bin/starterctl.rs", f"crates/{name}-server/src/bin/{name}ctl.rs"),
):
    os.rename(os.path.join(dest, old), os.path.join(dest, new))
left = [os.path.join(r, f) for r, ds, fs in os.walk(dest) for f in fs + ds if "starter" in f]
if left:
    sys.exit(f"file names the rename missed: {left}")
print(f"renamed for {name}: {changed} files rewritten")
PY

commit="$(git rev-parse HEAD)"
cat >"$dest/TEMPLATE_ORIGIN" <<EOF
# Where this copy came from. Pull later template changes as a reviewed diff
# (docs/COPYING.md, "Pulling template changes"):
#   git -C <backend-platform> diff $commit <newer> -- template/
template-repository: https://github.com/akira-shirase-oss/backend-platform
template-commit: $commit
copied: $(date -u +%Y-%m-%d)
name: $name
port-band: $band
EOF
# The copy's band is its own, recorded in its check's source and reserved here.
printf '%s\n' "$band" >"$dest/scripts/port-band"
printf '%s %s\n' "$name" "$band" >>"$registry"
echo "copied template/ at $commit to $dest as $name (band $band reserved in $registry)"

# The rename changes crate-name lengths, which rustfmt and Clippy see: format the copy so
# it passes `cargo fmt --check` out of the box. Whatever cargo is first on PATH is used —
# under rustup it resolves the copy's rust-toolchain.toml pin.
if ! command -v cargo >/dev/null 2>&1; then
  echo "note: no cargo on PATH; run 'cargo fmt --all' in the copy before scripts/check.sh" >&2
elif ! (cd "$dest" && cargo fmt --all); then
  echo "note: 'cargo fmt --all' failed in the copy; run it there before scripts/check.sh" >&2
fi

if [ "$verify" = --verify ]; then
  : "${STARTER_TEST_DATABASE_URL:?set it to an administrator URL to verify the copy}"
  upper="$(tr '[:lower:]' '[:upper:]' <<<"$name")"
  (
    cd "$dest"
    export CARGO_TARGET_DIR="$PWD/target"
    export "${upper}_TEST_DATABASE_URL=$STARTER_TEST_DATABASE_URL"
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets --locked -q -- -D warnings
    cargo test --workspace --locked -q 2>&1 | grep -E 'test result|FAILED|panicked' || true
    cargo test --workspace --locked -q >/dev/null 2>&1
  )
  # -a and LC_ALL=C: GNU grep skips lines it cannot decode, which is exactly
  # where a non-UTF-8 leftover (a fuzz corpus seed) would hide.
  left="$(LC_ALL=C grep -rIna --exclude-dir=target -E '(^|[^A-Za-z0-9])starter[-_]|(^|[^A-Z0-9])STARTER_|starterctl|(id|nologin|--service|is-active|-u|--value|Environment|stop|restart|enable) starter\b' "$dest" | grep -v TEMPLATE_ORIGIN || true)"
  if [ -n "$left" ]; then
    echo "error: names the rename missed:" >&2
    echo "$left" >&2
    exit 1
  fi
  echo "copy verified: it builds, passes clippy and its tests, and no template name is left"
fi
