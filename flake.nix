{
  description = "Veil - local-first privacy platform";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }:
    let
      system = "x86_64-linux";
      pkgs = import nixpkgs { inherit system; };
      schemaPath = pkgs.lib.concatMapStringsSep ":" (pkg: "${pkg}/share/gsettings-schemas/${pkg.name}") [
        pkgs.gtk3
        pkgs.gsettings-desktop-schemas
      ];
      devShell = x11Workaround: pkgs.mkShell {
        packages = with pkgs; [
          python312
          nodejs_22
          rustc
          cargo
          pkg-config
          git
          gh
          gcc
          gnumake
          perl
          openssl
          glib
          gtk3
          gsettings-desktop-schemas
          webkitgtk_4_1
          librsvg
          wrapGAppsHook4
        ];

        shellHook = ''
          export LD_LIBRARY_PATH="${pkgs.lib.makeLibraryPath [ pkgs.stdenv.cc.cc.lib pkgs.zlib ]}:$LD_LIBRARY_PATH"
          export XDG_DATA_DIRS="${schemaPath}:''${XDG_DATA_DIRS:-/usr/local/share:/usr/share}"
          ${pkgs.lib.optionalString x11Workaround ''
            export GDK_BACKEND=x11
            export WEBKIT_DISABLE_DMABUF_RENDERER=1
          ''}
          echo ""
          echo "================================"
          echo "    VEIL Development Environment"
          echo "================================"
          echo ""
          echo "Python: $(python3 --version)"
          echo "Node:   $(node --version)"
          echo "Rust:   $(rustc --version)"
          echo ""
        '';
      };
    in
    {
      devShells.${system} = {
        default = devShell false;
        nixos-x11 = devShell true;
      };
    };
}
