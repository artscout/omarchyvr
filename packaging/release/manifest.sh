#!/bin/bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# manifest.sh: write MANIFEST.txt of a release tree (sourced by build-release.sh and by the installer tests).
#
#   write_manifest DIR HEADER_FILE MARKERS_FILE LIBS_FILE
#
# HEADER_FILE: "key: value" lines (version, commit, fork commits, ...). MARKERS_FILE: "marker: count" lines.
# LIBS_FILE: the tab-separated decision list of bundle-libs.py (kind, soname, source).
# The file ends with [links] (path TAB target) and [sha256] (sha256sum -c format, every regular file except MANIFEST.txt).
# installer/lib.sh verify_release reads exactly this format.
write_manifest() {
	local dir=$1 header=$2 markers=$3 libs=$4 m tmp
	m=$dir/MANIFEST.txt
	tmp=$(mktemp)
	{
		echo "omarchyvr release manifest v1"
		cat "$header"
		echo "[markers]"
		cat "$markers"
		echo "[bundled-libs]"
		cat "$libs"
		echo "[links]"
		(cd "$dir" && find . -type l -print0 | LC_ALL=C sort -z | while IFS= read -r -d '' l; do
			l=${l#./}
			printf '%s\t%s\n' "$l" "$(readlink "$dir/$l")"
		done)
		echo "[sha256]"
		(cd "$dir" && find . -type f ! -path ./MANIFEST.txt -print0 | LC_ALL=C sort -z | xargs -0 -r sha256sum | sed 's|  \./|  |')
	} >"$tmp"
	mv -f "$tmp" "$m"
	chmod 644 "$m"
}
