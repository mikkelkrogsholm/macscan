#!/bin/sh
set -eu
project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd -P)
. "$project_dir/scripts/signing.sh"
# Fake identity inventory exercises policy without requiring private certificates.
macscan_signing_identities() { printf '%s\n' "$inventory"; }
MACSCAN_SIGNING_IDENTITY=
inventory=
macscan_resolve_signing_identity
[ "$macscan_signing_identity" = - ]
inventory='AAAA|Developer ID Application: Example A (TEAM)'
macscan_resolve_signing_identity
[ "$macscan_signing_identity" = AAAA ]
MACSCAN_SIGNING_IDENTITY='Developer ID Application: Example A (TEAM)'
macscan_resolve_signing_identity
[ "$macscan_signing_identity" = AAAA ]
MACSCAN_SIGNING_IDENTITY=AAAA
macscan_resolve_signing_identity
[ "$macscan_signing_identity" = AAAA ]
MACSCAN_SIGNING_IDENTITY=missing
if macscan_resolve_signing_identity; then exit 1; fi
inventory="$inventory
BBBB|Developer ID Application: Example B (TEAM)"
MACSCAN_SIGNING_IDENTITY=
if macscan_resolve_signing_identity; then exit 1; fi
MACSCAN_SIGNING_IDENTITY=BBBB
macscan_resolve_signing_identity
[ "$macscan_signing_identity" = BBBB ]
MACSCAN_SIGNING_IDENTITY=-
macscan_resolve_signing_identity
[ "$macscan_signing_identity" = - ]
echo 'Signing selector tests passed.'
