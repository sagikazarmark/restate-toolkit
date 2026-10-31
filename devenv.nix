{
  pkgs,
  lib,
  config,
  ...
}:

{
  dotenv.enable = true;

  dagger.enable = true;
  env.DAGGER_X_RELEASE = "v1.0.0-beta.14";

  packages = with pkgs; [
    lld
    cargo-audit
    cargo-deny
    cargo-hack
    cargo-nextest
    cargo-release
    cargo-watch
  ];

  languages = {
    rust = {
      enable = true;
      channel = "stable";
      targets = [ "wasm32-unknown-unknown" ];
    };
  };

  services.restate = {
    enable = true;
    package = pkgs.restate-bin;
  };

  # The end-to-end tests start their own restate-server.
  env.RESTATE_SERVER_BIN = lib.getExe' config.services.restate.package "restate-server";
}
