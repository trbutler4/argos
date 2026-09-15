{
  description = "Personal argos CLI and development tools";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/dc5d91f840324650bac8c379428c7037a416959a";

  outputs = { self, nixpkgs, ... }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
    in {
      packages = nixpkgs.lib.genAttrs systems (system:
        let pkgs = import nixpkgs { inherit system; };
        in rec {
          argos = pkgs.rustPlatform.buildRustPackage {
            pname = "argos";
            version = "0.1.0";
            src = pkgs.lib.cleanSource ./.;
            cargoLock.lockFile = ./Cargo.lock;
            nativeBuildInputs = [ pkgs.makeWrapper ];
            postInstall = ''
              wrapProgram $out/bin/argos \
                --prefix PATH : ${pkgs.lib.makeBinPath [ pkgs.tmux pkgs.openssh ]}
            '';
            meta = {
              description = "Personal command center for tmux work";
              mainProgram = "argos";
              platforms = systems;
            };
          };
          default = argos;
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
    };
}
