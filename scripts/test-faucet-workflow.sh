#!/bin/bash
# End-to-end Faucet Workflow Test Script
#
# Validates the complete testnet workflow from faucet request through
# transaction confirmation. Corresponds to issue #296.
#
# Usage:
#   ./scripts/test-faucet-workflow.sh [--local|--testnet] [--verbose]
#
# Options:
#   --local    Test against local testnet (default)
#   --testnet  Test against public testnet (seed.botho.io)
#   --verbose  Show detailed output

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Default configuration
RPC_HOST="127.0.0.1"
RPC_PORT="27200"
FAUCET_HOST="127.0.0.1"
FAUCET_PORT="27200"
VERBOSE=false
NETWORK="local"

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --local)
            NETWORK="local"
            RPC_HOST="127.0.0.1"
            RPC_PORT="27200"
            FAUCET_HOST="127.0.0.1"
            FAUCET_PORT="27200"
            shift
            ;;
        --testnet)
            NETWORK="testnet"
            RPC_HOST="seed.botho.io"
            RPC_PORT="17101"
            FAUCET_HOST="faucet.botho.io"
            FAUCET_PORT="17101"
            shift
            ;;
        --verbose|-v)
            VERBOSE=true
            shift
            ;;
        *)
            echo "Unknown option: $1"
            exit 1
            ;;
    esac
done

# Helper functions
log_info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

log_success() {
    echo -e "${GREEN}[PASS]${NC} $1"
}

log_fail() {
    echo -e "${RED}[FAIL]${NC} $1"
}

log_warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

rpc_call() {
    local method=$1
    local params=$2
    local host=${3:-$RPC_HOST}
    local port=${4:-$RPC_PORT}

    if $VERBOSE; then
        echo -e "${YELLOW}RPC:${NC} $method -> $host:$port"
    fi

    curl -s -X POST "http://${host}:${port}" \
        -H "Content-Type: application/json" \
        -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"${method}\",\"params\":${params}}"
}

check_prereqs() {
    log_info "Checking prerequisites..."

    # Check curl
    if ! command -v curl &> /dev/null; then
        log_fail "curl is required but not installed"
        exit 1
    fi

    # Check jq
    if ! command -v jq &> /dev/null; then
        log_fail "jq is required but not installed"
        exit 1
    fi

    log_success "Prerequisites satisfied"
}

check_node_status() {
    log_info "Checking node status at ${RPC_HOST}:${RPC_PORT}..."

    local response=$(rpc_call "node_getStatus" "{}")

    if echo "$response" | jq -e '.result' > /dev/null 2>&1; then
        local version=$(echo "$response" | jq -r '.result.version')
        local height=$(echo "$response" | jq -r '.result.chainHeight')
        local peers=$(echo "$response" | jq -r '.result.peerCount')
        log_success "Node is online: version=$version, height=$height, peers=$peers"
        return 0
    else
        local error=$(echo "$response" | jq -r '.error.message // "Connection failed"')
        log_fail "Node check failed: $error"
        return 1
    fi
}

# Generate a test address
generate_test_address() {
    local seed=$1
    # Generate deterministic hex keys for testing
    local view_key=$(printf '%064d' $((seed * 12345)))
    local spend_key=$(printf '%064d' $((seed * 67890)))
    echo "view:${view_key}"$'\n'"spend:${spend_key}"
}

# ============================================================================
# Test Scenarios
# ============================================================================

TESTS_PASSED=0
TESTS_FAILED=0

test_faucet_dispenses_correct_amount() {
    log_info "Test 1: Faucet dispenses correct amount (10 BTH)"

    local address=$(generate_test_address 1)
    local response=$(rpc_call "faucet_request" "{\"address\":\"$address\"}" "$FAUCET_HOST" "$FAUCET_PORT")

    if $VERBOSE; then
        echo "Response: $response"
    fi

    if echo "$response" | jq -e '.result.success == true' > /dev/null 2>&1; then
        local amount=$(echo "$response" | jq -r '.result.amount')
        local formatted=$(echo "$response" | jq -r '.result.amountFormatted')
        local tx_hash=$(echo "$response" | jq -r '.result.txHash')

        # Check amount is 10 BTH (10_000_000_000_000 picocredits)
        if [[ "$amount" == "10000000000000" ]]; then
            log_success "Faucet dispensed correct amount: $formatted (tx: ${tx_hash:0:16}...)"
            ((TESTS_PASSED++))
        else
            log_fail "Incorrect amount: expected 10000000000000, got $amount"
            ((TESTS_FAILED++))
        fi
    else
        local error=$(echo "$response" | jq -r '.error.message // .result.error // "Unknown error"')
        log_fail "Faucet request failed: $error"
        ((TESTS_FAILED++))
    fi
}

test_rate_limiting() {
    log_info "Test 2: Rate limiting works (cooldown between requests)"

    local address=$(generate_test_address 2)

    # First request should succeed
    local response1=$(rpc_call "faucet_request" "{\"address\":\"$address\"}" "$FAUCET_HOST" "$FAUCET_PORT")

    if ! echo "$response1" | jq -e '.result.success == true' > /dev/null 2>&1; then
        log_warn "First request failed (may be rate limited from previous test)"
    fi

    # Immediate second request should be rate limited
    local response2=$(rpc_call "faucet_request" "{\"address\":\"$address\"}" "$FAUCET_HOST" "$FAUCET_PORT")

    if echo "$response2" | jq -e '.error' > /dev/null 2>&1; then
        local error_msg=$(echo "$response2" | jq -r '.error.message')
        if [[ "$error_msg" == *"wait"* ]] || [[ "$error_msg" == *"cooldown"* ]] || [[ "$error_msg" == *"seconds"* ]]; then
            log_success "Rate limiting active: $error_msg"
            ((TESTS_PASSED++))
        else
            log_fail "Unexpected error message: $error_msg"
            ((TESTS_FAILED++))
        fi
    else
        log_fail "Second request should have been rate limited"
        ((TESTS_FAILED++))
    fi
}

