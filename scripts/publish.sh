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
  # Package all crates in a single `cargo package` invocation. This is
  # required: cargo only spins up the temporary overlay registry that lets
  # it resolve workspace-internal deps against the in-progress publish when
  # more than one package is being packaged in the same call (see the
  # `do_package` function in cargo's `cargo_package` source — the overlay
  # is gated on `deps.has_dependencies()`). Per-crate calls like
  # `cargo package -p fenestra-cli` would try to resolve `fenestra-runtime`
  # against crates.io and fail because we haven't published it yet.
  package_args=()
  for crate in "${CRATES[@]}"; do
    package_args+=(-p "$crate")
  done
  cargo package "${package_args[@]}" "${CARGO_EXTRA[@]}"
  echo
  echo "all fenestra crates packaged. run without --dry-run/--package-only to publish."
  exit 0
fi

echo "publishing ${#CRATES[@]} fenestra crates in dependency order:"
printf '  - %s\n' "${CRATES[@]}"
echo

# Publish a single crate, retrying on crates.io 429 rate limits.
#
# Captures stderr/stdout. On a 429, parses the "try again after <RFC2822>" hint
# from the error and sleeps until then (plus a small buffer) before retrying.
# On any other failure, the captured output is echoed and the function returns
# non-zero so the caller can abort.
#
# The default max attempts is high enough to ride out a multi-hour crates.io
# rate-limit cooldown (which can be 30+ minutes for new accounts). Override
# with FENESTRA_PUBLISH_MAX_ATTEMPTS.
publish_crate() {
  local crate="$1"
  local max_attempts="${FENESTRA_PUBLISH_MAX_ATTEMPTS:-20}"
  local attempt=0
  local out status

  while (( attempt < max_attempts )); do
    attempt=$((attempt + 1))
    echo "==> publishing $crate (attempt $attempt)"

    set +e
    out=$(cargo publish -p "$crate" "${CARGO_EXTRA[@]}" 2>&1)
    status=$?
    set -e

    if (( status == 0 )); then
      echo "$out"
      # Let the crates.io index settle so the next crate can resolve its dep.
      echo "waiting 30s for crates.io index to update..."
      sleep 30
      return 0
    fi

    # 429 from crates.io looks like:
    #   ...status 429 Too Many Requests): You have published too many new
    #   crates in a short period of time. Please try again after Sun, 14
    #   Jun 2026 14:59:56 GMT and see...
    if [[ "$out" == *"status 429"* ]]; then
      local when
      when=$(printf '%s' "$out" | grep -oP 'try again after \K[^.]+' | head -1 | sed 's/[[:space:]]*$//')
      if [[ -n "$when" ]]; then
        local epoch now wait wait_min wait_sec
        epoch=$(date -d "$when" +%s 2>/dev/null || echo "")
        if [[ -n "$epoch" ]]; then
          now=$(date +%s)
          wait=$((epoch - now + 15))
          if (( wait > 0 )); then
            wait_min=$((wait / 60))
            wait_sec=$((wait % 60))
            echo "rate limited until $when; sleeping ${wait_min}m${wait_sec}s before retry..."
            sleep "$wait"
            continue
          fi
        fi
      fi
      # 429 but couldn't parse the wait time — back off and retry.
      echo "rate limited; backing off 60s before retry..."
      sleep 60
      continue
    fi

    # Version already on crates.io: treat as success so the script is
    # idempotent and can be re-run safely to publish the crates that didn't
    # make it on a previous attempt.
    if [[ "$out" == *"already exists at version"* ]] \
       || [[ "$out" == *"already exists"* && "$out" == *"at registry"* ]]; then
      echo "$out"
      echo "$crate is already on crates.io, skipping"
      return 0
    fi

    # Non-rate-limit failure: surface the cargo output and bail.
    echo "$out"
    return "$status"
  done

  echo "gave up publishing $crate after $max_attempts attempts"
  return 1
}

for crate in "${CRATES[@]}"; do
  publish_crate "$crate"
done

echo
echo "all fenestra crates published."
