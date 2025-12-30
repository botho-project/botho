#!/bin/bash
# Convenient fuzzing script for Cadence/Botho
# Usage: ./scripts/fuzz.sh [command] [target] [options]

set -e

FUZZ_DIR="$(cd "$(dirname "$0")/.." && pwd)/fuzz"
LOG_DIR="$FUZZ_DIR/logs"
CORPUS_DIR="$FUZZ_DIR/corpus"

# Available targets
TARGETS=(
    fuzz_address_parsing
    fuzz_block
    fuzz_lion_signature
    fuzz_mlkem_decapsulation
    fuzz_network_messages
    fuzz_pq_keys
    fuzz_pq_transaction
    fuzz_ring_signature
    fuzz_rpc_request
    fuzz_transaction
)

# Durations in seconds
DURATION_QUICK=300      # 5 minutes
DURATION_MEDIUM=1800    # 30 minutes
DURATION_LONG=3600      # 1 hour
DURATION_OVERNIGHT=28800 # 8 hours

usage() {
    cat <<EOF
Fuzzing helper for Cadence

Usage: $0 <command> [target] [options]

Commands:
    list            List all available fuzz targets
    quick <target>  Run target for 5 minutes
    medium <target> Run target for 30 minutes
    long <target>   Run target for 1 hour
    overnight       Run ALL targets for 8 hours each (sequentially)
    parallel        Run ALL targets in parallel for 1 hour each
    run <target>    Run target indefinitely (Ctrl+C to stop)
    timed <target> <seconds>  Run target for specific duration
    status          Show any running fuzz processes
    stop            Stop all running fuzz processes
    coverage        Show corpus coverage stats

Targets:
$(printf '    %s\n' "${TARGETS[@]}")

Examples:
    $0 quick fuzz_block           # 5 min test of block fuzzer
    $0 overnight                  # Run all targets overnight
    $0 parallel                   # Run all targets in parallel
    $0 timed fuzz_transaction 600 # Run for 10 minutes
    $0 run fuzz_block             # Run indefinitely

EOF
    exit 1
}

ensure_nightly() {
    if ! rustup run nightly rustc --version &>/dev/null; then
        echo "Error: Rust nightly toolchain required"
        echo "Install with: rustup toolchain install nightly"
        exit 1
    fi
}

validate_target() {
    local target=$1
    for t in "${TARGETS[@]}"; do
        if [[ "$t" == "$target" ]]; then
            return 0
        fi
    done
    echo "Error: Unknown target '$target'"
    echo "Available targets: ${TARGETS[*]}"
    exit 1
}

run_fuzz() {
    local target=$1
    local duration=$2
    local background=${3:-false}

    mkdir -p "$LOG_DIR"
    local log_file="$LOG_DIR/${target}_$(date +%Y%m%d_%H%M%S).log"

    echo "Starting $target..."
    if [[ -n "$duration" ]]; then
        echo "Duration: $((duration / 60)) minutes ($duration seconds)"
    else
        echo "Duration: indefinite (Ctrl+C to stop)"
    fi
    echo "Log file: $log_file"
    echo ""

    cd "$FUZZ_DIR"

    local args="-print_final_stats=1"
    if [[ -n "$duration" ]]; then
        args="$args -max_total_time=$duration"
    fi

    if [[ "$background" == "true" ]]; then
        nohup cargo +nightly fuzz run "$target" -- $args > "$log_file" 2>&1 &
        echo "PID: $!"
    else
        cargo +nightly fuzz run "$target" -- $args 2>&1 | tee "$log_file"
    fi
}

cmd_list() {
    echo "Available fuzz targets:"
    printf '  %s\n' "${TARGETS[@]}"
}

cmd_quick() {
    validate_target "$1"
    run_fuzz "$1" $DURATION_QUICK
}

cmd_medium() {
    validate_target "$1"
    run_fuzz "$1" $DURATION_MEDIUM
}

cmd_long() {
    validate_target "$1"
    run_fuzz "$1" $DURATION_LONG
}

