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

{
  description = "heptix - the missing haptic feedback for Linux!";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAll = f: nixpkgs.lib.genAttrs systems (s: f nixpkgs.legacyPackages.${s});
    in {
      packages = forAll (pkgs: {
        default = pkgs.rustPlatform.buildRustPackage {
          pname = "heptix";
          version = "0.7.0";
          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;
          nativeBuildInputs = [ pkgs.makeWrapper ];
          # Audio sinks are found on PATH at runtime; add pipewire as a default.
          postInstall = ''
            wrapProgram $out/bin/heptix --suffix PATH : ${pkgs.lib.makeBinPath [ pkgs.pipewire pkgs.alsa-utils ]}
          '';
          meta.mainProgram = "heptix";
        };
      });

      # The recommended setup: a reader service (own user, `input` group, no network) that
      # serves input events to the verified heptix of whoever is at the keyboard, so that
      # your own programs never need the `input` group. Untested on a real NixOS system.
      nixosModules.default = { config, lib, pkgs, ... }:
        let
          cfg = config.services.heptix;
          pkg = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
        in {
          options.services.heptix.enable = lib.mkEnableOption "the heptix reader service and per-user client";

          config = lib.mkIf cfg.enable {
            users.groups.heptix = { };
            users.users.heptix = {
              isSystemUser = true;
              group = "heptix";
              extraGroups = [ "input" ];
              description = "heptix input reader";
            };
            environment.systemPackages = [ pkg ];

            systemd.services.heptix-reader = {
              description = "heptix input reader (serves the verified heptix client)";
              wantedBy = [ "multi-user.target" ];
              serviceConfig = {
                Type = "simple";
                User = "heptix";
                Group = "heptix";
                ExecStart = "${pkg}/bin/heptix --reader";
                Restart = "on-failure";
                RestartSec = 2;
                RuntimeDirectory = "heptix";
                RuntimeDirectoryMode = "0755";
                UMask = "0022";
                # Needed to read another user's /proc/<pid>/exe; the ptrace calls are blocked below.
                AmbientCapabilities = [ "CAP_SYS_PTRACE" ];
                CapabilityBoundingSet = [ "CAP_SYS_PTRACE" ];
                NoNewPrivileges = true;
                SystemCallArchitectures = "native";
                SystemCallFilter = [ "@system-service" "~@debug @mount @swap @reboot @raw-io @module @obsolete" ];
                PrivateNetwork = true;
                RestrictAddressFamilies = [ "AF_UNIX" ];
                PrivateTmp = true;
                ProtectHome = true;
                ProtectSystem = "strict";
                ProtectKernelTunables = true;
                ProtectKernelModules = true;
                ProtectKernelLogs = true;
                ProtectControlGroups = true;
                ProtectClock = true;
                ProtectHostname = true;
                RestrictNamespaces = true;
                RestrictRealtime = true;
                RestrictSUIDSGID = true;
                LockPersonality = true;
                MemoryDenyWriteExecute = true;
                DevicePolicy = "closed";
                DeviceAllow = [ "char-input rw" ];
              };
            };

            systemd.user.services.heptix = {
              description = "heptix - the missing haptic feedback for Linux";
              wantedBy = [ "default.target" ];
              after = [ "pipewire.service" ];
              serviceConfig = {
                ExecStart = "${pkg}/bin/heptix --client";
                Restart = "on-failure";
                RestartSec = 2;
                RestrictAddressFamilies = [ "AF_UNIX" ];
                IPAddressDeny = "any";
                NoNewPrivileges = true;
              };
            };
          };
        };

      devShells = forAll (pkgs: {
        default = pkgs.mkShell {
          packages = [ pkgs.cargo pkgs.rustc pkgs.clippy pkgs.pipewire pkgs.alsa-utils ];
        };
      });
    };
}
