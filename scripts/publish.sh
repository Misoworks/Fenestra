#!/usr/bin/env bash
# Publish the public Fenestra crates to crates.io in dependency order.
#
# Usage:
#   scripts/publish.sh                # publish all publishable crates
#   scripts/publish.sh --dry-run      # dry-run only, nothing uploaded
#   scripts/publish.sh --allow-dirty  # also pass through to cargo
#   scripts/publish.sh --package-only # like --dry-run, but skip the verify step
#                                     # (needed when upstream crates that we depend on
#                                     # haven't been published to crates.io yet)
#
# Required:
#   - The four upstream stuk-* crates that fenestra-cef depends on must
#     already be on crates.io. Publish them first from the stuk repo with
#     `stuk/scripts/publish.sh`.
#   - A valid crates.io token (`cargo login <TOKEN>`).
set -euo pipefail

cd "$(dirname "$0")/.."

DRY_RUN=""
PACKAGE_ONLY=""
CARGO_EXTRA=()
for arg in "$@"; do
  case "$arg" in
    --dry-run) DRY_RUN=1 ;;
    --package-only) PACKAGE_ONLY=1; CARGO_EXTRA+=(--no-verify) ;;
    *) CARGO_EXTRA+=("$arg") ;;
  esac
done

# Publishable crates in dependency order.
CRATES=(
  fenestra-runtime
  fenestra-cli
  fenestra-cef
)

if [[ -n "$DRY_RUN" || -n "$PACKAGE_ONLY" ]]; then
  echo "package-only run for ${#CRATES[@]} fenestra crates:"
  printf '  - %s\n' "${CRATES[@]}"
  echo
  for crate in "${CRATES[@]}"; do
    echo "==> packaging $crate"
    cargo package -p "$crate" "${CARGO_EXTRA[@]}"
  done
  echo
  echo "all fenestra crates packaged. run without --dry-run/--package-only to publish."
  exit 0
fi

echo "publishing ${#CRATES[@]} fenestra crates in dependency order:"
printf '  - %s\n' "${CRATES[@]}"
echo

for crate in "${CRATES[@]}"; do
  echo "==> publishing $crate"
  cargo publish -p "$crate" "${CARGO_EXTRA[@]}"
  echo "waiting 30s for crates.io index to update..."
  sleep 30
done

echo
echo "all fenestra crates published."
