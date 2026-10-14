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

.PHONY: build run install uninstall status logs logs-reader

TARGET_DIR := $(or $(CARGO_TARGET_DIR),target)
# Overridden by the installer tests, which use a binary that is already built.
CARGO ?= cargo
# Passed to the installer: make install MODE=service|direct, make install YES=1
MODE ?=
YES ?=

## build: Release build
build:
	$(CARGO) build --release

run:
	cargo run -- --verbose

## install: Build, then run the installer, which asks how heptix should read your input:
##          service mode (recommended, needs sudo) or direct mode (not recommended).
##          Skip the question with: make install MODE=service  (or MODE=direct, YES=1)
install:
	$(CARGO) build --release
	rm -rf $(TARGET_DIR)/stage && mkdir -p $(TARGET_DIR)/stage
	cp $(TARGET_DIR)/release/heptix $(TARGET_DIR)/stage/
	cp -r packaging $(TARGET_DIR)/stage/packaging
	HEPTIX_LOCAL_DIR=$(abspath $(TARGET_DIR)/stage) sh install.sh $(if $(MODE),--$(MODE)) $(if $(YES),--yes)

## uninstall: Remove heptix, whichever mode was installed (keeps your settings and stats)
uninstall:
	sh install.sh --uninstall

## status: Show the reader service (service mode) and your user service
status:
	-systemctl --no-pager status heptix-reader.service | head -5
	-systemctl --user --no-pager status heptix.service | head -5

## logs: Follow your user service's log
logs:
	journalctl --user -u heptix.service -f

## logs-reader: Follow the reader service's log (service mode; may need sudo)
logs-reader:
	journalctl -u heptix-reader.service -f
