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

# A distro with real systemd (PID 1), a lingering login user and sudo, to test the
# installer end to end. See run.sh; BASE is the distro image.
ARG BASE=debian:12
FROM ${BASE}
COPY setup.sh /setup.sh
RUN sh /setup.sh && rm /setup.sh
STOPSIGNAL SIGRTMIN+3
CMD ["/sbin/init"]
