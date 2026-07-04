#!/usr/bin/env bash
# pipe-demo.sh — benday's primary flow: a query engine emits rows on stdout,
# benday draws them. Here `echo` stands in for the query engine; the payload is
# the columnar envelope an MCP query tool emits as `structuredContent`.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"

# Run benday from source so the demo always tracks the current build. Swap in
# an installed `benday` binary if you have one on PATH.
benday() { cargo run --quiet --manifest-path "$here/../Cargo.toml" -p benday -- "$@"; }

# 1) Columnar envelope on stdin, spec via --spec. The declared column types
#    (STRING, INT64) beat inference; the unknown `query` key is ignored;
#    `truncated`/`total_rows` surface under --meta.
echo '{
  "columns": [{"name":"day","type":"STRING"},{"name":"signups","type":"INT64"}],
  "rows": [["mon",32],["tue",78],["wed",51],["thu",94],["fri",67]],
  "total_rows": 5,
  "truncated": false,
  "query": {"job_id":"demo-1234","note":"benday ignores this"}
}' | benday --spec '{"title":"weekly signups","mark":"bar","encoding":{"x":{"field":"day"},"y":{"field":"signups"}}}' --meta

# 2) The other accepted stdin shape: a bare JSON array of row objects.
echo
echo '[{"day":"mon","signups":32},{"day":"tue","signups":78},{"day":"wed","signups":51}]' \
  | benday --spec '{"mark":"line","encoding":{"x":{"field":"day"},"y":{"field":"signups"}}}'
