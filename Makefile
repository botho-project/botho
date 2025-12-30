# Cadence/Botho Makefile
# Convenient aliases for common development tasks

.PHONY: build test clean fuzz-% fuzz-quick fuzz-overnight fuzz-parallel fuzz-status fuzz-stop fuzz-coverage

# ============================================================
# Build & Test
# ============================================================

build:
	cargo build

build-release:
	cargo build --release

test:
	cargo test

check:
	cargo check

clippy:
	cargo clippy -- -D warnings

fmt:
	cargo fmt

fmt-check:
	cargo fmt -- --check

# ============================================================
# Fuzzing - Quick Reference
# ============================================================
#
# Quick (5 min):     make fuzz-quick TARGET=fuzz_block
# Medium (30 min):   make fuzz-medium TARGET=fuzz_block
# Long (1 hour):     make fuzz-long TARGET=fuzz_block
# Overnight (8h ea): make fuzz-overnight
# Parallel (1h all): make fuzz-parallel
# Custom duration:   make fuzz-timed TARGET=fuzz_block DURATION=600
# Indefinite:        make fuzz TARGET=fuzz_block
# Status:            make fuzz-status
# Stop all:          make fuzz-stop
# Coverage stats:    make fuzz-coverage
#
# Available targets:
#   fuzz_address_parsing, fuzz_block, fuzz_lion_signature,
#   fuzz_mlkem_decapsulation, fuzz_network_messages, fuzz_pq_keys,
#   fuzz_pq_transaction, fuzz_ring_signature, fuzz_rpc_request,
#   fuzz_transaction
# ============================================================

fuzz:
ifndef TARGET
	@echo "Usage: make fuzz TARGET=<target>"
	@echo "Run 'make fuzz-list' to see available targets"
else
	./scripts/fuzz.sh run $(TARGET)
endif

fuzz-list:
	@./scripts/fuzz.sh list

fuzz-quick:
ifndef TARGET
	@echo "Usage: make fuzz-quick TARGET=<target>"
else
	./scripts/fuzz.sh quick $(TARGET)
endif

fuzz-medium:
ifndef TARGET
	@echo "Usage: make fuzz-medium TARGET=<target>"
else
	./scripts/fuzz.sh medium $(TARGET)
endif

fuzz-long:
ifndef TARGET
	@echo "Usage: make fuzz-long TARGET=<target>"
else
	./scripts/fuzz.sh long $(TARGET)
endif

fuzz-timed:
ifndef TARGET
	@echo "Usage: make fuzz-timed TARGET=<target> DURATION=<seconds>"
else ifndef DURATION
	@echo "Usage: make fuzz-timed TARGET=<target> DURATION=<seconds>"
else
	./scripts/fuzz.sh timed $(TARGET) $(DURATION)
endif

fuzz-overnight:
	./scripts/fuzz.sh overnight

fuzz-parallel:
	./scripts/fuzz.sh parallel

fuzz-status:
	@./scripts/fuzz.sh status

fuzz-stop:
	./scripts/fuzz.sh stop

fuzz-coverage:
	@./scripts/fuzz.sh coverage

# ============================================================
# Node
# ============================================================

run:
	cargo run --bin botho

run-release:
	cargo run --release --bin botho

# ============================================================
# Docs
# ============================================================

doc:
	cargo doc --no-deps --open

# ============================================================
# Clean
# ============================================================

clean:
	cargo clean

clean-fuzz:
	rm -rf fuzz/target fuzz/logs
