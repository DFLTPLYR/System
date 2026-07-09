{
  description = "system: Rust QML plugin";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };
        qt = pkgs.qt6;

        # Merge qtbase + qtdeclarative so that qmake queries return paths
        # covering both. Without this, cxx_qt_build can't find Qt6Qml.
        qt-merged = pkgs.symlinkJoin {
          name = "qt-merged";
          paths = [ qt.qtbase qt.qtdeclarative ];
        };

        # qmake wrapper that merges qtbase/qtdeclarative paths
        qmake-wrapper = pkgs.writeShellScriptBin "qmake-wrapper" ''
          set -euo pipefail
          MERGED="${qt-merged}"
          if [ "$1" = "-query" ]; then
            case "$2" in
              QT_INSTALL_LIBS|QT_HOST_LIBS)
                echo "$MERGED/lib"; exit 0 ;;
              QT_INSTALL_HEADERS|QT_HOST_HEADERS)
                echo "$MERGED/include"; exit 0 ;;
              QT_HOST_LIBEXECS|QT_INSTALL_LIBEXECS)
                echo "$MERGED/libexec"; exit 0 ;;
              QT_HOST_BINS|QT_INSTALL_BINS)
                echo "$MERGED/bin"; exit 0 ;;
            esac
          fi
          exec ${qt.qtbase}/bin/qmake6 "$@"
        '';
      in
      {
        devShells.default = pkgs.mkShell {
          nativeBuildInputs = with pkgs; [
            cargo rustc
            pkg-config cmake gnumake
            stdenv.cc
            qt.qtbase qt.qtdeclarative qt.qttools
          ];

          shellHook = ''
            # Override QMAKE here so it takes effect after qt6 setup hooks
            export QMAKE="${qmake-wrapper}/bin/qmake-wrapper"
            echo "system dev shell — run 'just build' to build"
          '';
        };
      });
}
