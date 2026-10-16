#!/bin/bash
# omarchyvr: the one command after `git clone --recursive` (details: installer/install.sh --help, docs/INSTALL.md)
exec "$(dirname "$(readlink -f "$0")")/installer/install.sh" "$@"
