#!/usr/bin/env bash
set -euo pipefail
radice="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
exec "$radice/lume" "$@"
