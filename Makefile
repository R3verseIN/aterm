.PHONY: all dev build build-debug run check install clean help

# Default target
all: build-debug

# Install dependencies
install:
	bun install

# Run application in development mode with hot reloading
dev:
	bun run tauri dev

# Build frontend and compile release binary
build:
	bun run build
	cargo build --manifest-path src-tauri/Cargo.toml --release
	@mkdir -p build/bin
	@cp src-tauri/target/release/aterm build/bin/aterm 2>/dev/null || true
	@echo "Production build complete! Binary located at build/bin/aterm"

# Build frontend and debug binary (faster build)
build-debug:
	bun run build
	cargo build --manifest-path src-tauri/Cargo.toml
	@mkdir -p build/bin
	@cp src-tauri/target/debug/aterm build/bin/aterm
	@echo "Debug build complete! Binary located at build/bin/aterm"

# Run the compiled binary
run:
	@if [ -f build/bin/aterm ]; then \
		./build/bin/aterm; \
	elif [ -f src-tauri/target/debug/aterm ]; then \
		./src-tauri/target/debug/aterm; \
	else \
		echo "No built binary found. Run 'make build-debug' first."; \
	fi

# Check frontend and backend code
check:
	bun run build
	cargo check --manifest-path src-tauri/Cargo.toml

# Clean build artifacts
clean:
	rm -rf dist
	rm -rf build/bin
	cargo clean --manifest-path src-tauri/Cargo.toml

# Show help
help:
	@echo "Available make commands:"
	@echo "  make dev         - Launch application in Tauri dev mode"
	@echo "  make build-debug - Build frontend & debug binary to build/bin/aterm"
	@echo "  make build       - Build frontend & release binary to build/bin/aterm"
	@echo "  make run         - Execute built aterm binary"
	@echo "  make check       - Type check frontend & cargo check backend"
	@echo "  make install     - Install bun dependencies"
	@echo "  make clean       - Remove build artifacts and cargo clean"
