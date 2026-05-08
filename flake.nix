{
  description = "yasr - yet another screen recorder";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    crane.url = "github:ipetkov/crane";
  };

  outputs =
    { self, nixpkgs, flake-utils, crane }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs { inherit system; };

        nativeBuildInputs = with pkgs; [ pkg-config makeWrapper ];

        buildInputs = with pkgs; [
          wayland
          wayland-protocols
          pipewire
          libxkbcommon
          dbus
          libxcb
          xcbutil
          xcbutilimage
          xcbutilkeysyms
          xcbutilrenderutil
          xcbutilwm
          ffmpeg
          intel-media-driver
        ];

      in
      {
        devShells.default = pkgs.mkShell {
          inherit nativeBuildInputs buildInputs;
          packages = with pkgs; [
            cargo
            rustc
            rust-analyzer
            rustfmt
            clippy
          ];
          shellHook = ''
            export LIBVA_DRIVERS_PATH="${pkgs.intel-media-driver}/lib/dri''${LIBVA_DRIVERS_PATH:+:$LIBVA_DRIVERS_PATH}"
            echo "yasr dev shell — run 'cargo run -- --help' to start"
          '';
        };

        packages =
          let
            craneLib = crane.mkLib pkgs;
            commonArgs = {
              src = craneLib.cleanCargoSource ./.;
              inherit nativeBuildInputs buildInputs;
            };
            cargoArtifacts = craneLib.buildDepsOnly commonArgs;
            yasr = craneLib.buildPackage (commonArgs // {
              inherit cargoArtifacts;
              meta.mainProgram = "yasr-cli";
              postInstall = ''
                for bin in yasr-cli yasr-tui; do
                  wrapProgram $out/bin/$bin \
                    --prefix PATH : "${pkgs.ffmpeg}/bin" \
                    --set LIBVA_DRIVERS_PATH "${pkgs.intel-media-driver}/lib/dri"
                done
              '';
            });
          in
          rec {
            default = yasr // { meta = (yasr.meta or { }) // { mainProgram = "yasr-cli"; }; };
            yasr-cli = yasr // { meta = (yasr.meta or { }) // { mainProgram = "yasr-cli"; }; };
            yasr-tui = yasr // { meta = (yasr.meta or { }) // { mainProgram = "yasr-tui"; }; };
          };
      }
    );
}
