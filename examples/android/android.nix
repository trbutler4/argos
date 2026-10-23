# Explicit, trusted guest policy, not host configuration. This example accepts
# Android licenses and starts a headless emulator through native systemd.
{ pkgs, ... }:
let
  android = pkgs.androidenv.composeAndroidPackages {
    cmdLineToolsVersion = "11.0";
    platformVersions = [ "36" ];
    buildToolsVersions = [ "36.0.0" ];
    includeEmulator = true;
    includeSystemImages = true;
    systemImageTypes = [ "google_apis" ];
    abiVersions = [ "x86_64" ];
    includeNDK = true;
    ndkVersions = [ "27.1.12297006" ];
    cmakeVersions = [ "3.22.1" ];
    includeSources = false;
    useGoogleAPIs = false;
    includeExtras = [];
  };
  sdk = "${android.androidsdk}/libexec/android-sdk";
  state = "/var/lib/android-dev";
  emulator = pkgs.writeShellApplication {
    name = "argos-android-emulator";
    runtimeInputs = [ pkgs.coreutils pkgs.gnugrep pkgs.jdk21 ];
    text = ''
      export JAVA_HOME=${pkgs.jdk21}
      export ANDROID_HOME=${sdk}
      export ANDROID_SDK_ROOT=${sdk}
      export ANDROID_USER_HOME=${state}/android
      export ANDROID_AVD_HOME="$ANDROID_USER_HOME/avd"
      mkdir -p "$ANDROID_AVD_HOME" ${state}/tmp
      imageSource=${sdk}/system-images/android-36/google_apis/x86_64
      imageDir=${state}/system-image
      # Guest-local random reads avoid paging every Android access over 9p.
      if ! test -f "$imageDir/.argos-image-source" || \
          test "$(cat "$imageDir/.argos-image-source")" != "$imageSource"; then
        mkdir -p "$imageDir"
        cp -a --sparse=always "$imageSource/." "$imageDir/"
        printf '%s\n' "$imageSource" > "$imageDir/.argos-image-source"
      fi
      if ! test -f "$ANDROID_AVD_HOME/task.ini"; then
        printf 'no\n' | ${sdk}/cmdline-tools/11.0/bin/avdmanager create avd \
          --name task --package 'system-images;android-36;google_apis;x86_64' \
          --device pixel_2 --path "$ANDROID_AVD_HOME/task.avd"
        cat >> "$ANDROID_AVD_HOME/task.avd/config.ini" <<'AVD'
      hw.lcd.width=720
      hw.lcd.height=1280
      hw.lcd.density=320
      hw.ramSize=1536
      hw.cpu.ncore=2
      disk.dataPartition.size=4G
      showDeviceFrame=no
      AVD
      fi
      exec ${sdk}/emulator/emulator -avd task -sysdir "$imageDir" -port 5554 \
        -no-window -no-audio -no-boot-anim -no-snapshot -gpu swiftshader \
        -accel on -memory 1536 -cores 2 -skin 720x1280 \
        -camera-back none -camera-front none "$@"
    '';
  };
in {
  nixpkgs.config.allowUnfree = true;
  nixpkgs.config.android_sdk.accept_license = true;
  boot.kernelModules = [ "kvm-amd" "kvm-intel" ];
  environment.systemPackages = [ android.androidsdk pkgs.jdk21 pkgs.maestro emulator ];
  environment.variables = {
    ANDROID_HOME = sdk;
    ANDROID_SDK_ROOT = sdk;
    ANDROID_USER_HOME = "${state}/android";
    ANDROID_AVD_HOME = "${state}/android/avd";
    ANDROID_SERIAL = "emulator-5554";
    JAVA_HOME = "${pkgs.jdk21}";
    GRADLE_USER_HOME = "${state}/gradle";
    COREPACK_HOME = "${state}/corepack";
    XDG_CACHE_HOME = "${state}/cache";
    YARN_GLOBAL_FOLDER = "${state}/yarn";
    YARN_NETWORK_CONCURRENCY = "4";
    YARN_TASK_POOL_CONCURRENCY = "1";
    YARN_TASK_POOL_MODE = "async";
    NODE_OPTIONS = "--max-old-space-size=2048";
    TMPDIR = "${state}/tmp";
    MAESTRO_OPTS = "-Xmx384m -Djava.io.tmpdir=${state}/tmp";
    MAESTRO_DRIVER_STARTUP_TIMEOUT = "180000";
    MAESTRO_CLI_NO_ANALYTICS = "1";
    MAESTRO_CLI_ANALYSIS_NOTIFICATION_DISABLED = "true";
    REACT_NATIVE_PACKAGER_HOSTNAME = "127.0.0.1";
  };
  systemd.tmpfiles.rules = [
    "d ${state} 0700 root root -"
    "d ${state}/tmp 0700 root root -"
    "d ${state}/maestro 0700 root root -"
    "L /root/.maestro - - - - ${state}/maestro"
  ];
  systemd.services.android-emulator = {
    description = "Opt-in isolated Android development emulator";
    wantedBy = [ "multi-user.target" ];
    after = [ "local-fs.target" ];
    serviceConfig = {
      ExecStart = "${emulator}/bin/argos-android-emulator";
      Restart = "on-failure";
      RestartSec = 5;
      TimeoutStartSec = 300;
      TimeoutStopSec = 30;
    };
    postStart = ''
      export ANDROID_USER_HOME=${state}/android
      ${sdk}/platform-tools/adb -s emulator-5554 wait-for-device
      until test "$(${sdk}/platform-tools/adb -s emulator-5554 shell getprop sys.boot_completed | ${pkgs.coreutils}/bin/tr -d '\r')" = 1; do
        sleep 2
      done
      ${sdk}/platform-tools/adb -s emulator-5554 shell wm density 320
    '';
  };
}
