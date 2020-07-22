{ moz_overlay ? import (builtins.fetchTarball https://github.com/mozilla/nixpkgs-mozilla/archive/master.tar.gz)
, nixpkgs ? import (builtins.fetchTarball https://github.com/nixos/nixpkgs-channels/archive/nixos-20.03.tar.gz) {
    overlays = [ moz_overlay ];
  }
}:

with nixpkgs.latest.rustChannels;
with nixpkgs;
with llvmPackages_latest;

let
  nightly = rustChannelOf { date = "2020-07-01"; channel = "nightly"; };
  rustNigthly = nightly.rust.override {
    targets = [ "wasm32-unknown-unknown" ];
  };
in
  stdenv.mkDerivation {
    name = "substrate-nix-shell";
    buildInputs = [ rustNigthly wasm-gc ];
    LIBCLANG_PATH = "${libclang}/lib";
    PROTOC = "${protobuf}/bin/protoc";
  }
