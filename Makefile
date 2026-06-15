PREFIX ?= $(HOME)/.local

.PHONY: install
install:
	cargo install --force --path crates/reeve --root $(PREFIX)
