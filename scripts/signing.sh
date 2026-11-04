#!/bin/sh
# Shared packaging policy. Source and resolve before changing release artifacts.
macscan_signing_identities() {
  /usr/bin/security find-identity -v -p codesigning |
    sed -n 's/^[[:space:]]*[0-9][0-9]*) \([A-Fa-f0-9]*\) "\(Developer ID Application:.*\)"$/\1|\2/p'
}

macscan_resolve_signing_identity() {
  if [ "${MACSCAN_SIGNING_IDENTITY:-}" = - ]; then
    macscan_signing_identity=-
    echo 'Macscan signing: explicit ad hoc (local only).' >&2
    return
  fi
  macscan_identities=$(macscan_signing_identities) || return
  if [ -n "${MACSCAN_SIGNING_IDENTITY:-}" ]; then
    macscan_matches=$(printf '%s\n' "$macscan_identities" | awk -F '|' -v requested="$MACSCAN_SIGNING_IDENTITY" '$1 == requested || $2 == requested {print $1}')
  else
    macscan_matches=$(printf '%s\n' "$macscan_identities" | awk -F '|' 'NF == 2 {print $1}')
  fi
  macscan_match_count=$(printf '%s\n' "$macscan_matches" | awk 'NF {n++} END {print n+0}')
  case "$macscan_match_count" in
    1)
      macscan_signing_identity=$macscan_matches
      macscan_signing_team=$(printf '%s\n' "$macscan_identities" | awk -F '|' -v selected="$macscan_signing_identity" '$1 == selected {print $2}' | sed -n 's/.*(\([^()]*\))$/\1/p')
      [ -n "$macscan_signing_team" ] || return 3
      ;;
    0)
      if [ -n "${MACSCAN_SIGNING_IDENTITY:-}" ]; then
        echo 'Requested MACSCAN_SIGNING_IDENTITY is not an available Developer ID Application identity.' >&2
        return 3
      fi
      macscan_signing_identity=-
      echo 'Macscan signing: no Developer ID Application identity; using ad hoc (local only).' >&2
      ;;
    *)
      echo 'Multiple Developer ID Application identities match; set MACSCAN_SIGNING_IDENTITY to the exact name or SHA-1 hash (or - for ad hoc).' >&2
      return 3
      ;;
  esac
}

macscan_sign() {
  # Stable identifiers keep designated requirements independent of code hashes.
  if [ "$macscan_signing_identity" = - ]; then
    /usr/bin/codesign --force --sign - --identifier "$2" "$1"
  else
    /usr/bin/codesign --force --sign "$macscan_signing_identity" \
      --identifier "$2" --options runtime --timestamp "$1"
  fi
}

macscan_prepare_duckdb() {
  # Timestamp signatures differ between runs. Share exact signed bytes so CLI/app
  # release coherence remains verifiable by the installer's existing SHA-256 check.
  macscan_duckdb_hash=$(/usr/bin/shasum -a 256 "$project_dir/.build/duckdb/libduckdb.dylib" | awk '{print $1}')
  macscan_signed_duckdb="$project_dir/.build/signed-duckdb/$macscan_duckdb_hash-$macscan_signing_identity.dylib"
  if [ -L "$project_dir/.build/signed-duckdb" ] || [ -L "$macscan_signed_duckdb" ]; then
    echo 'Refusing symlink signing cache.' >&2
    return 3
  fi
  if [ ! -f "$macscan_signed_duckdb" ]; then
    (
      mkdir -p "$project_dir/.build/signed-duckdb"
      staging=$(mktemp "$project_dir/.build/signed-duckdb/.signing.XXXXXX")
      trap 'rm -f "$staging"' EXIT HUP INT TERM
      /usr/bin/install -m 755 "$project_dir/.build/duckdb/libduckdb.dylib" "$staging"
      macscan_sign "$staging" dk.macscan.duckdb
      /usr/bin/codesign --verify --strict "$staging"
      mv "$staging" "$macscan_signed_duckdb"
    ) || return
  fi
  macscan_verify_duckdb
}

macscan_verify_duckdb() (
  set -eu
  /usr/bin/codesign --verify --strict "$macscan_signed_duckdb"
  details=$(/usr/bin/codesign -dv --verbose=4 "$macscan_signed_duckdb" 2>&1)
  printf '%s\n' "$details" | /usr/bin/grep -qx 'Identifier=dk.macscan.duckdb'
  if [ "$macscan_signing_identity" = - ]; then
    printf '%s\n' "$details" | /usr/bin/grep -qx 'Signature=adhoc'
  else
    printf '%s\n' "$details" | /usr/bin/grep -qx "TeamIdentifier=$macscan_signing_team"
    cert_dir=$(mktemp -d "$project_dir/.build/signed-duckdb/.cert.XXXXXX")
    trap 'rm -rf "$cert_dir"' EXIT HUP INT TERM
    /usr/bin/codesign -d --extract-certificates="$cert_dir/cert" "$macscan_signed_duckdb" 2>/dev/null
    actual_identity=$(/usr/bin/shasum -a 1 "$cert_dir/cert0" | awk '{print toupper($1)}')
    [ "$actual_identity" = "$macscan_signing_identity" ]
  fi
)
