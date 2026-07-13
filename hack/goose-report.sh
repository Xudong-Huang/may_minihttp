#!/usr/bin/env bash
# Extract structured metrics from goose test output.
#
# Reads goose stdout (which contains the print_goose_report() output),
# parses key metrics, and produces:
#   - JSON report (written to --output)
#   - Markdown table (printed to stdout)
#
# Usage: hack/goose-report.sh --output report.json < goose-stdout.txt

set -euo pipefail

OUTPUT=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        --output) OUTPUT="$2"; shift 2 ;;
        *) shift ;;
    esac
done

if [[ -z "$OUTPUT" ]]; then
    echo "Usage: $0 --output <file> < input.txt" >&2
    exit 1
fi

# Read stdin into variable
GOOSE_STDOUT="$(cat)"

# Extract only the last [REPORT] block (the most recent test run),
# since all 6 test reports are concatenated in the log.
GOOSE_STDOUT=$(echo "$GOOSE_STDOUT" | awk '/^\[REPORT\]/{buf=""} {buf=buf $0 "\n"} END{printf "%s", buf}')

# Parse from print_goose_report() output
# Use `tail -1` to extract only the last (most recent) test's metrics,
# since the log contains all 6 test runs concatenated together.
total_users=$(echo "$GOOSE_STDOUT" | grep -oP 'Total users spawned: \K\d+' | tail -1 || echo "0")
total_requests=$(echo "$GOOSE_STDOUT" | grep -oP 'Total requests: +\K\d+' | tail -1 || echo "0")
successful_requests=$(echo "$GOOSE_STDOUT" | grep -oP 'Successful requests: +\K\d+' | tail -1 || echo "0")
failed_requests=$(echo "$GOOSE_STDOUT" | grep -oP 'Failed requests: +\K\d+' | tail -1 || echo "0")

# Extract per-transaction metrics from Response Times section
# Goose outputs two patterns:
#   "  GET /path:" (normal: method + path)
#   "  GET GET :"   (double-word: just the method repeated)
# followed by indented Requests/Average/Min/Max lines
transactions=""
method=""
path=""
req_count=""
avg_ms=""
min_ms=""
max_ms=""

extract_number() {
    local val="${1:-0}"
    # Strip trailing 'ms' if present
    val="${val%%ms*}"
    echo "$val"
}

while IFS= read -r line; do
    # Detect method line: "  GET /path:" or "  GET GET :"
    if [[ "$line" =~ ^[[:space:]]+(GET|POST|PUT|DELETE|PATCH|HEAD|OPTIONS)[[:space:]]+(.+):[[:space:]]*$ ]]; then
        # Flush previous transaction
        if [[ -n "$method" && -n "$req_count" ]]; then
            transactions="${transactions}
{\"method\":\"${method}\",\"path\":\"${path}\",\"requests\":${req_count},\"avg_ms\":$(extract_number "$avg_ms"),\"min_ms\":$(extract_number "$min_ms"),\"max_ms\":$(extract_number "$max_ms")}"
        fi
        method="${BASH_REMATCH[1]}"
        # Second capture could be "GET" (double-word) or "/path" (normal)
        path="${BASH_REMATCH[2]}"
        req_count=""
        avg_ms=""
        min_ms=""
        max_ms=""
    elif [[ -n "$method" ]]; then
        if [[ "$line" =~ Requests:[[:space:]]+([0-9]+) ]]; then
            req_count="${BASH_REMATCH[1]}"
        elif [[ "$line" =~ Average:[[:space:]]+([0-9.]+)ms ]]; then
            avg_ms="${BASH_REMATCH[1]}"
        elif [[ "$line" =~ Min:[[:space:]]+([0-9.]+)ms ]]; then
            min_ms="${BASH_REMATCH[1]}"
        elif [[ "$line" =~ Max:[[:space:]]+([0-9.]+)ms ]]; then
            max_ms="${BASH_REMATCH[1]}"
        fi
    fi
done <<< "$GOOSE_STDOUT"

# Flush last transaction
if [[ -n "$method" && -n "$req_count" ]]; then
    transactions="${transactions}
{\"method\":\"${method}\",\"path\":\"${path}\",\"requests\":${req_count},\"avg_ms\":$(extract_number "$avg_ms"),\"min_ms\":$(extract_number "$min_ms"),\"max_ms\":$(extract_number "$max_ms")}"
fi

# Build JSON - write transaction data to temp file for reliable JSON construction
TXN_FILE=$(mktemp)
printf '%s\n' "$transactions" > "$TXN_FILE"

python3 -c "
import json, sys

rate_args = [int(sys.argv[i]) for i in range(1, 5)]
txn_file = sys.argv[5]
output_file = sys.argv[6]

try:
    rate = rate_args[2] / (rate_args[1] + 1)
except:
    rate = 0.0

txns = []
with open(txn_file) as f:
    for line in f:
        line = line.strip()
        if not line:
            continue
        try:
            txns.append(json.loads(line))
        except:
            pass

report = {
    'total_users': rate_args[0],
    'total_requests': rate_args[1],
    'successful_requests': rate_args[2],
    'failed_requests': rate_args[3],
    'success_rate': round(rate, 4),
    'transactions': txns
}
with open(output_file, 'w') as f:
    json.dump(report, f, indent=2)
" "$total_users" "$total_requests" "$successful_requests" "$failed_requests" "$TXN_FILE" "$OUTPUT"

rm -f "$TXN_FILE"

# Print markdown to stdout
echo "### Load Test Report"
echo ""
echo "| Metric | Value |"
echo "|--------|-------|"
echo "| Total users spawned | ${total_users} |"
echo "| Total requests | ${total_requests} |"
echo "| Successful requests | ${successful_requests} |"
echo "| Failed requests | ${failed_requests} |"
echo ""
echo "#### Per-Transaction"
echo ""
echo "| Method | Path | Requests | Avg (ms) | Min (ms) | Max (ms) |"
echo "|--------|------|----------|----------|----------|----------|"

while IFS= read -r tline; do
    if [[ -z "$tline" ]]; then continue; fi
    t_method=$(echo "$tline" | grep -oP '"method":"\K[^"]+')
    t_path=$(echo "$tline" | grep -oP '"path":"\K[^"]+')
    t_req=$(echo "$tline" | grep -oP '"requests":\K\d+')
    t_avg=$(echo "$tline" | grep -oP '"avg_ms":\K[0-9.]+')
    t_min=$(echo "$tline" | grep -oP '"min_ms":\K[0-9.]+')
    t_max=$(echo "$tline" | grep -oP '"max_ms":\K[0-9.]+')
    if [[ -n "$t_method" ]]; then
        echo "| ${t_method} | ${t_path} | ${t_req} | ${t_avg} | ${t_min} | ${t_max} |"
    fi
done <<< "$transactions"
echo ""
