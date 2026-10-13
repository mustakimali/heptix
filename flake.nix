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
          version = "0.8.0";
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
          options.services.heptix.allowUids = lib.mkOption {
            type = lib.types.listOf lib.types.int;
            default = [ ];
            example = [ 1000 ];
            description = ''
              User ids the reader serves when there is no active graphical session
              (headless machines). Normally empty: it serves the user who owns the
              active session on the seat.
            '';
          };

          config = lib.mkIf cfg.enable {
            users.groups.heptix = { };
            users.users.heptix = {
              isSystemUser = true;
              group = "heptix";
              extraGroups = [ "input" ];
              description = "heptix input reader";
            };
            environment.systemPackages = [ pkg ];
            environment.etc."heptix/allow-uids" = lib.mkIf (cfg.allowUids != [ ]) {
              text = lib.concatMapStringsSep "\n" toString cfg.allowUids + "\n";
            };

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

      # `nix build .#checks.x86_64-linux.nixos-module` boots a NixOS VM with the module.
      checks = nixpkgs.lib.genAttrs [ "x86_64-linux" "aarch64-linux" ] (system:
        let pkgs = nixpkgs.legacyPackages.${system}; in {
          nixos-module = pkgs.testers.runNixOSTest {
            name = "heptix-module";
            nodes.machine = { pkgs, ... }: {
              imports = [ self.nixosModules.default ];
              services.heptix.enable = true;
              services.heptix.allowUids = [ 1000 ]; # a VM has no graphical session
              users.users.alice = { isNormalUser = true; uid = 1000; };
              users.users.bob = { isNormalUser = true; uid = 1001; };
              environment.systemPackages = [ pkgs.python3 pkgs.iproute2 ];
            };
            testScript = ''
              probe = """
              import socket, sys
              s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
              s.settimeout(5)
              s.connect("/run/heptix/events.sock")
              s.sendall(bytes([11, 0x80, 3, 200, 0, 0, 0, 24, 0, 0, 0, 0]))
              try:
                  data = s.recv(64)
              except Exception:
                  data = b""
              sys.exit(0 if data == b"" else 1)
              """
              machine.wait_for_unit("multi-user.target")
              machine.wait_for_unit("heptix-reader.service")
              machine.succeed("test -S /run/heptix/events.sock")

              with subtest("the reader is hardened"):
                  pid = machine.succeed("systemctl show -p MainPID --value heptix-reader.service").strip()
                  machine.succeed(f"grep -q '^Seccomp:.2' /proc/{pid}/status")
                  machine.succeed(f"grep -q '^NoNewPrivs:.1' /proc/{pid}/status")
                  machine.succeed(f"grep -q '^CapEff:.0000000000080000' /proc/{pid}/status")
                  machine.succeed(f"[ \"$(nsenter -t {pid} -n ip -o link | wc -l)\" = 1 ]")

              with subtest("alice's heptix passes the reader's verification"):
                  machine.succeed("loginctl enable-linger alice")
                  machine.wait_for_unit("user@1000.service")
                  machine.wait_until_succeeds(
                      "su alice -c 'XDG_RUNTIME_DIR=/run/user/1000 systemctl --user is-active heptix.service'", timeout=60)
                  machine.wait_until_succeeds(
                      "journalctl --no-pager -o cat _UID=1000 _SYSTEMD_USER_UNIT=heptix.service | grep -q 'reader: connected'", timeout=60)
                  machine.fail("journalctl -u heptix-reader.service --no-pager -o cat | grep -q refused")

              with subtest("everyone else is refused"):
                  machine.succeed("echo '%s' > /tmp/probe.py" % probe)
                  machine.succeed("su alice -c 'python3 /tmp/probe.py'")
                  machine.succeed("su bob -c 'python3 /tmp/probe.py'")
                  machine.wait_until_succeeds("journalctl -u heptix-reader.service --no-pager -o cat | grep -q refused")
                  machine.succeed("su alice -c 'XDG_RUNTIME_DIR=/run/user/1000 systemctl --user is-active heptix.service'")
            '';
          };
        });

      devShells = forAll (pkgs: {
        default = pkgs.mkShell {
          packages = [ pkgs.cargo pkgs.rustc pkgs.clippy pkgs.pipewire pkgs.alsa-utils ];
        };
      });
    };
}
