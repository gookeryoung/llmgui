PYTHON ?= python
CARGO ?= cargo
GIT ?= git
PYTEST ?= $(PYTHON) -m pytest

# Extra environment for `make desktop` (e.g. WSLg software rendering):
#   make desktop DESKTOP_ENV="GDK_BACKEND=x11 WEBKIT_DISABLE_DMABUF_RENDERER=1"
# DESKTOP_ARGS is appended verbatim (e.g. "serve --backend upstream --model x.gguf").
DESKTOP_ENV ?=
DESKTOP_ARGS ?=
# Turbo acceptance flow (ADR 0004):
#   make llama-server-turbo MODEL=/path/Qwen3-Coder-30B-A3B-Instruct-Q4_K_M.gguf
#   make bench-turbo
# Override engine/port/targets without editing this file.
LLAMA_SERVER ?= llama-server
MODEL ?=
PORT ?= 18080
UPSTREAM_URL ?= http://127.0.0.1:$(PORT)
ITERATIONS ?= 5
WARMUP ?= 3
MAX_TOKENS ?= 256
TARGET_MIN ?= 90
TARGET_MAX ?= 120
# `make push` pushes the current branch to every configured remote.
REMOTES ?= $(shell $(GIT) remote)
BRANCH ?= $(shell $(GIT) rev-parse --abbrev-ref HEAD)

.PHONY: build test cov check-fast check-docs adr-check gitkeep-check flow push desktop desktop-accept bench-runtime bench-download bench-turbo llama-server-turbo serve-turbo clean

build:
	$(CARGO) build --workspace

release:
	$(CARGO) build --release -p llmgui-cli

test:
	$(CARGO) test --workspace
	$(PYTEST) python/tests

cov:
	$(PYTEST) python/tests --cov --cov-report=term-missing
	$(CARGO) llvm-cov --workspace --summary-only 2>NUL || echo "cargo-llvm-cov not installed; Rust coverage skipped"

check-fast:
	$(CARGO) fmt --all --check
	$(CARGO) clippy --workspace --all-targets -- -D warnings
	$(PYTHON) -m ruff check python
	$(PYTHON) -m compileall -q python/llmgui_train/src
	$(MAKE) adr-check

check-docs:
	$(PYTHON) scripts/gitkeep_check.py
	$(PYTHON) scripts/check_adr_landing.py

adr-check:
	$(PYTHON) scripts/check_adr_landing.py

gitkeep-check:
	$(PYTHON) scripts/gitkeep_check.py

flow: check-fast test

# Push to every remote; a failing remote does not block the others, but the
# target still exits non-zero so the failure is visible.
push:
	@if [ "$(strip $(REMOTES))" = "" ]; then echo "no git remote configured"; exit 1; fi
	@failed=0; \
	for remote in $(REMOTES); do \
	  echo "==> $(GIT) push $$remote $(BRANCH)"; \
	  $(GIT) push $$remote $(BRANCH) || failed=1; \
	done; \
	if [ $$failed -ne 0 ]; then echo "push failed for at least one remote"; exit 1; fi; \
	echo "pushed $(BRANCH) to: $(REMOTES)"

desktop:
	$(DESKTOP_ENV) $(CARGO) run -p llmgui-desktop -- $(DESKTOP_ARGS)

desktop-accept:
	$(DESKTOP_ENV) $(CARGO) run -p llmgui-desktop -- --selftest $(DESKTOP_ARGS)

bench-runtime:
	$(CARGO) run -p llmgui-cli -- bench --iterations 4 --concurrency 1 --max-tokens 64

# Start llama.cpp with the exact `turbo` flags from ADR 0004.
# MODEL is required; e.g. MODEL=models/Qwen3-Coder-30B-A3B-Instruct-Q4_K_M.gguf
llama-server-turbo:
	@if [ -z "$(MODEL)" ]; then \
	  echo "usage: make llama-server-turbo MODEL=/path/to/Qwen3-Coder-30B-A3B-Instruct-Q4_K_M.gguf [PORT=18080] [LLAMA_SERVER=./llama-server]"; \
	  exit 1; \
	fi
	$(LLAMA_SERVER) -m $(MODEL) \
	  -ngl 999 -fa on -ctk q4_0 -ctv q4_0 \
	  -c 8192 --no-context-shift -b 2048 -ub 512 \
	  -t 8 -tb 8 --host 127.0.0.1 --port $(PORT)

# Single-stream decode acceptance: median of ITERATIONS, warmup excluded.
bench-turbo:
	LLMGUI_UPSTREAM_URL=$(UPSTREAM_URL) $(CARGO) run -p llmgui-cli -- bench \
	  --backend upstream --profile turbo --concurrency 1 \
	  --iterations $(ITERATIONS) --warmup $(WARMUP) --max-tokens $(MAX_TOKENS) \
	  --target-min $(TARGET_MIN) --target-max $(TARGET_MAX)

# Serve the Web GUI/OpenAI API against the already running upstream engine.
serve-turbo:
	LLMGUI_UPSTREAM_URL=$(UPSTREAM_URL) $(CARGO) run -p llmgui-cli -- serve \
	  --backend upstream --profile turbo --port 11435

bench-download:
	@echo "bench-download requires a running local fixture; see docs/adr/0001-local-llm-runtime-finetune.md §13.4"

clean:
	$(CARGO) clean
	$(PYTHON) -c "import shutil, pathlib; [shutil.rmtree(p, ignore_errors=True) for p in ['python/.pytest_cache','.coverage','htmlcov']]"
