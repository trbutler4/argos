{
  description = "Personal argos CLI and development tools";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/dc5d91f840324650bac8c379428c7037a416959a";
  inputs.microvm = {
    url = "github:microvm-nix/microvm.nix";
    inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs = { self, nixpkgs, microvm, ... }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      microvmSystem = "x86_64-linux";
      cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);
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
              wrapProgram $out/bin/argos \
                --suffix PATH : ${pkgs.lib.makeBinPath [ pkgs.tmux pkgs.openssh pkgs.git ]}
            '' + pkgs.lib.optionalString
              (pkgs.stdenv.buildPlatform.canExecute pkgs.stdenv.hostPlatform) ''
              # Dynamic completion: the emitted script re-invokes the wrapper on
              # each Tab, so it must be generated after wrapProgram.
              installShellCompletion --cmd argos \
                --zsh <(COMPLETE=zsh $out/bin/argos) \
                --bash <(COMPLETE=bash $out/bin/argos) \
                --fish <(COMPLETE=fish $out/bin/argos)
            '';
            meta = {
              description = "Personal command center for tmux work";
              mainProgram = "argos";
              platforms = systems;
            };
          };
          default = argos;
        } // nixpkgs.lib.optionalAttrs (system == microvmSystem) {
          argos-microvm-prototype = self.nixosConfigurations.argos-microvm-prototype.config.microvm.declaredRunner;
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