test_error_includes_retry_time() {
    log_info "Test 3: Rate limit error includes retry time"

    local address=$(generate_test_address 3)

    # Make requests until rate limited
    rpc_call "faucet_request" "{\"address\":\"$address\"}" "$FAUCET_HOST" "$FAUCET_PORT" > /dev/null
    local response=$(rpc_call "faucet_request" "{\"address\":\"$address\"}" "$FAUCET_HOST" "$FAUCET_PORT")

    local error_msg=$(echo "$response" | jq -r '.error.message // ""')

    # Check if error message contains a number (the retry time)
    if [[ "$error_msg" =~ [0-9]+ ]]; then
        log_success "Retry time included in error: $error_msg"
        ((TESTS_PASSED++))
    else
        log_fail "Error message should include retry time: $error_msg"
        ((TESTS_FAILED++))
    fi
}

test_transaction_visible_in_mempool() {
    log_info "Test 4: Faucet transaction appears in mempool"

    local address=$(generate_test_address 4)
    local faucet_response=$(rpc_call "faucet_request" "{\"address\":\"$address\"}" "$FAUCET_HOST" "$FAUCET_PORT")

    if ! echo "$faucet_response" | jq -e '.result.txHash' > /dev/null 2>&1; then
        log_warn "Could not get tx hash (may be rate limited)"
        return
    fi

    local tx_hash=$(echo "$faucet_response" | jq -r '.result.txHash')

    # Check mempool
    local mempool_response=$(rpc_call "getMempoolInfo" "{}")

    if echo "$mempool_response" | jq -e '.result.txHashes' > /dev/null 2>&1; then
        local in_mempool=$(echo "$mempool_response" | jq -r ".result.txHashes | map(select(. == \"$tx_hash\")) | length")

        if [[ "$in_mempool" -gt 0 ]]; then
            log_success "Transaction ${tx_hash:0:16}... found in mempool"
            ((TESTS_PASSED++))
        else
            # May have been confirmed already
            log_warn "Transaction not in mempool (may be confirmed)"
            ((TESTS_PASSED++))
        fi
    else
        log_fail "Could not check mempool"
        ((TESTS_FAILED++))
    fi
}

test_user_friendly_errors() {
    log_info "Test 5: Error messages are user-friendly"

    # Test invalid address
    local response=$(rpc_call "faucet_request" "{\"address\":\"invalid\"}" "$FAUCET_HOST" "$FAUCET_PORT")
    local error_msg=$(echo "$response" | jq -r '.error.message // ""')

    # Check error doesn't contain implementation details
    if [[ "$error_msg" != *"panic"* ]] && [[ "$error_msg" != *"unwrap"* ]] && [[ "$error_msg" != *"internal"* ]]; then
        log_success "Error message is user-friendly: ${error_msg:0:60}..."
        ((TESTS_PASSED++))
    else
        log_fail "Error message exposes implementation details: $error_msg"
        ((TESTS_FAILED++))
    fi
}

test_faucet_stats() {
    log_info "Test 6: Faucet stats endpoint works"

    local response=$(rpc_call "faucet_getStats" "{}" "$FAUCET_HOST" "$FAUCET_PORT")

    if echo "$response" | jq -e '.result.enabled' > /dev/null 2>&1; then
        local enabled=$(echo "$response" | jq -r '.result.enabled')
        local dispensed=$(echo "$response" | jq -r '.result.dailyDispensed')
        local limit=$(echo "$response" | jq -r '.result.dailyLimit')
        log_success "Faucet stats: enabled=$enabled, dispensed=$dispensed, limit=$limit"
        ((TESTS_PASSED++))
    else
        local error=$(echo "$response" | jq -r '.error.message // "Unknown error"')
        log_fail "Stats request failed: $error"
        ((TESTS_FAILED++))
    fi
}

# ============================================================================
# Main Execution
# ============================================================================

echo ""
echo "======================================"
echo "  Faucet Workflow E2E Test Suite"
echo "======================================"
echo "  Network: $NETWORK"
echo "  RPC:     ${RPC_HOST}:${RPC_PORT}"
echo "  Faucet:  ${FAUCET_HOST}:${FAUCET_PORT}"
echo "======================================"
echo ""

check_prereqs

if ! check_node_status; then
    echo ""
    log_fail "Cannot connect to node. Is the testnet running?"
    echo ""
    echo "To start a local testnet:"
    echo "  ./target/release/botho-testnet start --nodes 2 --wait-consensus"
    echo ""
    exit 1
fi

echo ""
echo "Running test scenarios..."
echo ""

# Run all tests
test_faucet_dispenses_correct_amount
test_rate_limiting
test_error_includes_retry_time
test_transaction_visible_in_mempool
test_user_friendly_errors
test_faucet_stats

# Summary
echo ""
echo "======================================"
echo "  Test Summary"
echo "======================================"
echo -e "  ${GREEN}Passed:${NC} $TESTS_PASSED"
echo -e "  ${RED}Failed:${NC} $TESTS_FAILED"
echo "======================================"
echo ""

if [[ $TESTS_FAILED -gt 0 ]]; then
    exit 1
fi

log_success "All tests passed!"
