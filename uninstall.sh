#!/bin/bash
# omarchyvr: remove everything an install changed (details: installer/uninstall.sh --help, docs/UNINSTALL.md)
exec "$(dirname "$(readlink -f "$0")")/installer/uninstall.sh" "$@"
