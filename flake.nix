{
  description = "Personal argos CLI and development tools";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/dc5d91f840324650bac8c379428c7037a416959a";
  inputs.microvm = {
    url = "github:microvm-nix/microvm.nix";
    inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs = { self, nixpkgs, microvm, ... }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "aarch64-darwin" ];
      microvmSystem = "x86_64-linux";
      cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);
      installCompletions = command: ''
        # Dynamic completion must execute on the first zsh fpath autoload too.
        COMPLETE=zsh ${command} > argos.zsh
        echo '_clap_dynamic_completer_argos "$@"' >> argos.zsh
        installShellCompletion --cmd argos \
          --zsh argos.zsh \
          --bash <(COMPLETE=bash ${command}) \
          --fish <(COMPLETE=fish ${command})
      '';
    in {
      packages = nixpkgs.lib.genAttrs systems (system:
        let pkgs = import nixpkgs { inherit system; };
        in rec {
          argos = pkgs.rustPlatform.buildRustPackage {
            pname = "argos";
            version = cargoToml.package.version;
            src = pkgs.lib.cleanSource ./.;
            cargoLock.lockFile = ./Cargo.lock;
            nativeBuildInputs = [ pkgs.makeWrapper pkgs.installShellFiles ];
            postInstall = ''
              install -Dm644 examples/connect.tmux.conf $out/share/argos/connect.tmux.conf
              wrapProgram $out/bin/argos \
                --suffix PATH : ${pkgs.lib.makeBinPath [ pkgs.tmux pkgs.openssh pkgs.git ]}
            '' + pkgs.lib.optionalString
              (pkgs.stdenv.buildPlatform.canExecute pkgs.stdenv.hostPlatform) (installCompletions "$out/bin/argos");
            meta = {
              description = "Personal command center for tmux work";
              mainProgram = "argos";
              platforms = systems;
            };
          };
          default = argos;
        } // nixpkgs.lib.optionalAttrs (system == microvmSystem) {
          argos-microvm-prototype = self.nixosConfigurations.argos-microvm-prototype.config.microvm.declaredRunner;
          # Downloadable artifact: a real static musl ELF, without a Nix wrapper
          # or store-path interpreter. SSH/tmux remain runtime tools on PATH.
          argos-release = pkgs.pkgsStatic.rustPlatform.buildRustPackage {
            pname = "argos";
            version = cargoToml.package.version;
            src = pkgs.lib.cleanSource ./.;
            cargoLock.lockFile = ./Cargo.lock;
            nativeBuildInputs = [ pkgs.installShellFiles ];
            postInstall = ''
              install -Dm644 examples/connect.tmux.conf $out/share/argos/connect.tmux.conf
              export PATH="$out/bin:$PATH"
            '' + installCompletions "argos";
            meta = {
              description = "Standalone Argos binary for x86-64 Linux";
              mainProgram = "argos";
              platforms = [ "x86_64-linux" ];
            };
          };
        });

      apps = nixpkgs.lib.genAttrs systems (system: {
        default = {
          type = "app";
          program = "${self.packages.${system}.default}/bin/argos";
        };
      });

      devShells = nixpkgs.lib.genAttrs systems (system:
        let pkgs = import nixpkgs { inherit system; };
        in {
          default = pkgs.mkShell {
            packages = with pkgs; [
              cargo
              rustc
              rustfmt
              clippy
              rust-analyzer
              tmux
              openssh
              python3
              zsh
              actionlint
              git
            ];
          };
        });

      nixosConfigurations.argos-microvm-prototype = nixpkgs.lib.nixosSystem {
        system = microvmSystem;
        modules = [
          microvm.nixosModules.microvm
          ./nix/microvm/prototype.nix
        ];
      };
    };
}
