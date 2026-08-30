#!/bin/sh
# Build the demo model the container serves, from the batch beside this file.
#
#   sh deploy/demo/build-model.sh <amcli binary> <output .archimate>
#
# The seed is pinned, so this is reproducible: the same batch and the same
# seed give the same bytes, which is also why the Docker build can do it
# instead of the repository carrying a generated file.
set -eu

AMCLI=${1:?usage: build-model.sh <amcli> <out.archimate>}
OUT=${2:?usage: build-model.sh <amcli> <out.archimate>}
HERE=$(dirname "$0")

AMCLI_ID_SEED=meridian
export AMCLI_ID_SEED

rm -f "$OUT"
"$AMCLI" init "Meridian Insurance" -o "$OUT" -q
"$AMCLI" -m "$OUT" apply "$HERE/meridian.jsonl" -q
"$AMCLI" -m "$OUT" validate -q
"$AMCLI" -m "$OUT" info
