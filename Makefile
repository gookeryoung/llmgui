PYTHON ?= python
CARGO ?= cargo
PYTEST ?= $(PYTHON) -m pytest

.PHONY: build test cov check-fast check-docs adr-check gitkeep-check flow bench-runtime bench-download clean

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

bench-runtime:
	$(CARGO) run -p llmgui-cli -- bench --iterations 4 --concurrency 1 --max-tokens 64

bench-download:
	@echo "bench-download requires a running local fixture; see docs/adr/0001-local-llm-runtime-finetune.md §13.4"

clean:
	$(CARGO) clean
	$(PYTHON) -c "import shutil, pathlib; [shutil.rmtree(p, ignore_errors=True) for p in ['python/.pytest_cache','.coverage','htmlcov']]"
