#!/usr/bin/env bash
# cargo's rustc-wrapper (.cargo/config.toml; a wrapper takes no arguments of its own): tools/vr-busy.sh --guard.
exec "$(dirname "$0")/vr-busy.sh" --guard "$@"
