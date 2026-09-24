GO      ?= go
BIN     ?= bin
VERSION ?= $(shell git describe --tags --always --dirty 2>/dev/null || echo dev)
LDFLAGS := -X beam/internal/cli.version=$(VERSION)

.PHONY: all build test vet fmt fmt-check tidy check clean

all: check build

build:
	$(GO) build -ldflags "$(LDFLAGS)" -o $(BIN)/ ./cmd/...

test:
	$(GO) test ./...

vet:
	$(GO) vet ./...

fmt:
	$(GO) fmt ./...

fmt-check:
	@test -z "$$(gofmt -l .)" || { echo "gofmt needed:"; gofmt -l .; exit 1; }

tidy:
	$(GO) mod tidy

# What CI runs, and what must pass before a milestone is called done.
check: fmt-check vet test

clean:
	$(GO) clean
	rm -rf $(BIN)
