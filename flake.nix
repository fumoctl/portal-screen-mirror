{
  description = "Standalone Wayland screen, window, and region mirror for any compositor via XDG ScreenCast + PipeWire + EGL";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-26.05";
  };

  outputs =
    { self, nixpkgs }:
    let
      inherit (nixpkgs) lib;
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = f: lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});

      mkPackage =
        pkgs:
        let
          cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);
          runtimeLibs = with pkgs; [
            wayland
            libxkbcommon
            libglvnd
          ];
        in
        pkgs.rustPlatform.buildRustPackage {
          pname = cargoToml.package.name;
          inherit (cargoToml.package) version;
          src = lib.cleanSource ./.;

          cargoLock.lockFile = ./Cargo.lock;

          nativeBuildInputs = with pkgs; [
            pkg-config
            makeWrapper
          ];

          buildInputs = with pkgs; [
            pipewire
            wayland
            libxkbcommon
            libglvnd
          ];

          postInstall = ''
            install -Dm644 assets/portal-screen-mirror.desktop \
              $out/share/applications/portal-screen-mirror.desktop
            install -Dm644 assets/portal-screen-mirror.svg \
              $out/share/icons/hicolor/scalable/apps/portal-screen-mirror.svg
          '';

          postFixup = ''
            wrapProgram $out/bin/portal-screen-mirror \
              --prefix LD_LIBRARY_PATH : "${lib.makeLibraryPath runtimeLibs}:/run/opengl-driver/lib"
          '';

          meta = {
            description = "Compositor-agnostic Wayland screen, window, and region mirror using XDG ScreenCast, PipeWire DMA-BUF/SHM, and EGL/GLES2";
            license = lib.licenses.mit;
            platforms = lib.platforms.linux;
            mainProgram = "portal-screen-mirror";
          };
        };
    in
    {
      packages = forAllSystems (pkgs: rec {
        portal-screen-mirror = mkPackage pkgs;
        default = portal-screen-mirror;
      });

      apps = forAllSystems (
        pkgs:
        let
          pkg = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
        in
        {
          default = {
            type = "app";
            program = lib.getExe pkg;
            inherit (pkg) meta;
          };
        }
      );

      overlays.default = final: _prev: {
        portal-screen-mirror = mkPackage final;
      };

      nixosModules.default =
        {
          config,
          lib,
          pkgs,
          ...
        }:
        let
          cfg = config.programs.portal-screen-mirror;
        in
        {
          options.programs.portal-screen-mirror = {
            enable = lib.mkEnableOption "portal-screen-mirror Wayland screen/window/region mirroring tool";
            package = lib.mkOption {
              type = lib.types.package;
              default = mkPackage pkgs;
              description = "The portal-screen-mirror package to install.";
            };
          };

          config = lib.mkIf cfg.enable {
            environment.systemPackages = [ cfg.package ];
          };
        };

      homeManagerModules.default =
        {
          config,
          lib,
          pkgs,
          ...
        }:
        let
          cfg = config.programs.portal-screen-mirror;
        in
        {
          options.programs.portal-screen-mirror = {
            enable = lib.mkEnableOption "portal-screen-mirror Wayland screen/window/region mirroring tool";
            package = lib.mkOption {
              type = lib.types.package;
              default = mkPackage pkgs;
              description = "The portal-screen-mirror package to install.";
            };
          };

          config = lib.mkIf cfg.enable {
            home.packages = [ cfg.package ];
          };
        };

      devShells = forAllSystems (
        pkgs:
        let
          runtimeLibs = with pkgs; [
            wayland
            libxkbcommon
            libglvnd
          ];
        in
        {
          default = pkgs.mkShell {
            packages = with pkgs; [
              cargo
              rustc
              rust-analyzer
              clippy
              rustfmt
              pkg-config
              pipewire
              wayland
              libxkbcommon
              libglvnd
            ];

            LD_LIBRARY_PATH = "${lib.makeLibraryPath runtimeLibs}:/run/opengl-driver/lib";
          };
        }
      );
    };
}
