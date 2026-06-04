{
  description = "ICM (fork) — permanent agent memory with a libSQL/Turso backend for concurrent multi-writer storage. See TURSO.md.";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAll = nixpkgs.lib.genAttrs systems;
    in
    {
      packages = forAll (
        system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in
        {
          icm = pkgs.rustPlatform.buildRustPackage {
            pname = "icm";
            version = "0.10.50-turso";

            src = ./.;
            # Build the crate vendor dir straight from our lockfile (which now
            # includes libsql) — no vendor hash to maintain on every dep bump.
            cargoLock.lockFile = ./Cargo.lock;

            # The libSQL/Turso backend is opt-in; the default build is rusqlite.
            # Build the binary with the turso backend (mutually exclusive with the
            # default rusqlite backend, hence --no-default-features).
            buildNoDefaultFeatures = true;
            buildFeatures = [ "turso" "embeddings" "tui" ];
            cargoBuildFlags = [ "-p" "icm-cli" ];

            nativeBuildInputs = [ pkgs.pkg-config ];
            buildInputs = [
              pkgs.openssl
              pkgs.onnxruntime
            ];

            env = {
              OPENSSL_NO_VENDOR = "1";
              ORT_STRATEGY = "system";
              ORT_LIB_LOCATION = "${pkgs.lib.getLib pkgs.onnxruntime}/lib";
            };

            # Workspace tests open libsql in-memory DBs; skip them in the package
            # build (functionality is exercised via the CLI / TURSO.md).
            doCheck = false;

            meta = {
              description = "ICM fork with libSQL/Turso backend (concurrent multi-writer memory)";
              homepage = "https://github.com/rtk-ai/icm";
              mainProgram = "icm";
            };
          };
          default = self.packages.${system}.icm;
        }
      );

      apps = forAll (system: {
        default = {
          type = "app";
          program = "${self.packages.${system}.icm}/bin/icm";
        };
      });
    };
}
