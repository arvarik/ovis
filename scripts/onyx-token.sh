#!/usr/bin/env bash
# Mint the Onyx token OVIS uses for connector actions.
#
# Without one, every action endpoint (pause, resume, run-once, prune, cc-pair
# delete, targeted reindex, boost, hide) answers 503 ONYX_UNCONFIGURED. Reads are
# unaffected.
#
#     scripts/onyx-token.sh                      # prompts for the password
#     scripts/onyx-token.sh --list               # show existing tokens
#     scripts/onyx-token.sh --revoke <id>        # revoke one
#
# Environment:
#     ONYX_API_URL     default http://192.168.4.113:8080
#     ONYX_ADMIN_EMAIL default admin@example.com
#     ONYX_ADMIN_PASSWORD  read interactively if unset (never echoed, never
#                          stored, never passed on a command line where `ps`
#                          could see it)
#
# Why a personal access token and not an API key: `POST /admin/api-key` is
# paywalled on this Onyx edition —
#     402 {"error_code":"FEATURE_NOT_AVAILABLE","required_tier":"business"}
# — and returns that before it even looks at credentials, which is why the
# `api_key` table is empty. A PAT (`POST /user/pats`, gated only on basic access)
# is presented the same way, as `Authorization: Bearer …`, so OVIS accepts either.
# This script tries the API-key endpoint first anyway, in case the edition ever
# changes.
set -euo pipefail

ONYX="${ONYX_API_URL:-http://192.168.4.113:8080}"
ONYX="${ONYX%/}"
EMAIL="${ONYX_ADMIN_EMAIL:-admin@example.com}"
TOKEN_NAME="${ONYX_TOKEN_NAME:-ovis}"
ACTION="${1:-create}"

COOKIES="$(mktemp)"
PWFILE="$(mktemp)"
chmod 600 "$COOKIES" "$PWFILE"
trap 'rm -f "$COOKIES" "$PWFILE"' EXIT

die() { echo "error: $*" >&2; exit 1; }

command -v curl >/dev/null || die "curl is required"

# ---------------------------------------------------------------------------
# 1. Log in. Onyx uses fastapi-users with a cookie transport, so the session
#    arrives as a `fastapiusersauth` cookie rather than a JSON token.
# ---------------------------------------------------------------------------
if [ -z "${ONYX_ADMIN_PASSWORD:-}" ]; then
  printf 'Onyx password for %s: ' "$EMAIL" >&2
  read -rs ONYX_ADMIN_PASSWORD
  printf '\n' >&2
fi
[ -n "$ONYX_ADMIN_PASSWORD" ] || die "no password given"

# The password goes through a 0600 temp file rather than curl's argv, which is
# world-readable via `ps` for the life of the request. `printf` rather than
# `echo` so no trailing newline becomes part of the password.
printf '%s' "$ONYX_ADMIN_PASSWORD" > "$PWFILE"
unset ONYX_ADMIN_PASSWORD

# `|| true` so a connection failure reaches the message below instead of
# tripping `set -e` and dumping a raw curl error.
login_status=$(
  curl -s -o /dev/null -w '%{http_code}' --connect-timeout 5 --max-time 30 -c "$COOKIES" \
    -X POST "$ONYX/auth/login" \
    --data-urlencode "username=$EMAIL" \
    --data-urlencode "password@$PWFILE" || true
)
[ -n "$login_status" ] && [ "$login_status" != "000" ] ||
  die "could not reach $ONYX — check ONYX_API_URL and that the host is up"
case "$login_status" in
  2*) ;;
  400|401) die "Onyx rejected those credentials (HTTP $login_status)" ;;
  *) die "login failed with HTTP $login_status — is $ONYX reachable?" ;;
esac
grep -q fastapiusersauth "$COOKIES" || die "login succeeded but set no session cookie"
echo "logged in as $EMAIL" >&2

case "$ACTION" in
  --list|list)
    curl -sS -b "$COOKIES" "$ONYX/user/pats"
    echo
    exit 0
    ;;
  --revoke|revoke)
    id="${2:?usage: $0 --revoke <token_id>}"
    curl -sS -b "$COOKIES" -X DELETE "$ONYX/user/pats/$id"
    echo
    exit 0
    ;;
esac

# ---------------------------------------------------------------------------
# 2. Try the documented API-key endpoint, then fall back to a PAT.
# ---------------------------------------------------------------------------
api_key_body=$(
  curl -sS -b "$COOKIES" -X POST "$ONYX/admin/api-key" \
    -H 'Content-Type: application/json' \
    -d "{\"name\":\"$TOKEN_NAME\",\"api_key_role\":\"admin\"}" || true
)
token=$(printf '%s' "$api_key_body" | sed -n 's/.*"api_key"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')

if [ -n "$token" ]; then
  echo "minted an admin API key" >&2
else
  case "$api_key_body" in
    *FEATURE_NOT_AVAILABLE*)
      echo "API keys are paywalled on this Onyx edition; minting a personal access token" >&2 ;;
    *) echo "API key endpoint unavailable; minting a personal access token" >&2 ;;
  esac

  # scopes:null = full user access, which for an ADMIN user is what the
  # /manage/admin/* endpoints require. expiration_days:null = no expiry, so the
  # server does not start failing actions months from now with no warning.
  pat_body=$(
    curl -sS -b "$COOKIES" -X POST "$ONYX/user/pats" \
      -H 'Content-Type: application/json' \
      -d "{\"name\":\"$TOKEN_NAME\",\"expiration_days\":null,\"scopes\":null}"
  )
  token=$(printf '%s' "$pat_body" | sed -n 's/.*"token"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')
  [ -n "$token" ] || die "could not mint a token. Onyx said: $pat_body"
  echo "minted a personal access token" >&2
fi

# Onyx returns the raw token exactly once; there is no way to read it back.
cat <<EOF >&2

Add this to your .env (it will not be shown again):

EOF
echo "ONYX_API_KEY=$token"
cat <<EOF >&2

Then restart OVIS and confirm:

    curl -fsS localhost:8080/api/v1/system/health | jq .onyx_api
    # -> {"configured": true, "status": "ok", ...}

EOF