cmd_overnight() {
    echo "=== Overnight Fuzzing Session ==="
    echo "Running ${#TARGETS[@]} targets for 8 hours each"
    echo "Total estimated time: $((${#TARGETS[@]} * 8)) hours"
    echo "Started: $(date)"
    echo ""

    mkdir -p "$LOG_DIR"
    local session_log="$LOG_DIR/overnight_$(date +%Y%m%d_%H%M%S).log"

    for target in "${TARGETS[@]}"; do
        echo "----------------------------------------"
        echo "[$target] Starting at $(date)"
        run_fuzz "$target" $DURATION_OVERNIGHT
        echo "[$target] Completed at $(date)"
        echo ""
    done | tee "$session_log"

    echo "=== Overnight Session Complete ==="
    echo "Finished: $(date)"
}

cmd_parallel() {
    echo "=== Parallel Fuzzing Session ==="
    echo "Running ${#TARGETS[@]} targets in parallel for 1 hour each"
    echo "Started: $(date)"
    echo ""

    mkdir -p "$LOG_DIR"
    local pids=()

    for target in "${TARGETS[@]}"; do
        echo "Starting $target in background..."
        run_fuzz "$target" $DURATION_LONG true
        pids+=($!)
        sleep 2  # Stagger starts slightly
    done

    echo ""
    echo "All targets started. PIDs: ${pids[*]}"
    echo "Logs in: $LOG_DIR"
    echo ""
    echo "Waiting for completion (1 hour)..."

    for pid in "${pids[@]}"; do
        wait "$pid" 2>/dev/null || true
    done

    echo ""
    echo "=== Parallel Session Complete ==="
    echo "Finished: $(date)"
}

cmd_run() {
    validate_target "$1"
    run_fuzz "$1" ""
}

cmd_timed() {
    validate_target "$1"
    local duration=$2
    if [[ -z "$duration" ]] || ! [[ "$duration" =~ ^[0-9]+$ ]]; then
        echo "Error: Please specify duration in seconds"
        exit 1
    fi
    run_fuzz "$1" "$duration"
}

cmd_status() {
    echo "Running fuzz processes:"
    if pgrep -f "cargo.*fuzz" > /dev/null; then
        ps aux | grep -E "cargo.*fuzz|libfuzzer" | grep -v grep
    else
        echo "  (none)"
    fi

    echo ""
    echo "Recent logs:"
    if [[ -d "$LOG_DIR" ]]; then
        ls -lt "$LOG_DIR" 2>/dev/null | head -6
    else
        echo "  (no logs yet)"
    fi
}

cmd_stop() {
    echo "Stopping all fuzz processes..."
    pkill -f "cargo.*fuzz" 2>/dev/null || true
    pkill -f "fuzz_" 2>/dev/null || true
    sleep 1

    if pgrep -f "cargo.*fuzz" > /dev/null; then
        echo "Force killing remaining processes..."
        pkill -9 -f "cargo.*fuzz" 2>/dev/null || true
        pkill -9 -f "fuzz_" 2>/dev/null || true
    fi

    echo "Done."
}

cmd_coverage() {
    echo "Corpus coverage stats:"
    echo ""

    for target in "${TARGETS[@]}"; do
        local corpus_path="$CORPUS_DIR/$target"
        if [[ -d "$corpus_path" ]]; then
            local count=$(find "$corpus_path" -type f | wc -l | tr -d ' ')
            local size=$(du -sh "$corpus_path" 2>/dev/null | cut -f1)
            printf "  %-30s %5s items  %s\n" "$target" "$count" "$size"
        else
            printf "  %-30s (no corpus)\n" "$target"
        fi
    done
}

# Main
ensure_nightly

case "${1:-}" in
    list)       cmd_list ;;
    quick)      cmd_quick "$2" ;;
    medium)     cmd_medium "$2" ;;
    long)       cmd_long "$2" ;;
    overnight)  cmd_overnight ;;
    parallel)   cmd_parallel ;;
    run)        cmd_run "$2" ;;
    timed)      cmd_timed "$2" "$3" ;;
    status)     cmd_status ;;
    stop)       cmd_stop ;;
    coverage)   cmd_coverage ;;
    *)          usage ;;
esac
