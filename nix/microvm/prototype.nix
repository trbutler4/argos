{ pkgs, lib, ... }:
{
  networking.hostName = "argos-microvm-prototype";
  system.stateVersion = "25.11";

  users.users.root.hashedPassword = "!";
  services.getty.autologinUser = "root";

  environment.systemPackages = with pkgs; [
    git
    tmux
  ];

  systemd.services.argos-prototype-ready = {
    description = "Argos MicroVM prototype readiness marker";
    wantedBy = [ "multi-user.target" ];
    serviceConfig = {
      Type = "oneshot";
      StandardOutput = "journal+console";
      StandardError = "journal+console";
    };
    script = ''
      echo ARGOS_MICROVM_READY host=$(${lib.getExe' pkgs.nettools "hostname"})
    '';
  };

  microvm = {
    hypervisor = "qemu";
    socket = "control.socket";

    volumes = [
      {
        mountPoint = "/var";
        image = "var.img";
        size = 512;
      }
    ];

    shares = [
      {
        proto = "9p";
        tag = "ro-store";
        source = "/nix/store";
        mountPoint = "/nix/.ro-store";
      }
    ];
  };
}
