.PHONY: build run install uninstall status logs

UNIT_DIR := $(HOME)/.config/systemd/user
ARGS ?=

## build: Release build
build:
	cargo build --release

run:
	cargo run -- --verbose

## install: Install as a systemd user service. Options: make install ARGS="--sound tick --volume 80"
install:
	cargo install --path . --root $(HOME)/.local --force
	mkdir -p $(UNIT_DIR)
	sed "s|@ARGS@|$(ARGS)|" heptix.service > $(UNIT_DIR)/heptix.service
	systemctl --user daemon-reload
	systemctl --user enable heptix.service
	systemctl --user restart heptix.service
	@systemctl --user --no-pager status heptix.service | head -5

## uninstall: Stop the service and remove the binary
uninstall:
	-systemctl --user disable --now heptix.service
	rm -f $(UNIT_DIR)/heptix.service $(HOME)/.local/bin/heptix
	systemctl --user daemon-reload

## status: Show service status
status:
	systemctl --user --no-pager status heptix.service

## logs: Follow service logs
logs:
	journalctl --user -u heptix.service -f
