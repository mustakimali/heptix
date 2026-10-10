# SPDX-License-Identifier: GPL-2.0-only
# heptix - the missing haptic feedback for Linux!
# Copyright (C) 2026 M Mustakim Ali
#
# This program is free software; you can redistribute it and/or modify
# it under the terms of the GNU General Public License version 2 as
# published by the Free Software Foundation.
#
# This program is distributed in the hope that it will be useful,
# but WITHOUT ANY WARRANTY; without even the implied warranty of
# MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
# GNU General Public License for more details.
#
# You should have received a copy of the GNU General Public License
# along with this program; if not, see <https://www.gnu.org/licenses/>.

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
